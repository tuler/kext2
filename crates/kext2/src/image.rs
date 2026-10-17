//! Reading a starting image (`-x`, SPEC §11.1).

use crate::error::{io_err, show, Error, Result};
use crate::format::{decode_dev, get16, get32};
use crate::runs::ZeroMapBuilder;
use crate::tree::{Data, Kind, Node, NodeId, Reg, Tree, ROOT};
use std::collections::HashMap;
use std::fs::File;

/// Parameters of the starting image used for the new one (R-X-3).
#[derive(Debug, Clone)]
pub struct ImageParams {
    pub bs: u32,
    pub blocks: u64,
    pub inodes: u64,
    pub reserved: u64,
    pub label: [u8; 16],
    pub creator_os: u32,
    pub uuid: [u8; 16],
}

struct Img {
    f: File,
    name: String,
    bs: u64,
    ipg: u64,
    inode_size: u64,
    inodes: u64,
    blocks: u64,
    tables: Vec<u64>,
}

struct RawInode {
    mode: u16,
    uid: u32,
    gid: u32,
    size: u64,
    mtime: i64,
    blocks512: u32,
    block: [u32; 15],
    file_acl: u32,
}

impl Img {
    fn read(&self, off: u64, buf: &mut [u8]) -> Result<()> {
        let mut done = 0;
        while done < buf.len() {
            match crate::host::read_at(&self.f, &mut buf[done..], off + done as u64) {
                Ok(0) => return Err(Error::fatal(format!("{}: truncated image", self.name))),
                Ok(n) => done += n,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) => return Err(io_err(&self.name, e)),
            }
        }
        Ok(())
    }
    fn block(&self, b: u64) -> Result<Vec<u8>> {
        if b == 0 || b >= self.blocks {
            return Err(Error::fatal(format!("{}: block number {b} out of range", self.name)));
        }
        let mut v = vec![0u8; self.bs as usize];
        self.read(b * self.bs, &mut v)?;
        Ok(v)
    }
    fn inode(&self, n: u64) -> Result<RawInode> {
        if n == 0 || n > self.inodes {
            return Err(Error::fatal(format!("{}: inode number {n} out of range", self.name)));
        }
        let g = ((n - 1) / self.ipg) as usize;
        let j = (n - 1) % self.ipg;
        let mut b = [0u8; 128];
        self.read(self.tables[g] * self.bs + j * self.inode_size, &mut b)?;
        let flags = get32(&b, 32);
        if flags & 0x0008_0000 != 0 {
            return Err(Error::fatal(format!("{}: inode {n} uses extents (not ext2)", self.name)));
        }
        let mode = get16(&b, 0);
        let mut size = get32(&b, 4) as u64;
        if mode & 0xF000 == 0x8000 {
            size |= (get32(&b, 108) as u64) << 32;
        }
        let mut block = [0u32; 15];
        for (i, v) in block.iter_mut().enumerate() {
            *v = get32(&b, 40 + 4 * i);
        }
        Ok(RawInode {
            mode,
            uid: get16(&b, 2) as u32 | (get16(&b, 120) as u32) << 16,
            gid: get16(&b, 24) as u32 | (get16(&b, 122) as u32) << 16,
            size,
            mtime: get32(&b, 16) as i32 as i64,
            blocks512: get32(&b, 28),
            block,
            file_acl: get32(&b, 104),
        })
    }

    /// Mapped (logical, physical, count) runs for the first `nblocks` blocks.
    fn block_map(&self, ino: &RawInode, nblocks: u64) -> Result<Vec<(u64, u64, u64)>> {
        let p = self.bs / 4;
        let mut runs: Vec<(u64, u64, u64)> = Vec::new();
        let mut push = |l: u64, b: u64| {
            if let Some(r) = runs.last_mut() {
                if r.0 + r.2 == l && r.1 + r.2 == b {
                    r.2 += 1;
                    return;
                }
            }
            runs.push((l, b, 1));
        };
        for l in 0..nblocks.min(12) {
            if ino.block[l as usize] != 0 {
                push(l, ino.block[l as usize] as u64);
            }
        }
        // (root block, level, first logical)
        let tops = [(ino.block[12], 1u32, 12u64), (ino.block[13], 2, 12 + p), (ino.block[14], 3, 12 + p + p * p)];
        for (root, level, first) in tops {
            if root == 0 || first >= nblocks {
                continue;
            }
            self.walk_ind(root as u64, level, first, nblocks, p, &mut push)?;
        }
        Ok(runs)
    }

    fn walk_ind(
        &self,
        b: u64,
        level: u32,
        first: u64,
        nblocks: u64,
        p: u64,
        push: &mut dyn FnMut(u64, u64),
    ) -> Result<()> {
        let data = self.block(b)?;
        let span = p.pow(level - 1);
        for i in 0..p {
            let l = first + i * span;
            if l >= nblocks {
                break;
            }
            let c = get32(&data, (i * 4) as usize) as u64;
            if c == 0 {
                continue;
            }
            if level == 1 {
                if c >= self.blocks {
                    return Err(Error::fatal(format!("{}: block number {c} out of range", self.name)));
                }
                push(l, c);
            } else {
                self.walk_ind(c, level - 1, l, nblocks, p, push)?;
            }
        }
        Ok(())
    }

    fn read_data(&self, map: &[(u64, u64, u64)], size: u64) -> Result<Vec<u8>> {
        let mut v = vec![0u8; size as usize];
        for &(l, b, c) in map {
            let s = l * self.bs;
            let e = ((l + c) * self.bs).min(size);
            if s < e {
                self.read(b * self.bs, &mut v[s as usize..e as usize])?;
            }
        }
        Ok(v)
    }
}

/// Load the tree of a starting image into `tree` (its root becomes the root).
pub fn load(tree: &mut Tree, path: &std::ffi::OsStr, holes: bool) -> Result<ImageParams> {
    let name = path.to_string_lossy().into_owned();
    let f = File::open(path).map_err(|e| io_err(&name, e))?;
    let mut sb = [0u8; 1024];
    let tmp = Img { f, name: name.clone(), bs: 1024, ipg: 1, inode_size: 128, inodes: 0, blocks: 0, tables: vec![] };
    tmp.read(1024, &mut sb)?;
    let bad = |m: &str| Error::fatal(format!("{name}: not a supported ext2 image: {m}"));
    if get16(&sb, 0x38) != 0xEF53 {
        return Err(bad("bad magic"));
    }
    let rev = get32(&sb, 0x4C);
    if rev > 1 {
        return Err(bad("unsupported revision"));
    }
    let log = get32(&sb, 0x18);
    if log > 2 {
        return Err(bad("unsupported block size"));
    }
    let bs = 1024u64 << log;
    let inode_size = if rev == 0 { 128 } else { get16(&sb, 0x58) as u64 };
    if inode_size < 128 || inode_size > bs || !inode_size.is_power_of_two() {
        return Err(bad("bad inode size"));
    }
    let (compat, incompat, ro) =
        if rev == 0 { (0, 0, 0) } else { (get32(&sb, 0x5C), get32(&sb, 0x60), get32(&sb, 0x64)) };
    if compat & !0x0008 != 0 || incompat & !0x0002 != 0 || ro & !0x0003 != 0 {
        return Err(bad("unsupported features"));
    }
    let blocks = get32(&sb, 0x04) as u64;
    let inodes = get32(&sb, 0x00) as u64;
    let fdb = get32(&sb, 0x14) as u64;
    let bpg = get32(&sb, 0x20) as u64;
    let ipg = get32(&sb, 0x28) as u64;
    if bpg == 0 || ipg == 0 || blocks <= fdb {
        return Err(bad("bad geometry"));
    }
    let groups = (blocks - fdb).div_ceil(bpg);
    if groups * ipg < inodes || inodes < 11 {
        return Err(bad("bad inode count"));
    }
    let mut gdt = vec![0u8; (groups * 32) as usize];
    tmp.read((fdb + 1) * bs, &mut gdt)?;
    let tables = (0..groups as usize).map(|g| get32(&gdt, g * 32 + 8) as u64).collect();
    let img = Img { f: tmp.f, name: name.clone(), bs, ipg, inode_size, inodes, blocks, tables };
    let mut label = [0u8; 16];
    label.copy_from_slice(&sb[0x78..0x88]);
    let mut uuid = [0u8; 16];
    uuid.copy_from_slice(&sb[0x68..0x78]);
    let params = ImageParams {
        bs: bs as u32,
        blocks,
        inodes,
        reserved: get32(&sb, 0x08) as u64,
        label,
        creator_os: get32(&sb, 0x48),
        uuid,
    };
    let src_file = img.f.try_clone().map_err(|e| io_err(&name, e))?;
    let src = tree.add_source(src_file, name.clone());

    // walk the tree from inode 2
    let root = img.inode(2)?;
    if root.mode & 0xF000 != 0x4000 {
        return Err(bad("root is not a directory"));
    }
    {
        let r = tree.node_mut(ROOT);
        r.perm = root.mode & 0o7777;
        r.uid = root.uid;
        r.gid = root.gid;
        r.implicit = false;
    }
    let mut seen: HashMap<u64, NodeId> = HashMap::new();
    let mut dirs_seen: HashMap<u64, ()> = HashMap::new();
    dirs_seen.insert(2, ());
    let mut stack: Vec<(u64, RawInode, NodeId)> = vec![(2, root, ROOT)];
    while let Some((_dino, d, did)) = stack.pop() {
        let nblocks = d.size.div_ceil(bs);
        let map = img.block_map(&d, nblocks)?;
        let data = img.read_data(&map, nblocks * bs)?;
        let mut pos = 0usize;
        while pos + 8 <= data.len() {
            let ino = get32(&data, pos) as u64;
            let rec = get16(&data, pos + 4) as usize;
            let nlen = data[pos + 6] as usize;
            if rec < 8 || pos + rec > data.len() || 8 + nlen > rec {
                return Err(bad("corrupt directory"));
            }
            let nm = &data[pos + 8..pos + 8 + nlen];
            pos += rec;
            if ino == 0 || nm == b"." || nm == b".." {
                continue;
            }
            if let Some(&existing) = seen.get(&ino) {
                tree.link_child(did, nm, existing);
                continue;
            }
            let x = img.inode(ino)?;
            if x.file_acl != 0 {
                return Err(bad("extended attribute blocks are in use"));
            }
            let perm = x.mode & 0o7777;
            let kind = match x.mode & 0xF000 {
                0x4000 => {
                    if dirs_seen.insert(ino, ()).is_some() {
                        return Err(bad("directory hard link"));
                    }
                    Kind::Dir(Box::default())
                }
                0x8000 => {
                    let nb = x.size.div_ceil(bs);
                    let map = img.block_map(&x, nb)?;
                    let zmap = if holes {
                        let mut z = ZeroMapBuilder::new(bs as u32);
                        let mut buf = vec![0u8; bs as usize];
                        for &(l, b, c) in &map {
                            for k in 0..c {
                                img.read((b + k) * bs, &mut buf)?;
                                let lim = (x.size - (l + k) * bs).min(bs) as usize;
                                z.feed((l + k) * bs, &buf[..lim]);
                            }
                        }
                        Some(z.finish())
                    } else {
                        None
                    };
                    Kind::Reg(Box::new(Reg {
                        size: x.size,
                        data: Data::Image { src, bs: bs as u32, map: map.into() },
                        zmap: zmap.map(Box::new),
                    }))
                }
                0xA000 => {
                    let fast = x.blocks512 == 0;
                    let t = if fast {
                        let mut raw = Vec::with_capacity(60);
                        for v in &x.block {
                            raw.extend_from_slice(&v.to_le_bytes());
                        }
                        raw.truncate(x.size.min(60) as usize);
                        raw
                    } else {
                        let map = img.block_map(&x, 1)?;
                        img.read_data(&map, x.size.min(bs))?
                    };
                    Kind::Symlink(t.into())
                }
                0x2000 | 0x6000 => {
                    let (ma, mi) = decode_dev(x.block[0], x.block[1]);
                    if x.mode & 0xF000 == 0x2000 {
                        Kind::Chr(ma, mi)
                    } else {
                        Kind::Blk(ma, mi)
                    }
                }
                0x1000 => Kind::Fifo,
                0xC000 => Kind::Sock,
                _ => return Err(bad(&format!("unknown file type in inode {ino} ({})", show(nm)))),
            };
            let is_dir = kind.is_dir();
            let node = Node::new(kind, perm, x.uid, x.gid, x.mtime);
            let id = tree.push(node);
            tree.link_child(did, nm, id);
            if is_dir {
                stack.push((ino, x, id));
            } else {
                seen.insert(ino, id);
            }
        }
    }
    Ok(params)
}

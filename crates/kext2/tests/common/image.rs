//! An ext2 image reader for the tests: walks the tree from the root inode
//! and returns every entry with its content.

use super::E;
use kext2::error::{io_err, show, Error, Result};
use kext2::format::{decode_dev, get16, get32};
use std::collections::{BTreeMap, HashMap};
use std::fs::File;

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
            match kext2::host::read_at(&self.f, &mut buf[done..], off + done as u64) {
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

/// Read every entry of the image at `path`, by path (`/a/b`).
pub fn read(path: &std::ffi::OsStr) -> Result<BTreeMap<String, E>> {
    let name = path.to_string_lossy().into_owned();
    let f = File::open(path).map_err(|e| io_err(&name, e))?;
    let mut sb = [0u8; 1024];
    let tmp = Img { f, name: name.clone(), bs: 1024, ipg: 1, inode_size: 128, inodes: 0, blocks: 0, tables: vec![] };
    tmp.read(1024, &mut sb)?;
    let bad = |m: &str| Error::fatal(format!("{name}: not a supported ext2 image: {m}"));
    if get16(&sb, 0x38) != 0xEF53 {
        return Err(bad("bad magic"));
    }
    let log = get32(&sb, 0x18);
    if log > 2 {
        return Err(bad("unsupported block size"));
    }
    let bs = 1024u64 << log;
    let inode_size = get16(&sb, 0x58) as u64;
    let blocks = get32(&sb, 0x04) as u64;
    let inodes = get32(&sb, 0x00) as u64;
    let fdb = get32(&sb, 0x14) as u64;
    let bpg = get32(&sb, 0x20) as u64;
    let ipg = get32(&sb, 0x28) as u64;
    if bpg == 0 || ipg == 0 || blocks <= fdb {
        return Err(bad("bad geometry"));
    }
    let groups = (blocks - fdb).div_ceil(bpg);
    let mut gdt = vec![0u8; (groups * 32) as usize];
    tmp.read((fdb + 1) * bs, &mut gdt)?;
    let tables = (0..groups as usize).map(|g| get32(&gdt, g * 32 + 8) as u64).collect();
    let img = Img { f: tmp.f, name: name.clone(), bs, ipg, inode_size, inodes, blocks, tables };

    let root = img.inode(2)?;
    if root.mode & 0xF000 != 0x4000 {
        return Err(bad("root is not a directory"));
    }
    let mut out = BTreeMap::new();
    let mut dirs_seen: HashMap<u64, ()> = HashMap::new();
    dirs_seen.insert(2, ());
    let mut stack: Vec<(String, RawInode)> = vec![(String::new(), root)];
    while let Some((dpath, d)) = stack.pop() {
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
            let nm = data[pos + 8..pos + 8 + nlen].to_vec();
            pos += rec;
            if ino == 0 || nm == b"." || nm == b".." {
                continue;
            }
            let x = img.inode(ino)?;
            let path = format!("{dpath}/{}", String::from_utf8_lossy(&nm));
            let mut e = E {
                kind: '?',
                perm: x.mode & 0o7777,
                uid: x.uid,
                gid: x.gid,
                mtime: x.mtime,
                data: vec![],
                target: vec![],
                dev: (0, 0),
                ino,
                map: vec![],
            };
            match x.mode & 0xF000 {
                0x4000 => {
                    if dirs_seen.insert(ino, ()).is_some() {
                        return Err(bad("directory hard link"));
                    }
                    e.kind = 'd';
                    stack.push((path.clone(), x));
                }
                0x8000 => {
                    e.kind = 'f';
                    e.map = img.block_map(&x, x.size.div_ceil(bs))?;
                    if x.size <= super::MAX_LOADED {
                        e.data = img.read_data(&e.map, x.size)?;
                    }
                }
                0xA000 => {
                    e.kind = 'l';
                    e.target = if x.blocks512 == 0 {
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
                }
                0x2000 | 0x6000 => {
                    e.kind = if x.mode & 0xF000 == 0x2000 { 'c' } else { 'b' };
                    e.dev = decode_dev(x.block[0], x.block[1]);
                }
                0x1000 => e.kind = 'p',
                0xC000 => e.kind = 's',
                _ => return Err(bad(&format!("unknown file type in inode {ino} ({})", show(&nm)))),
            }
            out.insert(path, e);
        }
    }
    Ok(out)
}

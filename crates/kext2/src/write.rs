//! The write phase (SPEC §10): renders the image block range by block range
//! from the plan and writes it to a file (temporary file + rename), a block
//! device, or a stream (standard output, pipes).
//!
//! The image is cut into chunks of consecutive blocks. Every chunk is a pure
//! function of the tree, the plan and the input data, so chunks can be
//! rendered by several threads; the bytes do not depend on the thread count
//! (R-REPRO-4).

use crate::cli::TimeMode;
use crate::dirent::{self, Entry};
use crate::error::{io_err, Error, Result};
use crate::format::{self, GroupDesc, Inode, Super};
use crate::plan::Plan;
use crate::runs::is_zero_block;
use crate::tree::{Data, Kind, NodeId, Tree, NONE, ROOT};
use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Condvar, Mutex};

/// Image-wide settings for rendering.
pub struct Settings {
    pub t_fs: i64,
    pub time: TimeMode,
    pub fill: u8,
    pub creator_os: u32,
    pub label: [u8; 16],
    pub uuid: [u8; 16],
}

/// Final inode time of a node (R-TIME-3, R-TIME-4).
pub fn inode_time(s: &Settings, id: NodeId, mtime: i64) -> u32 {
    let t = if id == ROOT { s.t_fs } else { mtime };
    let t = match s.time {
        TimeMode::Fake => 0,
        TimeMode::Fixed(v) => v,
        TimeMode::Clamp(v) => t.min(v),
        TimeMode::Default => t,
    };
    t.clamp(i32::MIN as i64, i32::MAX as i64) as i32 as u32
}

pub struct Renderer<'a> {
    pub tree: &'a Tree,
    pub plan: &'a Plan,
    pub s: &'a Settings,
    sb: Super,
    gdt: Vec<u8>,
}

/// Per-thread cache of the last opened host file.
#[derive(Default)]
pub struct Cache {
    host: Option<(NodeId, File)>,
}

/// Byte ranges of a chunk that may be non-zero.
#[derive(Default)]
pub struct Marks(pub Vec<(usize, usize)>);

impl Marks {
    fn add(&mut self, off: usize, len: usize) {
        if len == 0 {
            return;
        }
        if let Some(l) = self.0.last_mut() {
            if l.0 + l.1 == off {
                l.1 += len;
                return;
            }
        }
        self.0.push((off, len));
    }
}

fn set_bits(b: &mut [u8], from: u64, to: u64) {
    let (mut i, to) = (from, to);
    while i < to && i % 8 != 0 {
        b[(i / 8) as usize] |= 1 << (i % 8);
        i += 1;
    }
    let full = (to - i) / 8;
    if full > 0 {
        b[(i / 8) as usize..(i / 8 + full) as usize].fill(0xFF);
        i += full * 8;
    }
    while i < to {
        b[(i / 8) as usize] |= 1 << (i % 8);
        i += 1;
    }
}

impl<'a> Renderer<'a> {
    pub fn new(tree: &'a Tree, plan: &'a Plan, s: &'a Settings) -> Renderer<'a> {
        let geo = &plan.geo;
        let free_blocks: u64 = plan.free_blocks.iter().sum();
        let free_inodes: u64 = plan.used_inodes.iter().map(|&u| geo.ipg - u as u64).sum();
        let mut ro = format::FEATURE_RO_COMPAT_SPARSE_SUPER;
        if plan.large_file {
            ro |= format::FEATURE_RO_COMPAT_LARGE_FILE;
        }
        let sb = Super {
            inodes_count: geo.inodes_count() as u32,
            blocks_count: geo.blocks as u32,
            r_blocks_count: plan.reserved as u32,
            free_blocks_count: free_blocks as u32,
            free_inodes_count: free_inodes as u32,
            first_data_block: geo.fdb as u32,
            log_block_size: geo.bs.trailing_zeros() - 10,
            blocks_per_group: geo.bpg as u32,
            inodes_per_group: geo.ipg as u32,
            wtime: s.t_fs as u32,
            creator_os: s.creator_os,
            feature_incompat: if plan.filetype { format::FEATURE_INCOMPAT_FILETYPE } else { 0 },
            feature_ro_compat: ro,
            volume_name: s.label,
            uuid: s.uuid,
        };
        let bs = geo.bs as usize;
        let mut gdt = vec![0u8; geo.gdb as usize * bs];
        for g in 0..geo.groups {
            let d = GroupDesc {
                block_bitmap: geo.block_bitmap(g) as u32,
                inode_bitmap: geo.inode_bitmap(g) as u32,
                inode_table: geo.inode_table(g) as u32,
                free_blocks: plan.free_blocks[g as usize] as u16,
                free_inodes: (geo.ipg - plan.used_inodes[g as usize] as u64) as u16,
                used_dirs: plan.used_dirs[g as usize] as u16,
            };
            let o = g as usize * 32;
            d.write(&mut gdt[o..o + 32]);
        }
        Renderer { tree, plan, s, sb, gdt }
    }

    pub fn superblock(&self) -> &Super {
        &self.sb
    }

    /// Estimated number of bytes of the image that hold data (metadata,
    /// directories, indirect blocks, file bytes actually present).
    pub fn estimated_payload(&self) -> u64 {
        let geo = &self.plan.geo;
        let bs = geo.bs as u64;
        // metadata: superblock/GDT copies, bitmaps, used inode-table blocks
        let mut blocks: u64 = (0..geo.groups)
            .map(|g| geo.overhead(g) - geo.itb + (self.plan.used_inodes[g as usize] as u64).div_ceil(geo.ipb()))
            .sum();
        let mut bytes = 0u64;
        for (id, n) in self.tree.nodes.iter().enumerate() {
            if self.plan.ino[id] == 0 {
                continue;
            }
            let nb = self.plan.nb[id];
            match &n.kind {
                Kind::Reg(r) => {
                    let data_blocks = match &r.zmap {
                        Some(z) => z.count(),
                        None => r.size.div_ceil(bs),
                    };
                    blocks += nb - data_blocks; // indirect blocks
                    bytes += match &r.data {
                        Data::Zero => 0,
                        Data::Host { .. } => r.size,
                        Data::Source { regions, .. } => regions.iter().map(|g| g.len).sum(),
                        Data::Image { map, .. } => map.iter().map(|m| m.2 * bs).sum(),
                    };
                }
                _ => blocks += nb,
            }
        }
        blocks * bs + bytes
    }

    /// Render blocks `[b0, b0 + n)` into `buf` (zeroed by the caller).
    pub fn render(&self, b0: u64, n: u64, buf: &mut [u8], marks: &mut Marks, cache: &mut Cache) -> Result<()> {
        let geo = &self.plan.geo;
        let end = b0 + n;
        let mut b = b0;
        if b < geo.fdb {
            b = geo.fdb; // boot block: zero
        }
        while b < end {
            let g = (b - geo.fdb) / geo.bpg;
            let gend = geo.start(g) + geo.size(g);
            let e = end.min(gend);
            self.render_group(g, b, e, b0, buf, marks, cache)?;
            b = e;
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn render_group(
        &self,
        g: u64,
        from: u64,
        to: u64,
        b0: u64,
        buf: &mut [u8],
        marks: &mut Marks,
        cache: &mut Cache,
    ) -> Result<()> {
        let geo = &self.plan.geo;
        let bs = geo.bs as usize;
        let off = |blk: u64| ((blk - b0) as usize) * bs;
        let gs = geo.start(g);
        let mut blk = gs;
        let inr = |x: u64| x >= from && x < to;
        if geo.has_super(g) {
            if inr(gs) {
                let sbb = self.sb.to_bytes(g as u16);
                let o = off(gs) + if g == 0 && geo.fdb == 0 { 1024 } else { 0 };
                buf[o..o + 1024].copy_from_slice(&sbb);
                marks.add(o, 1024);
            }
            for k in 0..geo.gdb {
                let x = gs + 1 + k;
                if inr(x) {
                    let o = off(x);
                    buf[o..o + bs].copy_from_slice(&self.gdt[k as usize * bs..(k as usize + 1) * bs]);
                    marks.add(o, bs);
                }
            }
            blk = gs + 1 + geo.gdb;
        }
        let bb = blk;
        if inr(bb) {
            let o = off(bb);
            self.block_bitmap(g, &mut buf[o..o + bs]);
            marks.add(o, bs);
        }
        if inr(bb + 1) {
            let o = off(bb + 1);
            let out = &mut buf[o..o + bs];
            set_bits(out, 0, self.plan.used_inodes[g as usize] as u64);
            set_bits(out, geo.ipg, 8 * bs as u64);
            marks.add(o, bs);
        }
        let it = bb + 2;
        let ds = it + geo.itb;
        let lo = from.max(it);
        let hi = to.min(ds);
        for x in lo..hi.max(lo) {
            let o = off(x);
            if self.inode_block(g, x - it, &mut buf[o..o + bs]) {
                marks.add(o, bs);
            }
        }
        // data area
        let lo = from.max(ds);
        if lo < to {
            let dfirst = self.plan.map.first_index(g);
            let d0 = dfirst + (lo - ds);
            let d1 = dfirst + (to - ds);
            let o0 = off(lo);
            self.render_data(d0, d1, &mut buf[o0..o0 + ((d1 - d0) as usize) * bs], marks, o0, cache)?;
        }
        Ok(())
    }

    fn block_bitmap(&self, g: u64, out: &mut [u8]) {
        let geo = &self.plan.geo;
        let ov = geo.overhead(g);
        set_bits(out, 0, ov);
        let lo = self.plan.map.first_index(g);
        let hi = lo + geo.data_blocks(g);
        let alloc = &self.plan.alloc;
        let mut i = alloc.partition_point(|r| r.0 + r.1 <= lo);
        while i < alloc.len() && alloc[i].0 < hi {
            let s = alloc[i].0.max(lo);
            let e = (alloc[i].0 + alloc[i].1).min(hi);
            set_bits(out, ov + (s - lo), ov + (e - lo));
            i += 1;
        }
        set_bits(out, geo.size(g), 8 * geo.bs as u64);
    }

    /// Inode-table block `k` of group `g`. Returns false if it is all zero.
    fn inode_block(&self, g: u64, k: u64, out: &mut [u8]) -> bool {
        let geo = &self.plan.geo;
        let ipb = geo.ipb();
        let used = self.plan.used_inodes[g as usize] as u64;
        let first = k * ipb;
        if first >= used {
            return false;
        }
        let base = self.plan.slot_base[g as usize];
        let mut any = false;
        for j in first..(first + ipb).min(used) {
            let id = self.plan.slots[(base + j) as usize];
            if id == NONE {
                continue;
            }
            let o = ((j - first) as usize) * format::INODE_SIZE;
            self.inode(id).write(&mut out[o..o + format::INODE_SIZE]);
            any = true;
        }
        any
    }

    /// The inode of node `id`.
    pub fn inode(&self, id: NodeId) -> Inode {
        let node = self.tree.node(id);
        let plan = self.plan;
        let bs = plan.geo.bs as u64;
        let nb = plan.nb[id as usize];
        let mut ino = Inode {
            mode: node.kind.mode_bits() | node.perm,
            uid: node.uid,
            gid: node.gid,
            size: 0,
            time: inode_time(self.s, id, node.mtime),
            links: plan.nlink[id as usize] as u16,
            blocks512: (nb * (bs / 512)) as u32,
            block: plan.iblock(id),
            fast_link: None,
            regular: false,
        };
        match &node.kind {
            Kind::Reg(r) => {
                ino.size = r.size;
                ino.regular = true;
            }
            Kind::Dir(_) => {
                ino.size = plan.dirs[id as usize].as_ref().unwrap().nblocks * bs;
            }
            Kind::Symlink(t) => {
                ino.size = t.len() as u64;
                if nb == 0 {
                    ino.fast_link = Some(t.to_vec());
                }
            }
            Kind::Chr(ma, mi) | Kind::Blk(ma, mi) => {
                let d = format::encode_dev(*ma, *mi);
                ino.block = [0; 15];
                ino.block[0] = d[0];
                ino.block[1] = d[1];
            }
            Kind::Fifo | Kind::Sock => ino.block = [0; 15],
        }
        ino
    }

    /// Render data indices `[d0, d1)` (contiguous blocks) into `out`.
    fn render_data(
        &self,
        d0: u64,
        d1: u64,
        out: &mut [u8],
        marks: &mut Marks,
        mark_base: usize,
        cache: &mut Cache,
    ) -> Result<()> {
        let bs = self.plan.geo.bs as usize;
        let ex = &self.plan.extents;
        let mut i = ex.partition_point(|e| e.start + e.len as u64 <= d0);
        let mut pos = d0;
        let fill = self.s.fill;
        let gap = |from: u64, to: u64, out: &mut [u8], marks: &mut Marks| {
            if fill != 0 && to > from {
                let o = ((from - d0) as usize) * bs;
                let l = ((to - from) as usize) * bs;
                out[o..o + l].fill(fill);
                marks.add(mark_base + o, l);
            }
        };
        while pos < d1 {
            if i >= ex.len() || ex[i].start >= d1 {
                gap(pos, d1, out, marks);
                break;
            }
            let e = ex[i];
            if e.start > pos {
                gap(pos, e.start, out, marks);
                pos = e.start;
            }
            let s = pos.max(e.start);
            let t = d1.min(e.start + e.len as u64);
            let o = ((s - d0) as usize) * bs;
            let l = ((t - s) as usize) * bs;
            let dst = &mut out[o..o + l];
            if e.level > 0 {
                self.indirect(i, dst);
            } else {
                let node = self.tree.node(e.node);
                let lfirst = e.lstart as u64 + (s - e.start);
                match &node.kind {
                    Kind::Reg(_) => {
                        // only the ranges actually read are marked: zero
                        // holes of the source stay holes in a sparse output
                        self.file_data(e.node, lfirst, t - s, dst, cache, marks, mark_base + o)?;
                        pos = t;
                        i += 1;
                        continue;
                    }
                    Kind::Dir(_) => {
                        for (k, blk) in dst.chunks_exact_mut(bs).enumerate() {
                            self.dir_block(e.node, lfirst + k as u64, blk);
                        }
                    }
                    Kind::Symlink(tg) => dst[..tg.len()].copy_from_slice(tg),
                    _ => {}
                }
            }
            marks.add(mark_base + o, l);
            pos = t;
            i += 1;
        }
        Ok(())
    }

    fn indirect(&self, ei: usize, out: &mut [u8]) {
        let ex = &self.plan.extents;
        let e = ex[ei];
        let p = self.plan.tiers.p;
        let map = &self.plan.map;
        let mut f = ei + 1;
        while f < ex.len() {
            let x = ex[f];
            if x.node != e.node {
                break;
            }
            match e.level {
                1 => {
                    if x.level != 0 {
                        break;
                    }
                    let mut idx = (x.lstart - e.lstart) as usize;
                    map.for_runs(x.start, x.len as u64, |b, n| {
                        for k in 0..n {
                            format::put32(out, idx * 4, (b + k) as u32);
                            idx += 1;
                        }
                    });
                }
                2 => {
                    if x.level >= 2 {
                        break;
                    }
                    if x.level == 1 {
                        let idx = ((x.lstart - e.lstart) as u64 / p) as usize;
                        format::put32(out, idx * 4, map.block(x.start) as u32);
                    }
                }
                _ => {
                    if x.level == 3 {
                        break;
                    }
                    if x.level == 2 {
                        let idx = ((x.lstart - e.lstart) as u64 / (p * p)) as usize;
                        format::put32(out, idx * 4, map.block(x.start) as u32);
                    }
                }
            }
            f += 1;
        }
    }

    fn dir_block(&self, d: NodeId, k: u64, out: &mut [u8]) {
        let dp = self.plan.dirs[d as usize].as_ref().unwrap();
        let ft = self.plan.filetype;
        if k as usize >= dp.starts.len() {
            dirent::write_block(out, std::iter::empty(), ft);
            return;
        }
        let children = self.tree.children(d);
        let first = dp.starts[k as usize] as usize;
        let last = dp.starts.get(k as usize + 1).map(|&x| x as usize).unwrap_or(children.len() + 2);
        let ino = &self.plan.ino;
        let tree = self.tree;
        let entries = (first..last).map(|i| match i {
            0 => Entry { ino: ino[d as usize], name: b".", file_type: 2 },
            1 => Entry { ino: ino[dp.parent as usize], name: b"..", file_type: 2 },
            _ => {
                let (name, c) = &children[i - 2];
                Entry { ino: ino[*c as usize], name, file_type: tree.node(*c).kind.file_type() }
            }
        });
        dirent::write_block(out, entries, ft);
    }

    /// Read logical blocks `[l0, l0+n)` of regular file `id` into `out`.
    #[allow(clippy::too_many_arguments)]
    fn file_data(
        &self,
        id: NodeId,
        l0: u64,
        n: u64,
        out: &mut [u8],
        cache: &mut Cache,
        marks: &mut Marks,
        mark_off: usize,
    ) -> Result<()> {
        let node = self.tree.node(id);
        let r = match &node.kind {
            Kind::Reg(r) => r,
            _ => unreachable!(),
        };
        let bs = self.plan.geo.bs as u64;
        let off = l0 * bs;
        let len = ((l0 + n) * bs).min(r.size).saturating_sub(off) as usize;
        let dst = &mut out[..len];
        // mark [s, e) of dst, widened to whole blocks
        let mut mark = |s: u64, e: u64| {
            let s = s / bs * bs;
            let e = e.div_ceil(bs) * bs;
            marks.add(mark_off + s as usize, (e - s) as usize);
        };
        match &r.data {
            Data::Zero => {}
            Data::Host { dir, name } => {
                let path = &self.tree.host_path(*dir, name);
                let reuse = matches!(&cache.host, Some((cid, _)) if *cid == id);
                if !reuse {
                    let f = File::open(path).map_err(|e| io_err(path.display(), e))?;
                    let md = f.metadata().map_err(|e| io_err(path.display(), e))?;
                    if !md.is_file() || md.len() != r.size {
                        return Err(Error::fatal(format!(
                            "{}: file changed since it was scanned (size {} -> {})",
                            path.display(),
                            r.size,
                            md.len()
                        )));
                    }
                    cache.host = Some((id, f));
                }
                let f = &cache.host.as_ref().unwrap().1;
                mark(0, len as u64);
                read_exact_at(f, dst, off).map_err(|e| match e.kind() {
                    std::io::ErrorKind::UnexpectedEof => {
                        Error::fatal(format!("{}: file shrank since it was scanned", path.display()))
                    }
                    _ => io_err(path.display(), e),
                })?;
            }
            Data::Source { src, regions } => {
                let f = &self.tree.sources[*src as usize];
                let end = off + len as u64;
                let mut i = regions.partition_point(|g| g.off + g.len <= off);
                while i < regions.len() && regions[i].off < end {
                    let g = regions[i];
                    let s = g.off.max(off);
                    let e = (g.off + g.len).min(end);
                    let d = &mut dst[(s - off) as usize..(e - off) as usize];
                    read_exact_at(&f.file, d, g.src_off + (s - g.off)).map_err(|e| io_err(&f.name, e))?;
                    mark(s - off, e - off);
                    i += 1;
                }
            }
            Data::Image { src, bs: ibs, map } => {
                let f = &self.tree.sources[*src as usize];
                let ibs = *ibs as u64;
                let end = off + len as u64;
                let (lb0, lb1) = (off / ibs, end.div_ceil(ibs));
                let mut i = map.partition_point(|m| m.0 + m.2 <= lb0);
                while i < map.len() && map[i].0 < lb1 {
                    let (ml, mp, mc) = map[i];
                    let s = (ml * ibs).max(off);
                    let e = ((ml + mc) * ibs).min(end);
                    if s < e {
                        let d = &mut dst[(s - off) as usize..(e - off) as usize];
                        read_exact_at(&f.file, d, mp * ibs + (s - ml * ibs)).map_err(|e| io_err(&f.name, e))?;
                        mark(s - off, e - off);
                    }
                    i += 1;
                }
            }
        }
        if r.zmap.is_some() {
            for (k, blk) in out.chunks_exact(bs as usize).enumerate() {
                if is_zero_block(blk) {
                    return Err(Error::fatal(format!(
                        "file data changed since it was scanned (block {} became zero)",
                        l0 + k as u64
                    )));
                }
            }
        }
        Ok(())
    }
}

fn read_exact_at(f: &File, mut buf: &mut [u8], mut off: u64) -> std::io::Result<()> {
    while !buf.is_empty() {
        match crate::host::read_at(f, buf, off) {
            Ok(0) => return Err(std::io::ErrorKind::UnexpectedEof.into()),
            Ok(n) => {
                buf = &mut buf[n..];
                off += n as u64;
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

// ------------------------------------------------------------------ output

/// Merge byte ranges separated by gaps of at most `gap` bytes (the gaps are
/// zero in the buffer): fewer, larger writes, fewer tiny holes.
/// Ranges are also widened to 4 KiB boundaries (within `len`): host
/// filesystems handle partial-page holes badly.
fn coalesce(m: &[(usize, usize)], gap: usize, len: usize) -> Vec<(usize, usize)> {
    const A: usize = 4096;
    let mut out: Vec<(usize, usize)> = Vec::with_capacity(m.len());
    for &(o, l) in m {
        let s = o / A * A;
        let e = ((o + l).div_ceil(A) * A).min(len);
        let (o, l) = (s, e - s);
        if let Some(last) = out.last_mut() {
            let end = last.0 + last.1;
            if o <= end || o - end <= gap {
                last.1 = (o + l).max(end) - last.0;
                continue;
            }
        }
        out.push((o, l));
    }
    out
}

/// Where the image goes.
enum Out {
    /// Regular file: written through a temporary file renamed on success.
    File { tmp: TempGuard },
    /// Block device, written in place, every block explicitly.
    Device(File),
    /// Standard output or another non-seekable target: sequential writes.
    Stream(Box<dyn Write + Send>),
}

struct TempGuard {
    path: PathBuf,
    file: File,
    keep: bool,
}

impl Drop for TempGuard {
    fn drop(&mut self) {
        if !self.keep {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

fn open_temp(dest: &Path) -> Result<TempGuard> {
    let dir = match dest.parent() {
        Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
        _ => PathBuf::from("."),
    };
    let base = dest.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "image".into());
    let existing = std::fs::metadata(dest).ok();
    for k in 0..1000u32 {
        let p = dir.join(format!(".{base}.kext2-{}-{k}.tmp", std::process::id()));
        match crate::host::create_new(&p) {
            Ok(file) => {
                if let Some(md) = &existing {
                    crate::host::copy_permissions(md, &file);
                }
                return Ok(TempGuard { path: p, file, keep: false });
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(io_err(p.display(), e)),
        }
    }
    Err(Error::fatal("cannot create a temporary file next to the image"))
}

fn thread_count() -> usize {
    if let Some(n) = std::env::var("KEXT2_THREADS").ok().and_then(|v| v.parse::<usize>().ok()) {
        return n.clamp(1, 64);
    }
    std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1).clamp(1, 8)
}

/// Write the whole image to `path` (`-` = standard output).
pub fn write_image(r: &Renderer, path: &std::ffi::OsStr) -> Result<()> {
    write_image_threads(r, path, thread_count())
}

/// [`write_image`] with an explicit number of rendering threads (the image
/// bytes do not depend on it).
///
/// Rendering threads fill chunk buffers; this thread writes them in block
/// order (sequential writes are the fastest pattern, and the only one a pipe
/// accepts). Regular files skip the zero parts of every chunk (holes);
/// devices and streams get every byte.
pub fn write_image_threads(r: &Renderer, path: &std::ffi::OsStr, threads: usize) -> Result<()> {
    let geo = &r.plan.geo;
    let bs = geo.bs as u64;
    let total = geo.blocks;
    let mut out = if path == "-" {
        Out::Stream(Box::new(std::io::stdout()))
    } else {
        let p = Path::new(path);
        match std::fs::metadata(p) {
            Ok(m) if crate::host::is_block_device(&m.file_type()) => {
                Out::Device(OpenOptions::new().write(true).open(p).map_err(|e| io_err(p.display(), e))?)
            }
            Ok(m) if m.is_dir() => return Err(Error::fatal(format!("{}: is a directory", p.display()))),
            Ok(m) if !m.is_file() => {
                let f = OpenOptions::new().write(true).open(p).map_err(|e| io_err(p.display(), e))?;
                Out::Stream(Box::new(f))
            }
            _ => Out::File { tmp: open_temp(p)? },
        }
    };
    if let Out::File { tmp } = &out {
        tmp.file.set_len(total * bs).map_err(|e| io_err(tmp.path.display(), e))?;
    }

    // Holes in a regular output file pay off when the image is mostly zero;
    // when little of it is zero, writing the zeros is cheaper than the
    // host filesystem's handling of many holes (observed on APFS).
    let dense = r.estimated_payload() >= total * bs / 8 * 7;
    let chunk_blocks = ((4u64 << 20) / bs).max(1);
    let nchunks = total.div_ceil(chunk_blocks);
    let threads = threads.clamp(1, 64).min(nchunks.max(1) as usize);
    let window = threads + 2;
    let chunk_range = |ci: u64| {
        let b0 = ci * chunk_blocks;
        (b0, chunk_blocks.min(total - b0))
    };

    struct Shared {
        /// Rendered chunks waiting to be written.
        ready: BTreeMap<u64, (Vec<u8>, Marks)>,
        /// Zeroed buffers.
        pool: Vec<Vec<u8>>,
        /// Chunks written so far.
        written: u64,
    }
    let shared = Mutex::new(Shared { ready: BTreeMap::new(), pool: Vec::new(), written: 0 });
    let cv = Condvar::new();
    let next = AtomicU64::new(0);
    let failed = AtomicBool::new(false);
    let err: Mutex<Option<Error>> = Mutex::new(None);
    let fail = |e: Error| {
        failed.store(true, Ordering::SeqCst);
        let mut g = err.lock().unwrap();
        if g.is_none() {
            *g = Some(e);
        }
        drop(g);
        cv.notify_all();
    };
    let buf_len = (chunk_blocks * bs) as usize;

    std::thread::scope(|sc| {
        for _ in 0..threads {
            sc.spawn(|| {
                let mut cache = Cache::default();
                loop {
                    if failed.load(Ordering::SeqCst) {
                        return;
                    }
                    let ci = next.fetch_add(1, Ordering::SeqCst);
                    if ci >= nchunks {
                        return;
                    }
                    let mut buf = {
                        let mut g = shared.lock().unwrap();
                        while ci >= g.written + window as u64 && !failed.load(Ordering::SeqCst) {
                            g = cv.wait(g).unwrap();
                        }
                        if failed.load(Ordering::SeqCst) {
                            return;
                        }
                        g.pool.pop().unwrap_or_else(|| vec![0u8; buf_len])
                    };
                    let (b0, n) = chunk_range(ci);
                    let len = (n * bs) as usize;
                    let mut marks = Marks::default();
                    if let Err(e) = r.render(b0, n, &mut buf[..len], &mut marks, &mut cache) {
                        return fail(e);
                    }
                    shared.lock().unwrap().ready.insert(ci, (buf, marks));
                    cv.notify_all();
                }
            });
        }
        // the writer
        let mut want = 0u64;
        while want < nchunks {
            let item = {
                let mut g = shared.lock().unwrap();
                loop {
                    if failed.load(Ordering::SeqCst) {
                        break None;
                    }
                    if let Some(x) = g.ready.remove(&want) {
                        break Some(x);
                    }
                    g = cv.wait(g).unwrap();
                }
            };
            let Some((mut buf, marks)) = item else { break };
            let (b0, n) = chunk_range(want);
            let len = (n * bs) as usize;
            let off = b0 * bs;
            let res = match &mut out {
                Out::File { tmp } => {
                    let mut r = Ok(());
                    let cm = if dense { vec![(0, len)] } else { coalesce(&marks.0, 64 << 10, len) };
                    for (o, l) in cm {
                        r = crate::host::write_all_at(&tmp.file, &buf[o..o + l], off + o as u64);
                        if r.is_err() {
                            break;
                        }
                    }
                    r.map_err(|e| io_err(tmp.path.display(), e))
                }
                Out::Device(dev) => {
                    crate::host::write_all_at(dev, &buf[..len], off).map_err(|e| io_err("block device", e))
                }
                Out::Stream(w) => w.write_all(&buf[..len]).map_err(|e| io_err("write", e)),
            };
            if let Err(e) = res {
                fail(e);
                break;
            }
            // rendering only writes inside the marks: clearing them makes
            // the buffer zero again
            for &(o, l) in &marks.0 {
                buf[o..o + l].fill(0);
            }
            want += 1;
            let mut g = shared.lock().unwrap();
            g.written = want;
            g.pool.push(buf);
            drop(g);
            cv.notify_all();
        }
        if !failed.load(Ordering::SeqCst) {
            if let Out::Stream(w) = &mut out {
                if let Err(e) = w.flush() {
                    fail(io_err("write", e));
                }
            }
        }
        cv.notify_all();
    });
    if let Some(e) = err.into_inner().unwrap() {
        return Err(e);
    }
    if let Out::File { tmp } = &mut out {
        let dest = Path::new(path);
        std::fs::rename(&tmp.path, dest).map_err(|e| io_err(dest.display(), e))?;
        tmp.keep = true;
    }
    Ok(())
}

//! The plan phase (SPEC §3, §7, §8.10, §9): limits, lost+found, sizing,
//! geometry and the deterministic allocation of every inode and block.

use crate::dirent;
use crate::error::{show, Error, Result};
use crate::geometry::{self, DataMap, Geo, Geometry, BS};
use crate::layout::{self, Seg, Tiers};
use crate::tree::{Kind, NodeId, Tree, NONE, ROOT};

pub const LINK_MAX: u64 = 32000;
pub const MAX_ID: u32 = 4_294_967_294;
/// Data blocks of the generated lost+found (16 KiB, as mke2fs makes it).
pub const LOST_FOUND_BLOCKS: u64 = 4;
/// `--free-space` bytes per extra free inode.
pub const BYTES_PER_FREE_INODE: u64 = 16384;

/// Sizing parameters taken from the options.
#[derive(Debug, Clone, Default)]
pub struct Params {
    /// Free blocks to leave in the image.
    pub free_blocks: u64,
    /// Free inodes to leave in the image (at least; the geometry rounds up).
    pub free_inodes: u64,
}

impl Params {
    /// The sizing for `--free-space bytes`: whole free blocks, and one free
    /// inode per [`BYTES_PER_FREE_INODE`] bytes.
    pub fn from_free_space(bytes: u64) -> Params {
        Params { free_blocks: bytes.div_ceil(BS as u64), free_inodes: bytes.div_ceil(BYTES_PER_FREE_INODE) }
    }
}

/// One run of an inode's layout sequence placed at data indices
/// `[start, start+len)`.
#[derive(Debug, Clone, Copy)]
pub struct Extent {
    pub start: u64,
    /// First logical block (leaf) or the first logical block mapped
    /// (indirect). An ext2 file has fewer than 2^32 logical blocks.
    pub lstart: u32,
    pub node: NodeId,
    pub len: u32,
    /// 0 = data/directory/symlink block(s), 1..3 = indirect level.
    pub level: u8,
}

impl Plan {
    /// `i_block` of node `id` (R-FMT-13): the first 12 data blocks, then the
    /// single, double and triple indirect blocks.
    pub fn iblock(&self, id: NodeId) -> [u32; 15] {
        let mut ib = [0u32; 15];
        let first = self.first_extent[id as usize];
        if first == NONE {
            return ib;
        }
        let t = self.tiers;
        for e in self.extents[first as usize..].iter().take_while(|e| e.node == id) {
            match e.level {
                0 => {
                    let ls = e.lstart as u64;
                    for l in ls..(ls + e.len as u64).min(12) {
                        ib[l as usize] = self.map.block(e.start + (l - ls)) as u32;
                    }
                }
                1 if e.lstart as u64 == t.single => ib[12] = self.map.block(e.start) as u32,
                2 if e.lstart as u64 == t.double => ib[13] = self.map.block(e.start) as u32,
                3 => ib[14] = self.map.block(e.start) as u32,
                _ => {}
            }
        }
        ib
    }
}

/// Directory packing.
#[derive(Debug, Clone)]
pub struct DirPlan {
    /// Index of the first entry (`.` = 0, `..` = 1, children from 2) of each
    /// packed block.
    pub starts: Vec<u32>,
    /// Total data blocks (≥ starts.len(); lost+found has empty blocks).
    pub nblocks: u64,
    pub parent: NodeId,
}

#[derive(Debug)]
pub struct Plan {
    pub geo: Geometry,
    pub map: DataMap,
    pub tiers: Tiers,
    pub large_file: bool,
    /// Per node: inode number (0 = not in the image).
    pub ino: Vec<u32>,
    /// Per node: link count.
    pub nlink: Vec<u32>,
    /// Per node: blocks (data + indirect).
    pub nb: Vec<u64>,
    /// Per node: index of its first extent (`NONE` without blocks); a
    /// node's extents are consecutive. `i_block` is derived from them
    /// ([`Plan::iblock`]).
    pub first_extent: Vec<u32>,
    /// Per node: directory packing.
    pub dirs: Vec<Option<Box<DirPlan>>>,
    /// The generated lost+found, if it is in the image.
    pub lost_found: Option<NodeId>,
    /// Layout of all blocks, sorted by data index.
    pub extents: Vec<Extent>,
    /// Allocated data-index runs (start, len).
    pub alloc: Vec<(u64, u64)>,
    /// Per group: used inode slots (a prefix), directories, free blocks.
    pub used_inodes: Vec<u32>,
    pub used_dirs: Vec<u32>,
    pub free_blocks: Vec<u64>,
    /// Nodes by inode slot: group g's slots start at `slot_base[g]`.
    pub slot_base: Vec<u64>,
    pub slots: Vec<NodeId>,
    /// Totals.
    pub total_blocks_used: u64,
    pub inodes_used: u64,
}

fn validate_name(name: &[u8], parent_path: &dyn Fn() -> String) -> Result<()> {
    if name.is_empty() || name.len() > 255 || name.contains(&b'/') || name.contains(&0) || name == b"." || name == b".."
    {
        return Err(Error::fatal(format!(
            "invalid file name '{}' in {} (names must be 1-255 bytes without '/' or NUL)",
            show(name),
            parent_path()
        )));
    }
    Ok(())
}

/// Path of a node for messages (computed on demand from parent links).
fn path_of(tree: &Tree, parents: &[NodeId], id: NodeId) -> String {
    let mut comps: Vec<String> = Vec::new();
    let mut cur = id;
    let mut guard = 0;
    while cur != ROOT && guard < 10000 {
        let p = parents[cur as usize];
        if p == NONE {
            break;
        }
        if let Some((n, _)) = tree.children(p).iter().find(|(_, c)| *c == cur) {
            comps.push(show(n));
        }
        cur = p;
        guard += 1;
    }
    comps.reverse();
    format!("/{}", comps.join("/"))
}

fn dir_pack(tree: &Tree, d: NodeId) -> Vec<u32> {
    let ch = tree.children(d);
    dirent::pack([1usize, 2].into_iter().chain(ch.iter().map(|(n, _)| n.len())))
}

/// Data blocks of node `id` (indirect blocks not included).
fn data_blocks(tree: &Tree, id: NodeId, dirs: &[Option<Box<DirPlan>>]) -> u64 {
    match &tree.node(id).kind {
        Kind::Reg(r) => r.size.div_ceil(BS as u64),
        Kind::Dir(_) => dirs[id as usize].as_ref().map(|d| d.nblocks).unwrap_or(1),
        Kind::Symlink(s) => (s.len() > 59) as u64,
        _ => 0,
    }
}

fn node_nb(tree: &Tree, id: NodeId, dirs: &[Option<Box<DirPlan>>], t: &Tiers) -> u64 {
    let n = data_blocks(tree, id, dirs);
    n + layout::dense_indirect(n, t)
}

/// Run the plan phase.
pub fn plan(tree: &mut Tree, p: &Params) -> Result<Plan> {
    let bs = BS;
    let tiers = Tiers::new();
    tree.sort_dirs();
    let n = tree.nodes.len();

    // --- the generated lost+found (R-RES-1), if no input replaced it
    let lost_found = tree
        .children(ROOT)
        .iter()
        .find(|(name, _)| &**name == b"lost+found")
        .map(|(_, id)| *id)
        .filter(|&id| tree.node(id).generated);

    // --- traversal: reachability, parents, link counts, limits
    let mut parents = vec![NONE; n];
    let mut nlink = vec![0u32; n];
    let mut reach = vec![false; n];
    let mut dir_order: Vec<NodeId> = Vec::new();
    reach[ROOT as usize] = true;
    parents[ROOT as usize] = ROOT;
    let mut stack = vec![ROOT];
    let mut large_file = false;
    let max_size = layout::MAX_FILE_SIZE;
    while let Some(d) = stack.pop() {
        dir_order.push(d);
        let mut subdirs = 0u64;
        for (name, c) in tree.children(d) {
            validate_name(name, &|| path_of(tree, &parents, d))?;
            let c = *c;
            let node = tree.node(c);
            if node.is_dir() {
                if reach[c as usize] {
                    return Err(Error::fatal(format!("directory hard link at {}", path_of(tree, &parents, d))));
                }
                subdirs += 1;
                parents[c as usize] = d;
                reach[c as usize] = true;
                stack.push(c);
            } else {
                if !reach[c as usize] {
                    reach[c as usize] = true;
                    parents[c as usize] = d;
                }
                nlink[c as usize] += 1;
                if nlink[c as usize] as u64 > LINK_MAX {
                    return Err(Error::fatal(format!(
                        "too many hard links to {} (max {LINK_MAX})",
                        path_of(tree, &parents, c)
                    )));
                }
            }
        }
        if 2 + subdirs > LINK_MAX {
            return Err(Error::fatal(format!(
                "too many subdirectories in {} (max {})",
                path_of(tree, &parents, d),
                LINK_MAX - 2
            )));
        }
        nlink[d as usize] = (2 + subdirs) as u32;
    }
    // per-node limits
    for id in 0..n as NodeId {
        if !reach[id as usize] {
            continue;
        }
        let node = tree.node(id);
        let what = || path_of(tree, &parents, id);
        if node.uid > MAX_ID || node.gid > MAX_ID {
            return Err(Error::fatal(format!("{}: uid/gid out of range (max {MAX_ID})", what())));
        }
        match &node.kind {
            Kind::Symlink(t) => {
                if t.is_empty() || t.len() > bs as usize - 1 {
                    return Err(Error::fatal(format!(
                        "{}: symlink target length {} not in 1..{}",
                        what(),
                        t.len(),
                        bs - 1
                    )));
                }
            }
            Kind::Chr(ma, mi) | Kind::Blk(ma, mi) => {
                if *ma > 4095 || *mi > 1_048_575 {
                    return Err(Error::fatal(format!("{}: device number {ma}:{mi} out of range", what())));
                }
            }
            Kind::Reg(r) => {
                if r.size > max_size {
                    return Err(Error::fatal(format!(
                        "{}: file too large ({} bytes, max {max_size} with {bs}-byte blocks)",
                        what(),
                        r.size
                    )));
                }
                if r.size > i32::MAX as u64 {
                    large_file = true;
                }
            }
            _ => {}
        }
    }

    // --- directory packing; lost+found gets at least LOST_FOUND_BLOCKS
    let mut dirs: Vec<Option<Box<DirPlan>>> = (0..n).map(|_| None).collect();
    for &d in &dir_order {
        let starts = dir_pack(tree, d);
        let mut nblocks = starts.len() as u64;
        if Some(d) == lost_found {
            nblocks = nblocks.max(LOST_FOUND_BLOCKS);
        }
        if nblocks * bs as u64 > (1u64 << 32) - bs as u64 {
            return Err(Error::fatal(format!("directory {} too large", path_of(tree, &parents, d))));
        }
        dirs[d as usize] = Some(Box::new(DirPlan { starts, nblocks, parent: parents[d as usize] }));
    }

    // --- blocks per node, inode count and size (R-SIZE-1..4)
    let mut nb = vec![0u64; n];
    let mut dsum = 0u64;
    let mut u = 0u64;
    for id in 0..n {
        if reach[id] {
            nb[id] = node_nb(tree, id as NodeId, &dirs, &tiers);
            dsum += nb[id];
            u += (id as NodeId != ROOT) as u64;
        }
    }
    let inodes = 10 + u + p.free_inodes;
    if inodes > u32::MAX as u64 {
        return Err(Error::fatal(format!("too many inodes ({inodes})")));
    }
    let blocks = geometry::size(dsum, p.free_blocks, inodes)?;
    let geo = match geometry::geometry(blocks, inodes) {
        Geo::Valid(g) => g,
        Geo::Invalid { .. } => return Err(Error::fatal("no valid geometry (too many inodes)")),
    };
    if geo.inodes_count() > u32::MAX as u64 {
        return Err(Error::fatal("too many inodes"));
    }

    // --- allocation (R-ALLOC)
    let map = DataMap::new(&geo);
    let mut a = Alloc {
        tree,
        geo: &geo,
        map: &map,
        tiers,
        c: 0,
        rem: dsum,
        next: vec![0u32; geo.groups as usize],
        used_dirs: vec![0u32; geo.groups as usize],
        ino: vec![0u32; n],
        first_extent: vec![NONE; n],
        nb: &nb,
        dirs: &dirs,
        extents: Vec::new(),
        alloc: Vec::new(),
    };
    a.next[0] = 10;
    // root: inode 2
    a.ino[ROOT as usize] = 2;
    a.used_dirs[0] += 1;
    a.assign_blocks(ROOT);
    if let Some(l) = lost_found {
        a.place(l, Some(0));
        a.process(l, false, None);
    }
    a.process(ROOT, true, lost_found);
    debug_assert_eq!(a.rem, 0);

    let Alloc { next, used_dirs, ino, first_extent, extents, alloc, .. } = a;
    // per-group free blocks
    let mut free_blocks = Vec::with_capacity(geo.groups as usize);
    let mut ai = 0usize;
    for g in 0..geo.groups {
        let lo = map.first_index(g);
        let hi = lo + geo.data_blocks(g);
        let mut used = 0u64;
        while ai < alloc.len() && alloc[ai].0 + alloc[ai].1 <= lo {
            ai += 1;
        }
        let mut j = ai;
        while j < alloc.len() && alloc[j].0 < hi {
            let s = alloc[j].0.max(lo);
            let e = (alloc[j].0 + alloc[j].1).min(hi);
            used += e - s;
            j += 1;
        }
        free_blocks.push(geo.data_blocks(g) - used);
    }
    // inode slots
    let mut slot_base = Vec::with_capacity(geo.groups as usize);
    let mut total = 0u64;
    for &used in &next {
        slot_base.push(total);
        total += used as u64;
    }
    let mut slots = vec![NONE; total as usize];
    let ipg = geo.ipg;
    let mut inodes_used = 0;
    for (id, &i) in ino.iter().enumerate() {
        if i != 0 {
            let g = (i as u64 - 1) / ipg;
            let j = (i as u64 - 1) % ipg;
            slots[(slot_base[g as usize] + j) as usize] = id as NodeId;
            inodes_used += 1;
        }
    }
    Ok(Plan {
        geo,
        map,
        tiers,
        large_file,
        ino,
        nlink,
        nb,
        first_extent,
        dirs,
        lost_found,
        extents,
        alloc,
        used_inodes: next,
        used_dirs,
        free_blocks,
        slot_base,
        slots,
        total_blocks_used: dsum,
        inodes_used,
    })
}

struct Alloc<'a> {
    tree: &'a Tree,
    geo: &'a Geometry,
    map: &'a DataMap,
    tiers: Tiers,
    c: u64,
    rem: u64,
    next: Vec<u32>,
    used_dirs: Vec<u32>,
    ino: Vec<u32>,
    first_extent: Vec<u32>,
    nb: &'a [u64],
    dirs: &'a [Option<Box<DirPlan>>],
    extents: Vec<Extent>,
    alloc: Vec<(u64, u64)>,
}

impl Alloc<'_> {
    fn cursor_group(&self) -> u64 {
        self.map.group_of(self.c)
    }

    fn alloc_inode(&mut self, g0: u64) -> u32 {
        let groups = self.geo.groups;
        for k in 0..groups {
            let g = ((g0 + k) % groups) as usize;
            if (self.next[g] as u64) < self.geo.ipg {
                let i = g as u64 * self.geo.ipg + self.next[g] as u64 + 1;
                self.next[g] += 1;
                return i as u32;
            }
        }
        unreachable!("out of inodes");
    }

    fn place(&mut self, x: NodeId, group: Option<u64>) {
        if self.ino[x as usize] != 0 {
            return;
        }
        let g0 = group.unwrap_or_else(|| self.cursor_group());
        let i = self.alloc_inode(g0);
        self.ino[x as usize] = i;
        if self.tree.node(x).is_dir() {
            self.used_dirs[((i as u64 - 1) / self.geo.ipg) as usize] += 1;
        }
        self.assign_blocks(x);
    }

    fn assign_blocks(&mut self, x: NodeId) {
        let nb = self.nb[x as usize];
        if nb == 0 {
            return;
        }
        let base = self.c;
        let n = data_blocks(self.tree, x, self.dirs);
        let mut pos = base;
        let t = self.tiers;
        self.first_extent[x as usize] = self.extents.len() as u32;
        let mut ext = std::mem::take(&mut self.extents);
        layout::walk(n, &t, |s| match s {
            Seg::Data { lstart, n } => {
                debug_assert!(lstart + n <= u32::MAX as u64);
                ext.push(Extent { start: pos, lstart: lstart as u32, node: x, len: n as u32, level: 0 });
                pos += n;
            }
            Seg::Ind { level, lstart } => {
                ext.push(Extent { start: pos, lstart: lstart as u32, node: x, len: 1, level });
                pos += 1;
            }
        });
        self.extents = ext;
        debug_assert_eq!(pos - base, nb);
        match self.alloc.last_mut() {
            Some(last) if last.0 + last.1 == base => last.1 += nb,
            _ => self.alloc.push((base, nb)),
        }
        self.c += nb;
        self.rem -= nb;
    }

    /// PROCESS(d, top) of R-ALLOC-3, iteratively.
    fn process(&mut self, d: NodeId, top: bool, skip: Option<NodeId>) {
        struct Frame {
            dirs: Vec<NodeId>,
            k: usize,
            top: bool,
        }
        let tree = self.tree;
        let mut stack: Vec<Frame> = Vec::new();
        let enter = |a: &mut Self, d: NodeId, top: bool, skip: Option<NodeId>| -> Frame {
            let mut dirs = Vec::new();
            for (_, c) in tree.children(d) {
                let c = *c;
                if tree.node(c).is_dir() {
                    if Some(c) != skip {
                        dirs.push(c);
                    }
                } else {
                    a.place(c, None);
                }
            }
            Frame { dirs, k: 0, top }
        };
        let f = enter(self, d, top, skip);
        stack.push(f);
        while let Some(f) = stack.last_mut() {
            if f.k >= f.dirs.len() {
                stack.pop();
                continue;
            }
            let k = f.k;
            let x = f.dirs[k];
            f.k += 1;
            if f.top {
                let tcount = f.dirs.len() as u64;
                let g = (k as u64 * self.geo.groups) / tcount;
                let t = self.map.first_index(g);
                let lim = self.geo.capacity - self.rem;
                self.c = self.c.max(t.min(lim));
            }
            self.place(x, None);
            let nf = enter(self, x, false, None);
            stack.push(nf);
        }
    }
}

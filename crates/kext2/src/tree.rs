//! The in-memory tree built by the scan phase (SPEC §3, §4).

use crate::error::{show, warn, Error, Result};
use crate::runs::Runs;
use std::collections::HashMap;
use std::fs::File;
use std::path::PathBuf;

pub type NodeId = u32;
pub const ROOT: NodeId = 0;
pub const NONE: NodeId = u32::MAX;

/// A byte region of a regular file's logical content stored contiguously in
/// a source: logical bytes `[off, off+len)` are at source offset `src_off`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Region {
    pub off: u64,
    pub len: u64,
    pub src_off: u64,
}

/// Where the data of a regular file can be read in the write phase.
#[derive(Debug, Clone)]
pub enum Data {
    /// No backing data: every byte is zero (empty files, device-table files).
    Zero,
    /// A host file, read again by path: `name` in host directory `dir`
    /// ([`Tree::host_dirs`]). Directory paths are stored once.
    Host { dir: u32, name: Box<std::ffi::OsStr> },
    /// Regions of a registered source file (tar archive or spool). Bytes not
    /// covered by a region are zero.
    Source { src: u32, regions: Box<[Region]> },
    /// Blocks of a starting image (`-x`): runs of (logical block, physical
    /// block, count) with the image block size. Unmapped blocks are zero.
    Image { src: u32, bs: u32, map: Box<[(u64, u64, u64)]> },
}

#[derive(Debug)]
pub struct Reg {
    pub size: u64,
    pub data: Data,
    /// With `-z`: the logical blocks holding non-zero bytes (boxed: most
    /// files have none, and every regular file carries the field).
    pub zmap: Option<Box<Runs>>,
}

#[derive(Debug, Default)]
pub struct Dir {
    /// Children during the scan.
    pub map: HashMap<Box<[u8]>, NodeId>,
    /// Children sorted by name (filled by [`Tree::sort_dirs`]).
    pub sorted: Vec<(Box<[u8]>, NodeId)>,
}

#[derive(Debug)]
pub enum Kind {
    Dir(Box<Dir>),
    Reg(Box<Reg>),
    /// The target (boxed, so that `Kind`, in every node, stays two words).
    Symlink(Box<Vec<u8>>),
    Chr(u32, u32),
    Blk(u32, u32),
    Fifo,
    Sock,
}

impl Kind {
    pub fn is_dir(&self) -> bool {
        matches!(self, Kind::Dir(_))
    }
    /// ext2 directory entry file type (R-DIRENT-1).
    pub fn file_type(&self) -> u8 {
        match self {
            Kind::Reg(_) => 1,
            Kind::Dir(_) => 2,
            Kind::Chr(..) => 3,
            Kind::Blk(..) => 4,
            Kind::Fifo => 5,
            Kind::Sock => 6,
            Kind::Symlink(_) => 7,
        }
    }
    /// Type bits of `i_mode` (R-FMT-10).
    pub fn mode_bits(&self) -> u16 {
        match self {
            Kind::Reg(_) => 0x8000,
            Kind::Dir(_) => 0x4000,
            Kind::Symlink(_) => 0xA000,
            Kind::Chr(..) => 0x2000,
            Kind::Blk(..) => 0x6000,
            Kind::Fifo => 0x1000,
            Kind::Sock => 0xC000,
        }
    }
}

#[derive(Debug)]
pub struct Node {
    pub kind: Kind,
    /// Permission bits (07777).
    pub perm: u16,
    pub uid: u32,
    pub gid: u32,
    pub mtime: i64,
    /// Created because a path needed it (R-IN-4).
    pub implicit: bool,
    /// The provisional lost+found (R-RES-1).
    pub generated: bool,
    /// Touched by a device-table fix-up.
    pub fixed_up: bool,
}

impl Node {
    pub fn new(kind: Kind, perm: u16, uid: u32, gid: u32, mtime: i64) -> Node {
        Node { kind, perm: perm & 0o7777, uid, gid, mtime, implicit: false, generated: false, fixed_up: false }
    }
    pub fn dir(perm: u16, uid: u32, gid: u32, mtime: i64) -> Node {
        Node::new(Kind::Dir(Box::default()), perm, uid, gid, mtime)
    }
    pub fn is_dir(&self) -> bool {
        self.kind.is_dir()
    }
}

/// A file registered for reading in the write phase.
#[derive(Debug)]
pub struct Source {
    pub file: File,
    pub name: String,
}

/// Outcome of inserting an entry (R-IN-3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Added {
    New(NodeId),
    Merged(NodeId),
    Ignored,
}

#[derive(Debug)]
pub struct Tree {
    pub nodes: Vec<Node>,
    pub sources: Vec<Source>,
    /// Host directories holding [`Data::Host`] files.
    pub host_dirs: Vec<PathBuf>,
    /// Filesystem time, for implicit directories.
    pub t_fs: i64,
}

impl Tree {
    /// A tree with an implicit root (R-IN-1).
    pub fn new(t_fs: i64) -> Tree {
        let mut root = Node::dir(0o755, 0, 0, t_fs);
        root.implicit = true;
        Tree { nodes: vec![root], sources: Vec::new(), host_dirs: Vec::new(), t_fs }
    }

    pub fn node(&self, id: NodeId) -> &Node {
        &self.nodes[id as usize]
    }
    pub fn node_mut(&mut self, id: NodeId) -> &mut Node {
        &mut self.nodes[id as usize]
    }

    pub fn push(&mut self, n: Node) -> NodeId {
        let id = self.nodes.len();
        assert!(id < NONE as usize, "too many nodes");
        self.nodes.push(n);
        id as NodeId
    }

    pub fn add_host_dir(&mut self, path: PathBuf) -> u32 {
        self.host_dirs.push(path);
        (self.host_dirs.len() - 1) as u32
    }

    /// Host path of a [`Data::Host`] file.
    pub fn host_path(&self, dir: u32, name: &std::ffi::OsStr) -> PathBuf {
        self.host_dirs[dir as usize].join(name)
    }

    pub fn add_source(&mut self, file: File, name: String) -> u32 {
        self.sources.push(Source { file, name });
        (self.sources.len() - 1) as u32
    }

    fn dir_ref(&self, id: NodeId) -> Option<&Dir> {
        match &self.node(id).kind {
            Kind::Dir(d) => Some(d),
            _ => None,
        }
    }
    fn dir_mut(&mut self, id: NodeId) -> &mut Dir {
        match &mut self.node_mut(id).kind {
            Kind::Dir(d) => d,
            _ => panic!("not a directory"),
        }
    }

    /// Child `name` of directory `dir` (scan phase).
    pub fn child(&self, dir: NodeId, name: &[u8]) -> Option<NodeId> {
        let d = self.dir_ref(dir)?;
        if d.map.is_empty() && !d.sorted.is_empty() {
            return d.sorted.binary_search_by(|(n, _)| (**n).cmp(name)).ok().map(|i| d.sorted[i].1);
        }
        d.map.get(name).copied()
    }

    /// Look up components from `start` without following symlinks.
    pub fn lookup(&self, start: NodeId, comps: &[&[u8]]) -> Option<NodeId> {
        let mut cur = start;
        for c in comps {
            cur = self.child(cur, c)?;
        }
        Some(cur)
    }

    /// Insert a raw child link (no checks).
    pub fn link_child(&mut self, dir: NodeId, name: &[u8], id: NodeId) {
        self.dir_mut(dir).map.insert(name.into(), id);
    }

    /// Remove child `name` of `dir`.
    pub fn unlink_child(&mut self, dir: NodeId, name: &[u8]) {
        let d = self.dir_mut(dir);
        d.map.remove(name);
        d.sorted.retain(|(n, _)| &**n != name);
    }

    /// Walk (and create as implicit directories) `comps` from `start`
    /// (R-IN-2, R-IN-3 step 1). `what` names the path being added, for errors.
    pub fn ensure_dirs(&mut self, start: NodeId, comps: &[&[u8]], what: &[u8]) -> Result<NodeId> {
        let mut cur = start;
        for c in comps {
            match self.child(cur, c) {
                Some(id) => {
                    if !self.node(id).is_dir() {
                        return Err(Error::fatal(format!(
                            "cannot add '{}': '{}' is not a directory",
                            show(what),
                            show(c)
                        )));
                    }
                    cur = id;
                }
                None => {
                    let mut n = Node::dir(0o755, 0, 0, self.t_fs);
                    n.implicit = true;
                    let id = self.push(n);
                    self.link_child(cur, c, id);
                    cur = id;
                }
            }
        }
        Ok(cur)
    }

    /// Apply a directory entry's metadata to an existing directory
    /// (R-IN-3 step 3). Returns true if adopted.
    pub fn merge_meta(&mut self, existing: NodeId, perm: u16, uid: u32, gid: u32, mtime: i64) -> bool {
        let n = self.node_mut(existing);
        if n.implicit {
            n.perm = perm & 0o7777;
            n.uid = uid;
            n.gid = gid;
            if existing != ROOT {
                n.mtime = mtime;
            }
            n.implicit = false;
            true
        } else {
            false
        }
    }

    /// Insert `node` as `name` in `parent` (R-IN-3 steps 2-4).
    pub fn add_entry(&mut self, parent: NodeId, name: &[u8], node: Node) -> Added {
        match self.child(parent, name) {
            None => {
                let id = self.push(node);
                self.link_child(parent, name, id);
                Added::New(id)
            }
            Some(ex) => {
                if self.node(ex).is_dir() && node.is_dir() {
                    self.merge_meta(ex, node.perm, node.uid, node.gid, node.mtime);
                    Added::Merged(ex)
                } else {
                    warn(format!("ignoring duplicate entry {}", show(name)));
                    Added::Ignored
                }
            }
        }
    }

    /// Add another name for the existing non-directory `target`.
    pub fn add_link(&mut self, parent: NodeId, name: &[u8], target: NodeId) -> Added {
        match self.child(parent, name) {
            None => {
                self.link_child(parent, name, target);
                Added::New(target)
            }
            Some(_) => {
                warn(format!("ignoring duplicate entry {}", show(name)));
                Added::Ignored
            }
        }
    }

    /// Sort every directory's children by name (R-MODEL-3).
    pub fn sort_dirs(&mut self) {
        for n in self.nodes.iter_mut() {
            if let Kind::Dir(d) = &mut n.kind {
                if !d.map.is_empty() {
                    let mut v: Vec<(Box<[u8]>, NodeId)> = std::mem::take(&mut d.map).into_iter().collect();
                    v.extend(std::mem::take(&mut d.sorted));
                    v.sort_unstable_by(|a, b| a.0.cmp(&b.0));
                    d.sorted = v;
                }
            }
        }
    }

    /// Sorted children of a directory (after [`Tree::sort_dirs`]).
    pub fn children(&self, dir: NodeId) -> &[(Box<[u8]>, NodeId)] {
        match &self.node(dir).kind {
            Kind::Dir(d) => &d.sorted,
            _ => &[],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file() -> Node {
        Node::new(Kind::Fifo, 0o644, 1, 1, 5)
    }

    #[test]
    fn first_wins_and_merge() {
        let mut t = Tree::new(100);
        let a = t.add_entry(ROOT, b"a", file());
        assert!(matches!(a, Added::New(_)));
        assert_eq!(t.add_entry(ROOT, b"a", Node::dir(0o700, 0, 0, 0)), Added::Ignored);
        let d = match t.add_entry(ROOT, b"d", Node::dir(0o700, 3, 4, 9)) {
            Added::New(d) => d,
            _ => panic!(),
        };
        assert_eq!(t.add_entry(ROOT, b"d", Node::dir(0o755, 5, 5, 1)), Added::Merged(d));
        assert_eq!(t.node(d).uid, 3);
    }

    #[test]
    fn implicit_adoption() {
        let mut t = Tree::new(100);
        let p = t.ensure_dirs(ROOT, &[b"x", b"y"], b"x/y/z").unwrap();
        assert!(t.node(p).implicit);
        assert_eq!(t.node(p).mtime, 100);
        let x = t.lookup(ROOT, &[b"x"]).unwrap();
        assert_eq!(t.add_entry(ROOT, b"x", Node::dir(0o700, 7, 8, 9)), Added::Merged(x));
        assert_eq!((t.node(x).perm, t.node(x).uid, t.node(x).mtime), (0o700, 7, 9));
        assert!(!t.node(x).implicit);
        // root keeps its time
        t.merge_meta(ROOT, 0o711, 1, 2, 5);
        assert_eq!((t.node(ROOT).perm, t.node(ROOT).mtime), (0o711, 100));
        // a non-directory in the way is fatal
        t.add_entry(ROOT, b"f", file());
        assert!(t.ensure_dirs(ROOT, &[b"f", b"g"], b"f/g/h").is_err());
    }
}

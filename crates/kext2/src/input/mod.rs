//! Input layers (SPEC §4, §5): host directories, tar archives, device tables.

pub mod devtable;
pub mod dir;
pub mod tar;

use crate::tree::{Kind, Node, NodeId, Tree};
use std::collections::HashMap;

/// Scan-phase state shared by the layers.
pub struct Ctx {
    pub tree: Tree,
    pub squash_uid: Option<u32>,
    pub squash_perms: bool,
    /// Hard-link detection across `-d` layers: (st_dev, st_ino) → node.
    pub host_links: HashMap<(u64, u64), NodeId>,
    /// (st_dev, st_ino) of the output file, if it already exists.
    pub output_id: Option<(u64, u64)>,
}

impl Ctx {
    /// Apply `-U`/`-P` to a node inserted by a `-d` or `-a` layer (R-SQ-1).
    pub fn squash(&self, n: &mut Node) {
        if let Some(u) = self.squash_uid {
            n.uid = u;
            n.gid = u;
        }
        if self.squash_perms && !matches!(n.kind, Kind::Symlink(_)) {
            n.perm &= !0o077;
        }
    }
}

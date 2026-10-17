//! Block-map files (`-g`, SPEC §11.2).

use crate::error::{io_err, show, Error, Result};
use crate::plan::Plan;
use crate::tree::{Kind, NodeId, Tree, ROOT};
use std::io::Write;

/// Resolve an image path from the root: `.` and empty components ignored,
/// `..` goes to the parent, symlinks are not followed.
pub fn resolve(tree: &Tree, plan: &Plan, path: &[u8]) -> Option<NodeId> {
    let mut stack: Vec<NodeId> = vec![ROOT];
    for c in path.split(|&b| b == b'/') {
        match c {
            b"" | b"." => {}
            b".." => {
                if stack.len() > 1 {
                    stack.pop();
                }
            }
            c => {
                let cur = *stack.last().unwrap();
                if !tree.node(cur).is_dir() {
                    return None;
                }
                let id = tree.child(cur, c)?;
                if plan.ino[id as usize] == 0 {
                    return None;
                }
                stack.push(id);
            }
        }
    }
    Some(*stack.last().unwrap())
}

/// Write the block map of node `id`: `i_size`, `:`, then for every logical
/// block a space and its block number (0 for a hole), then a newline.
pub fn render(tree: &Tree, plan: &Plan, id: NodeId, out: &mut impl Write) -> std::io::Result<()> {
    let node = tree.node(id);
    let bs = plan.geo.bs as u64;
    let size = match &node.kind {
        Kind::Reg(r) => r.size,
        Kind::Dir(_) => plan.dirs[id as usize].as_ref().map(|d| d.nblocks * bs).unwrap_or(0),
        Kind::Symlink(t) => t.len() as u64,
        _ => 0,
    };
    write!(out, "{size}:")?;
    if plan.nb[id as usize] > 0 {
        let nblocks = size.div_ceil(bs);
        let mut next = 0u64; // next logical block to print
        let mut err = Ok(());
        // extents of one node are in increasing logical order
        for e in plan.extents.iter().filter(|e| e.node == id && e.level == 0) {
            let mut l = e.lstart;
            plan.map.for_runs(e.start, e.len as u64, |b, n| {
                for k in 0..n {
                    if l + k >= nblocks || err.is_err() {
                        continue;
                    }
                    while next < l + k {
                        if let Err(e) = out.write_all(b" 0") {
                            err = Err(e);
                        }
                        next += 1;
                    }
                    if let Err(e) = write!(out, " {}", b + k) {
                        err = Err(e);
                    }
                    next += 1;
                }
                l += n;
            });
        }
        err?;
        while next < nblocks {
            out.write_all(b" 0")?;
            next += 1;
        }
    }
    out.write_all(b"\n")
}

/// Write `<cwd>/<path with / replaced by _>.blk` for every `-g` path.
pub fn write_all(tree: &Tree, plan: &Plan, paths: &[Vec<u8>]) -> Result<()> {
    for p in paths {
        let id =
            resolve(tree, plan, p).ok_or_else(|| Error::fatal(format!("path {} not found in filesystem", show(p))))?;
        let mut fname: Vec<u8> = p.iter().map(|&b| if b == b'/' { b'_' } else { b }).collect();
        fname.extend_from_slice(b".blk");
        let fname = crate::host::os_string(fname);
        let f = std::fs::File::create(&fname).map_err(|e| io_err(fname.to_string_lossy(), e))?;
        let mut w = std::io::BufWriter::new(f);
        render(tree, plan, id, &mut w).and_then(|_| w.flush()).map_err(|e| io_err(fname.to_string_lossy(), e))?;
    }
    Ok(())
}

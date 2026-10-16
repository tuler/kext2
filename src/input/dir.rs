//! Host directories (`-d`, SPEC §5.1).

use super::Ctx;
use crate::error::{io_err, warn, Error, Result};
use crate::runs::ZeroMapBuilder;
use crate::tree::{Added, Data, Kind, Node, NodeId, Reg};
use std::fs;
use std::io::Read;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::path::{Path, PathBuf};

/// Host major/minor decoding (R-DIR-2).
pub fn host_major_minor(rdev: u64) -> (u64, u64) {
    #[cfg(target_os = "macos")]
    {
        ((rdev >> 24) & 0xff, rdev & 0xff_ffff)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let major = ((rdev >> 8) & 0xfff) | ((rdev >> 32) & !0xfff);
        let minor = (rdev & 0xff) | ((rdev >> 12) & !0xff);
        (major, minor)
    }
}

/// Copy the children of host directory `src` into image directory `target`.
pub fn add_dir(ctx: &mut Ctx, src: &Path, target: NodeId) -> Result<()> {
    let md = fs::metadata(src).map_err(|_| Error::fatal(format!("{} should be a directory", src.display())))?;
    if !md.is_dir() {
        return Err(Error::fatal(format!("{} should be a directory", src.display())));
    }
    // explicit stack: (host dir, image dir)
    let mut stack: Vec<(PathBuf, NodeId)> = vec![(src.to_path_buf(), target)];
    while let Some((hdir, idir)) = stack.pop() {
        let rd = fs::read_dir(&hdir).map_err(|e| io_err(hdir.display(), e))?;
        let mut names: Vec<std::ffi::OsString> = Vec::new();
        for ent in rd {
            let ent = ent.map_err(|e| io_err(hdir.display(), e))?;
            names.push(ent.file_name());
        }
        names.sort_unstable();
        let mut subdirs = Vec::new();
        for name in names {
            let path = hdir.join(&name);
            let st = fs::symlink_metadata(&path).map_err(|e| io_err(path.display(), e))?;
            let nb = name.as_bytes();
            if let Some(Added::New(id) | Added::Merged(id)) = add_entry(ctx, &path, nb, &st, idir)? {
                if st.is_dir() {
                    subdirs.push((path, id));
                }
            }
        }
        // reverse so that the stack pops in name order
        stack.extend(subdirs.into_iter().rev());
    }
    Ok(())
}

fn add_entry(ctx: &mut Ctx, path: &Path, name: &[u8], st: &fs::Metadata, idir: NodeId) -> Result<Option<Added>> {
    let ft = st.file_type();
    let key = (st.dev(), st.ino());
    if ft.is_file() && Some(key) == ctx.output_id {
        warn(format!("skipping the output image {}", path.display()));
        return Ok(None);
    }
    let perm = (st.mode() & 0o7777) as u16;
    let (uid, gid) = (st.uid(), st.gid());
    let mtime = st.mtime();
    if !ft.is_dir() && st.nlink() > 1 {
        if let Some(&existing) = ctx.host_links.get(&key) {
            return Ok(Some(ctx.tree.add_link(idir, name, existing)));
        }
    }
    let kind = if ft.is_dir() {
        Kind::Dir(Box::default())
    } else if ft.is_file() {
        let size = st.len();
        Kind::Reg(Box::new(Reg { size, data: Data::Host(path.to_path_buf()), zmap: None }))
    } else if ft.is_symlink() {
        let t = fs::read_link(path).map_err(|e| io_err(path.display(), e))?;
        Kind::Symlink(t.as_os_str().as_bytes().into())
    } else if ft.is_char_device() || ft.is_block_device() {
        let (ma, mi) = host_major_minor(st.rdev());
        let (ma, mi) = (ma.min(u32::MAX as u64) as u32, mi.min(u32::MAX as u64) as u32);
        if ft.is_char_device() {
            Kind::Chr(ma, mi)
        } else {
            Kind::Blk(ma, mi)
        }
    } else if ft.is_fifo() {
        Kind::Fifo
    } else if ft.is_socket() {
        Kind::Sock
    } else {
        warn(format!("skipping {}: unsupported file type", path.display()));
        return Ok(None);
    };
    let is_link = matches!(kind, Kind::Symlink(_));
    let is_reg = matches!(kind, Kind::Reg(_));
    let mut node = Node::new(kind, if is_link { 0o777 } else { perm }, uid, gid, mtime);
    ctx.squash(&mut node);
    let added = ctx.tree.add_entry(idir, name, node);
    if let Added::New(id) = added {
        if !ft.is_dir() && st.nlink() > 1 {
            ctx.host_links.insert(key, id);
        }
        if is_reg && ctx.holes {
            let zmap = zero_map_of(path, st.len(), ctx.bs)?;
            if let Kind::Reg(r) = &mut ctx.tree.node_mut(id).kind {
                r.zmap = Some(zmap);
            }
        }
    }
    Ok(Some(added))
}

/// Read a host file and compute its non-zero blocks (for `-z`).
fn zero_map_of(path: &Path, size: u64, bs: u32) -> Result<crate::runs::Runs> {
    let mut f = fs::File::open(path).map_err(|e| io_err(path.display(), e))?;
    let mut z = ZeroMapBuilder::new(bs);
    let mut buf = vec![0u8; 1 << 20];
    let mut off = 0u64;
    loop {
        let n = f.read(&mut buf).map_err(|e| io_err(path.display(), e))?;
        if n == 0 {
            break;
        }
        z.feed(off, &buf[..n]);
        off += n as u64;
    }
    if off != size {
        return Err(Error::fatal(format!("{}: file size changed while reading", path.display())));
    }
    Ok(z.finish())
}

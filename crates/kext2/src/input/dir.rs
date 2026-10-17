//! Host directories (`-d`, SPEC §5.1).

use super::Ctx;
use crate::error::{io_err, warn, Error, Result};
use crate::host;
use crate::tree::{Added, Data, Kind, Node, NodeId, Reg};
use std::fs;
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
        let mut hid = None;
        for name in names {
            let path = hdir.join(&name);
            let st = fs::symlink_metadata(&path).map_err(|e| io_err(path.display(), e))?;
            let nb = host::os_bytes(&name);
            let host = if st.file_type().is_file() {
                let h = *hid.get_or_insert_with(|| ctx.tree.add_host_dir(hdir.clone()));
                Some((h, name.into_boxed_os_str()))
            } else {
                None
            };
            if let Some(Added::New(id) | Added::Merged(id)) = add_entry(ctx, &path, &nb, &st, idir, host)? {
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

/// `host`: for a regular file, its host directory id and name.
fn add_entry(
    ctx: &mut Ctx,
    path: &Path,
    name: &[u8],
    st: &fs::Metadata,
    idir: NodeId,
    host: Option<(u32, Box<std::ffi::OsStr>)>,
) -> Result<Option<Added>> {
    let ft = st.file_type();
    let key = host::file_id(st);
    if ft.is_file() && key.is_some() && key == ctx.output_id {
        warn(format!("skipping the output image {}", path.display()));
        return Ok(None);
    }
    let perm = host::perm(st);
    let (uid, gid) = host::owner(st);
    let mtime = host::mtime(st);
    let key = key.filter(|_| !ft.is_dir() && host::nlink(st) > 1);
    if let Some(key) = key {
        if let Some(&existing) = ctx.host_links.get(&key) {
            return Ok(Some(ctx.tree.add_link(idir, name, existing)));
        }
    }
    let kind = if ft.is_dir() {
        Kind::Dir(Box::default())
    } else if ft.is_file() {
        let size = st.len();
        let (dir, name) = host.expect("regular files carry their host name");
        Kind::Reg(Box::new(Reg { size, data: Data::Host { dir, name } }))
    } else if ft.is_symlink() {
        let t = fs::read_link(path).map_err(|e| io_err(path.display(), e))?;
        Kind::Symlink(host::os_bytes(t.as_os_str()).into())
    } else if host::is_char_device(&ft) || host::is_block_device(&ft) {
        let (ma, mi) = host_major_minor(host::rdev(st));
        let (ma, mi) = (ma.min(u32::MAX as u64) as u32, mi.min(u32::MAX as u64) as u32);
        if host::is_char_device(&ft) {
            Kind::Chr(ma, mi)
        } else {
            Kind::Blk(ma, mi)
        }
    } else if host::is_fifo(&ft) {
        Kind::Fifo
    } else if host::is_socket(&ft) {
        Kind::Sock
    } else {
        warn(format!("skipping {}: unsupported file type", path.display()));
        return Ok(None);
    };
    let is_link = matches!(kind, Kind::Symlink(_));
    let mut node = Node::new(kind, if is_link { 0o777 } else { perm }, uid, gid, mtime);
    ctx.squash(&mut node);
    let added = ctx.tree.add_entry(idir, name, node);
    if let Added::New(id) = added {
        if let Some(key) = key {
            ctx.host_links.insert(key, id);
        }
    }
    Ok(Some(added))
}

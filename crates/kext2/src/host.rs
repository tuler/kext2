//! Host platform helpers: byte strings for paths, file metadata and
//! positional reads, for Unix and Windows.
//!
//! On Unix, names are the raw bytes the OS gives. On Windows, they are UTF-8
//! (unpaired surrogates are replaced), and files have no owner, permission
//! bits, inode identity or device numbers: regular files are 0644 (0444 when
//! read-only), directories 0755, owned by 0:0, and hard links are not
//! detected.

use std::ffi::{OsStr, OsString};
use std::fs::{File, FileType, Metadata};

/// The bytes of an OS string.
pub fn os_bytes(s: &OsStr) -> Vec<u8> {
    #[cfg(unix)]
    {
        std::os::unix::ffi::OsStrExt::as_bytes(s).to_vec()
    }
    #[cfg(not(unix))]
    {
        s.to_string_lossy().into_owned().into_bytes()
    }
}

/// An OS string from bytes ([`os_bytes`] in reverse).
pub fn os_string(b: Vec<u8>) -> OsString {
    #[cfg(unix)]
    {
        std::os::unix::ffi::OsStringExt::from_vec(b)
    }
    #[cfg(not(unix))]
    {
        OsString::from(String::from_utf8_lossy(&b).into_owned())
    }
}

/// Identity of a file (device, inode), when the platform has one.
pub fn file_id(md: &Metadata) -> Option<(u64, u64)> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Some((md.dev(), md.ino()))
    }
    #[cfg(not(unix))]
    {
        let _ = md;
        None
    }
}

/// Number of hard links (1 where unknown).
pub fn nlink(md: &Metadata) -> u64 {
    #[cfg(unix)]
    {
        std::os::unix::fs::MetadataExt::nlink(md)
    }
    #[cfg(not(unix))]
    {
        let _ = md;
        1
    }
}

/// Permission bits (including set-id and sticky bits).
pub fn perm(md: &Metadata) -> u16 {
    #[cfg(unix)]
    {
        (std::os::unix::fs::MetadataExt::mode(md) & 0o7777) as u16
    }
    #[cfg(not(unix))]
    {
        if md.is_dir() {
            0o755
        } else if md.permissions().readonly() {
            0o444
        } else {
            0o644
        }
    }
}

/// Owner (uid, gid).
pub fn owner(md: &Metadata) -> (u32, u32) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        (md.uid(), md.gid())
    }
    #[cfg(not(unix))]
    {
        let _ = md;
        (0, 0)
    }
}

/// Modification time in seconds since the epoch.
pub fn mtime(md: &Metadata) -> i64 {
    #[cfg(unix)]
    {
        std::os::unix::fs::MetadataExt::mtime(md)
    }
    #[cfg(not(unix))]
    {
        match md.modified() {
            Ok(t) => match t.duration_since(std::time::UNIX_EPOCH) {
                Ok(d) => d.as_secs() as i64,
                Err(e) => -(e.duration().as_secs_f64().ceil() as i64),
            },
            Err(_) => 0,
        }
    }
}

/// Raw device number of a device file.
pub fn rdev(md: &Metadata) -> u64 {
    #[cfg(unix)]
    {
        std::os::unix::fs::MetadataExt::rdev(md)
    }
    #[cfg(not(unix))]
    {
        let _ = md;
        0
    }
}

/// Special file types (always false where the platform has none).
pub fn is_char_device(ft: &FileType) -> bool {
    #[cfg(unix)]
    {
        std::os::unix::fs::FileTypeExt::is_char_device(ft)
    }
    #[cfg(not(unix))]
    {
        let _ = ft;
        false
    }
}

pub fn is_block_device(ft: &FileType) -> bool {
    #[cfg(unix)]
    {
        std::os::unix::fs::FileTypeExt::is_block_device(ft)
    }
    #[cfg(not(unix))]
    {
        let _ = ft;
        false
    }
}

pub fn is_fifo(ft: &FileType) -> bool {
    #[cfg(unix)]
    {
        std::os::unix::fs::FileTypeExt::is_fifo(ft)
    }
    #[cfg(not(unix))]
    {
        let _ = ft;
        false
    }
}

pub fn is_socket(ft: &FileType) -> bool {
    #[cfg(unix)]
    {
        std::os::unix::fs::FileTypeExt::is_socket(ft)
    }
    #[cfg(not(unix))]
    {
        let _ = ft;
        false
    }
}

/// Read at an offset without using the file position (Unix `pread`). On
/// Windows the file position moves, so callers must not mix this with
/// sequential reads of the same handle.
pub fn read_at(f: &File, buf: &mut [u8], off: u64) -> std::io::Result<usize> {
    #[cfg(unix)]
    {
        std::os::unix::fs::FileExt::read_at(f, buf, off)
    }
    #[cfg(windows)]
    {
        std::os::windows::fs::FileExt::seek_read(f, buf, off)
    }
}

/// Write all of `buf` at an offset (Unix `pwrite`). On Windows the file
/// position moves.
pub fn write_all_at(f: &File, mut buf: &[u8], mut off: u64) -> std::io::Result<()> {
    while !buf.is_empty() {
        #[cfg(unix)]
        let r = std::os::unix::fs::FileExt::write_at(f, buf, off);
        #[cfg(windows)]
        let r = std::os::windows::fs::FileExt::seek_write(f, buf, off);
        match r {
            Ok(0) => return Err(std::io::ErrorKind::WriteZero.into()),
            Ok(n) => {
                buf = &buf[n..];
                off += n as u64;
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

/// Create a new file (failing if it exists), readable and writable, with
/// permission bits 0666 before the umask on Unix.
pub fn create_new(path: &std::path::Path) -> std::io::Result<File> {
    let mut o = std::fs::OpenOptions::new();
    o.read(true).write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut o, 0o666);
    o.open(path)
}

/// Copy the permission bits of `from` (an existing file) to `file`, where the
/// platform has them.
pub fn copy_permissions(from: &Metadata, file: &File) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = from.permissions().mode() & 0o7777;
        let _ = file.set_permissions(std::fs::Permissions::from_mode(mode));
    }
    #[cfg(not(unix))]
    {
        // A read-only attribute would only make the final rename fail.
        let _ = (from, file);
    }
}

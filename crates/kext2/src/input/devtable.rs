//! Device tables (`-D`, SPEC §5.3).

use super::Ctx;
use crate::error::{io_err, show, warn, Error, Result};
use crate::tree::{Data, Kind, Node, NodeId, Reg};
use std::io::Read;

/// One parsed device-table line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    pub name: Vec<u8>,
    pub typ: u8,
    pub mode: u16,
    pub uid: u64,
    pub gid: u64,
    pub major: u64,
    pub minor: u64,
    pub start: u64,
    pub inc: u64,
    pub count: u64,
}

/// Parse result for one line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Parsed {
    Skip,
    Bad(String),
    Ok(Line),
}

const MAX_ID: u64 = 4_294_967_294;
const MAX_MAJOR: u64 = 4095;
const MAX_MINOR: u64 = 1_048_575;

/// Parse one line (R-DT-1, R-DT-2).
pub fn parse_line(line: &[u8], lineno: usize) -> Parsed {
    let toks: Vec<&[u8]> = line.split(|b| b.is_ascii_whitespace()).filter(|t| !t.is_empty()).collect();
    if toks.is_empty() || toks[0].starts_with(b"#") {
        return Parsed::Skip;
    }
    let name = toks[0];
    let badfmt = || Parsed::Bad(format!("device table line {lineno} skipped: bad format for entry '{}'", show(name)));
    if toks.len() < 3 || toks[1].len() != 1 || !(b'0'..=b'7').contains(&toks[2][0]) {
        return badfmt();
    }
    let typ = toks[1][0];
    let mtok = toks[2];
    let mut mode: u64 = 0;
    let mut k = 0;
    while k < mtok.len() && (b'0'..=b'7').contains(&mtok[k]) {
        mode = mode.wrapping_mul(8).wrapping_add((mtok[k] - b'0') as u64);
        k += 1;
    }
    let mut vals = [0u64; 7];
    let mut ended = k < mtok.len();
    let mut negative = false;
    for (i, v) in vals.iter_mut().enumerate() {
        if ended {
            break;
        }
        let Some(t) = toks.get(3 + i) else { break };
        if t[0] == b'-' && t.len() > 1 && t[1].is_ascii_digit() {
            // a negative number: out of range (D-25)
            negative = true;
            break;
        }
        if !t[0].is_ascii_digit() {
            break;
        }
        let mut x: u64 = 0;
        let mut j = 0;
        while j < t.len() && t[j].is_ascii_digit() {
            x = x.saturating_mul(10).saturating_add((t[j] - b'0') as u64);
            j += 1;
        }
        *v = x;
        if j < t.len() {
            ended = true;
        }
    }
    let [uid, gid, major, minor, start, inc, count] = vals;
    if negative || uid > MAX_ID || gid > MAX_ID || major > MAX_MAJOR || minor > MAX_MINOR {
        return Parsed::Bad(format!(
            "device table line {lineno} skipped: value out of range for entry '{}'",
            show(name)
        ));
    }
    Parsed::Ok(Line {
        name: name.to_vec(),
        typ,
        mode: (mode & 0o7777) as u16,
        uid,
        gid,
        major,
        minor,
        start,
        inc,
        count,
    })
}

/// Apply device table `src` (`-` = standard input) relative to `target`.
pub fn add_devtable(ctx: &mut Ctx, src: &std::ffi::OsStr, target: NodeId, t_fs: i64) -> Result<()> {
    let name = src.to_string_lossy().into_owned();
    let mut text = Vec::new();
    let mtime;
    if src == "-" {
        std::io::stdin().lock().read_to_end(&mut text).map_err(|e| io_err("standard input", e))?;
        mtime = t_fs;
    } else {
        let bad = || Error::fatal(format!("{name} should be a file"));
        let md = std::fs::metadata(src).map_err(|_| bad())?;
        if !md.is_file() {
            return Err(bad());
        }
        let mut f = std::fs::File::open(src).map_err(|e| io_err(&name, e))?;
        f.read_to_end(&mut text).map_err(|e| io_err(&name, e))?;
        mtime = crate::host::mtime(&md);
    }
    for (i, line) in text.split(|&b| b == b'\n').enumerate() {
        let lineno = i + 1;
        let l = match parse_line(line, lineno) {
            Parsed::Skip => continue,
            Parsed::Bad(m) => {
                warn(m);
                continue;
            }
            Parsed::Ok(l) => l,
        };
        apply_line(ctx, &l, lineno, target, mtime);
    }
    Ok(())
}

fn apply_line(ctx: &mut Ctx, l: &Line, lineno: usize, target: NodeId, mtime: i64) {
    if !b"fdcbps".contains(&l.typ) {
        if l.typ == b'l' {
            warn(format!("device table line {lineno} skipped: symlinks need a target ('{}')", show(&l.name)));
        } else {
            warn(format!("bad type '{}' for entry '{}'", l.typ as char, show(&l.name)));
        }
        return;
    }
    if l.count == 0 {
        apply_node(ctx, &l.name, l, l.minor as i128, lineno, target, mtime);
    } else {
        let mut i = l.start;
        while i < l.count {
            let mut nm = l.name.clone();
            nm.extend_from_slice(i.to_string().as_bytes());
            let minor = l.minor as i128 + i as i128 * l.inc as i128 - l.start as i128;
            apply_node(ctx, &nm, l, minor, lineno, target, mtime);
            i += 1;
        }
    }
}

fn apply_node(ctx: &mut Ctx, name: &[u8], l: &Line, minor: i128, lineno: usize, target: NodeId, mtime: i64) {
    let mut comps: Vec<&[u8]> = Vec::new();
    let mut dotdot = false;
    for c in name.split(|&b| b == b'/') {
        match c {
            b"" | b"." => {}
            b".." => dotdot = true,
            c => comps.push(c),
        }
    }
    let (parent_comps, base) = match comps.split_last() {
        Some((b, p)) => (p, Some(*b)),
        None => (&comps[..], None),
    };
    let parent_name = parent_comps.join(&b'/');
    let cant = || {
        warn(format!(
            "device table line {lineno} skipped: can't find directory '/{}' to create '{}'",
            show(&parent_name),
            show(base.unwrap_or(b""))
        ))
    };
    if dotdot {
        return cant();
    }
    let parent = match ctx.tree.lookup(target, parent_comps) {
        Some(p) if ctx.tree.node(p).is_dir() => p,
        _ => return cant(),
    };
    let existing = match base {
        None => Some(parent),
        Some(b) => ctx.tree.child(parent, b),
    };
    if let Some(id) = existing {
        let n = ctx.tree.node_mut(id);
        n.perm = l.mode;
        n.uid = l.uid as u32;
        n.gid = l.gid as u32;
        n.fixed_up = true;
        return;
    }
    if !(0..=MAX_MINOR as i128).contains(&minor) {
        warn(format!("device table line {lineno}: minor number {minor} out of range for '{}', skipped", show(name)));
        return;
    }
    let (ma, mi) = (l.major as u32, minor as u32);
    let kind = match l.typ {
        b'f' => Kind::Reg(Box::new(Reg { size: 0, data: Data::Zero, zmap: None })),
        b'd' => Kind::Dir(Box::default()),
        b'c' => Kind::Chr(ma, mi),
        b'b' => Kind::Blk(ma, mi),
        b'p' => Kind::Fifo,
        _ => Kind::Sock,
    };
    let node = Node::new(kind, l.mode, l.uid as u32, l.gid as u32, mtime);
    let id = ctx.tree.push(node);
    ctx.tree.link_child(parent, base.unwrap(), id);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(s: &str) -> Line {
        match parse_line(s.as_bytes(), 1) {
            Parsed::Ok(l) => l,
            p => panic!("{s}: {p:?}"),
        }
    }

    #[test]
    fn o27_cases() {
        let l = ok("/dev/hda b 640 0 6 3 1 1 1 4");
        assert_eq!(
            (l.typ, l.mode, l.uid, l.gid, l.major, l.minor, l.start, l.inc, l.count),
            (b'b', 0o640, 0, 6, 3, 1, 1, 1, 4)
        );
        let l = ok("/x c 6a0 5 5 1 1");
        assert_eq!((l.mode, l.uid, l.gid, l.major), (0o6, 0, 0, 0));
        let l = ok("/x c 170600 0 0 1 1");
        assert_eq!(l.mode, 0o600);
        let l = ok("/x c 600 0 0 a b");
        assert_eq!((l.major, l.minor), (0, 0));
        let l = ok("/x c 600 0 0 1 1 - 1 2");
        assert_eq!((l.major, l.minor, l.start, l.inc, l.count), (1, 1, 0, 0, 0));
        let l = ok("/x c 600 0 0 12x 5");
        assert_eq!((l.major, l.minor), (12, 0));
        let l = ok("/x\tc\t600");
        assert_eq!((l.mode, l.uid), (0o600, 0));
        assert!(matches!(parse_line(b"/x c", 1), Parsed::Bad(_)));
        assert!(matches!(parse_line(b"/x cc 600", 1), Parsed::Bad(_)));
        assert!(matches!(parse_line(b"/x c 9600", 1), Parsed::Bad(_)));
        assert!(matches!(parse_line(b"/x c 600 -1 0", 1), Parsed::Bad(_)));
        assert!(matches!(parse_line(b"/x c 600 0 0 4096 0", 1), Parsed::Bad(_)));
        assert!(matches!(parse_line(b"  # comment", 1), Parsed::Skip));
        assert!(matches!(parse_line(b"   ", 1), Parsed::Skip));
    }
}

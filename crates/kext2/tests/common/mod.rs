//! Test helpers: a small tar writer and an image reader built on
//! `kext2::image`.
#![allow(dead_code)]

use kext2::tree::{Data, Kind, NodeId, Tree, ROOT};
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};

// ------------------------------------------------------------------- tar

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Fmt {
    V7,
    Ustar,
    Gnu,
}

pub struct TarW {
    pub out: Vec<u8>,
}

fn octal(field: &mut [u8], v: u64) {
    let w = field.len() - 1;
    let s = format!("{:0w$o}", v, w = w);
    assert!(s.len() <= w, "value too large for field");
    field[..w].copy_from_slice(s.as_bytes());
    field[w] = 0;
}

pub struct H<'a> {
    pub name: &'a [u8],
    pub typeflag: u8,
    pub size: u64,
    pub mode: u32,
    pub uid: u64,
    pub gid: u64,
    pub mtime: i64,
    pub link: &'a [u8],
    pub major: u64,
    pub minor: u64,
    pub fmt: Fmt,
}

impl Default for H<'_> {
    fn default() -> Self {
        H {
            name: b"",
            typeflag: b'0',
            size: 0,
            mode: 0o644,
            uid: 0,
            gid: 0,
            mtime: 1_500_000_000,
            link: b"",
            major: 0,
            minor: 0,
            fmt: Fmt::Gnu,
        }
    }
}

pub fn header(h: &H) -> [u8; 512] {
    let mut b = [0u8; 512];
    let n = h.name.len().min(100);
    b[..n].copy_from_slice(&h.name[..n]);
    octal(&mut b[100..108], h.mode as u64);
    if h.uid >= 1 << 21 {
        // GNU base-256
        b[108] = 0x80;
        b[109..116].copy_from_slice(&h.uid.to_be_bytes()[1..]);
    } else {
        octal(&mut b[108..116], h.uid);
    }
    octal(&mut b[116..124], h.gid);
    octal(&mut b[124..136], h.size);
    if h.mtime < 0 {
        let v = (h.mtime as i128 + (1i128 << 96)) as u128;
        b[136..148].copy_from_slice(&v.to_be_bytes()[4..]);
        b[136] |= 0x80;
        b[136] = 0xFF;
    } else {
        octal(&mut b[136..148], h.mtime as u64);
    }
    b[156] = h.typeflag;
    let l = h.link.len().min(100);
    b[157..157 + l].copy_from_slice(&h.link[..l]);
    match h.fmt {
        Fmt::V7 => {}
        Fmt::Ustar => {
            b[257..263].copy_from_slice(b"ustar\0");
            b[263..265].copy_from_slice(b"00");
        }
        Fmt::Gnu => {
            b[257..263].copy_from_slice(b"ustar ");
            b[263..265].copy_from_slice(b" \0");
        }
    }
    if h.fmt != Fmt::V7 {
        octal(&mut b[329..337], h.major);
        octal(&mut b[337..345], h.minor);
    }
    checksum(&mut b);
    b
}

pub fn checksum(b: &mut [u8; 512]) {
    b[148..156].fill(b' ');
    let sum: u32 = b.iter().map(|&x| x as u32).sum();
    let s = format!("{:06o}\0 ", sum);
    b[148..156].copy_from_slice(s.as_bytes());
}

impl TarW {
    pub fn new() -> TarW {
        TarW { out: Vec::new() }
    }
    pub fn raw(&mut self, h: &[u8; 512], data: &[u8]) {
        self.out.extend_from_slice(h);
        self.out.extend_from_slice(data);
        let pad = (512 - data.len() % 512) % 512;
        self.out.extend(std::iter::repeat_n(0u8, pad));
    }
    pub fn entry(&mut self, h: H, data: &[u8]) {
        let h2 = H { size: data.len() as u64, ..h };
        self.raw(&header(&h2), data);
    }
    pub fn file(&mut self, name: &str, data: &[u8]) {
        self.entry(H { name: name.as_bytes(), ..Default::default() }, data);
    }
    pub fn dir(&mut self, name: &str, mode: u32) {
        self.entry(H { name: name.as_bytes(), typeflag: b'5', mode, ..Default::default() }, b"");
    }
    pub fn gnu_long(&mut self, typeflag: u8, value: &[u8]) {
        let mut v = value.to_vec();
        v.push(0);
        self.entry(H { name: b"././@LongLink", typeflag, ..Default::default() }, &v);
    }
    pub fn pax(&mut self, typeflag: u8, recs: &[(&str, &[u8])]) {
        let mut d = Vec::new();
        for (k, v) in recs {
            let body_len = 1 + k.len() + 1 + v.len() + 1;
            let mut len = body_len + 1;
            loop {
                let l = len.to_string().len() + body_len;
                if l == len {
                    break;
                }
                len = l;
            }
            d.extend_from_slice(format!("{len} {k}=").as_bytes());
            d.extend_from_slice(v);
            d.push(b'\n');
        }
        self.entry(H { name: b"PaxHeaders/x", typeflag, fmt: Fmt::Ustar, ..Default::default() }, &d);
    }
    pub fn finish(mut self) -> Vec<u8> {
        self.out.extend(std::iter::repeat_n(0u8, 1024));
        self.out
    }
}

// ------------------------------------------------------------------- build

pub fn args(v: &[&str]) -> Vec<OsString> {
    v.iter().map(OsString::from).collect()
}

/// Run kext2 with `argv` (without program name); returns Ok or the message.
pub fn kext2(argv: &[&str]) -> Result<(), String> {
    kext2_threads(argv, None)
}

pub fn kext2_threads(argv: &[&str], threads: Option<usize>) -> Result<(), String> {
    match kext2::cli::parse(args(argv)).map_err(|e| e.msg)? {
        kext2::cli::Action::Run(o) => {
            kext2::build_with(&o, Some(OsString::from("1600000000")), threads).map(|_| ()).map_err(|e| e.msg)
        }
        _ => Ok(()),
    }
}

pub fn tmpdir() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

pub fn p(d: &Path, n: &str) -> PathBuf {
    d.join(n)
}

pub fn s(p: &Path) -> &str {
    p.to_str().unwrap()
}

// ------------------------------------------------------------------- read

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct E {
    pub kind: char,
    pub perm: u16,
    pub uid: u32,
    pub gid: u32,
    pub mtime: i64,
    pub data: Vec<u8>,
    pub target: Vec<u8>,
    pub dev: (u32, u32),
    pub ino_group: NodeId,
}

/// Read an image back into a path → entry map (via `kext2::image`).
pub fn read_image(img: &Path) -> BTreeMap<String, E> {
    let mut t = Tree::new(0);
    kext2::image::load(&mut t, img.as_os_str(), false).unwrap();
    t.sort_dirs();
    let mut out = BTreeMap::new();
    let mut stack: Vec<(String, NodeId)> = vec![(String::new(), ROOT)];
    while let Some((path, id)) = stack.pop() {
        for (name, c) in t.children(id).to_vec() {
            let pth = format!("{}/{}", path, String::from_utf8_lossy(&name));
            let n = t.node(c);
            let mut e = E {
                kind: '?',
                perm: n.perm,
                uid: n.uid,
                gid: n.gid,
                mtime: n.mtime,
                data: vec![],
                target: vec![],
                dev: (0, 0),
                ino_group: c,
            };
            match &n.kind {
                Kind::Dir(_) => {
                    e.kind = 'd';
                    stack.push((pth.clone(), c));
                }
                Kind::Reg(r) => {
                    e.kind = 'f';
                    e.data = read_data(&t, &r.data, r.size);
                }
                Kind::Symlink(x) => {
                    e.kind = 'l';
                    e.target = x.to_vec();
                }
                Kind::Chr(a, b) => {
                    e.kind = 'c';
                    e.dev = (*a, *b);
                }
                Kind::Blk(a, b) => {
                    e.kind = 'b';
                    e.dev = (*a, *b);
                }
                Kind::Fifo => e.kind = 'p',
                Kind::Sock => e.kind = 's',
            }
            out.insert(pth, e);
        }
    }
    out
}

fn read_data(t: &Tree, d: &Data, size: u64) -> Vec<u8> {
    let mut v = vec![0u8; size as usize];
    if let Data::Image { src, bs, map } = d {
        let f = &t.sources[*src as usize].file;
        let bs = *bs as u64;
        for &(l, b, c) in map.iter() {
            let s = l * bs;
            let e = ((l + c) * bs).min(size);
            if s < e {
                f.read_exact_at(&mut v[s as usize..e as usize], b * bs).unwrap();
            }
        }
    }
    v
}

pub fn u32_at(b: &[u8], off: usize) -> u32 {
    u32::from_le_bytes(b[off..off + 4].try_into().unwrap())
}
pub fn u16_at(b: &[u8], off: usize) -> u16 {
    u16::from_le_bytes(b[off..off + 2].try_into().unwrap())
}

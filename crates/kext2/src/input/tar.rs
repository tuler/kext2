//! Tar archives (`-a`, SPEC §5.2): v7, ustar, GNU (long names, old sparse),
//! pax (extended and global headers, sparse 0.0/0.1/1.0), compressed with
//! gzip, bzip2, xz or zstd.

use super::Ctx;
use crate::error::{io_err, show, warn, Error, Result};
use crate::runs::ZeroMapBuilder;
use crate::tree::{Added, Data, Kind, Node, NodeId, Reg, Region};
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{self, BufRead, BufReader, BufWriter, Read, Seek, SeekFrom, Write};

/// Compression format, detected from the first bytes (R-TAR-2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Compression {
    None,
    Gzip,
    Bzip2,
    Xz,
    Zstd,
}

pub fn detect(magic: &[u8]) -> Compression {
    if magic.starts_with(&[0x1F, 0x8B]) {
        Compression::Gzip
    } else if magic.starts_with(b"BZh") {
        Compression::Bzip2
    } else if magic.starts_with(&[0xFD, b'7', b'z', b'X', b'Z', 0x00]) {
        Compression::Xz
    } else if magic.starts_with(&[0x28, 0xB5, 0x2F, 0xFD]) {
        Compression::Zstd
    } else {
        Compression::None
    }
}

/// Wrap `r` in the decoder for `c` (all concatenated members/streams/frames).
pub fn decoder<'a, R: Read + 'a>(c: Compression, r: R) -> io::Result<Box<dyn Read + 'a>> {
    Ok(match c {
        Compression::None => Box::new(r),
        #[cfg(feature = "gzip")]
        Compression::Gzip => Box::new(flate2::read::MultiGzDecoder::new(BufReader::with_capacity(1 << 16, r))),
        #[cfg(feature = "bzip2")]
        Compression::Bzip2 => Box::new(bzip2::read::MultiBzDecoder::new(BufReader::with_capacity(1 << 16, r))),
        #[cfg(feature = "xz")]
        Compression::Xz => Box::new(liblzma::read::XzDecoder::new_multi_decoder(BufReader::with_capacity(1 << 16, r))),
        #[cfg(feature = "zstd")]
        Compression::Zstd => Box::new(zstd::stream::read::Decoder::new(r)?),
        #[allow(unreachable_patterns)]
        other => {
            let _ = BufReader::new(r);
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                format!("{other:?} compression is not supported by this build of kext2"),
            ));
        }
    })
}

/// Decompresses on a separate thread, handing over chunks through a bounded
/// channel, so that decompression overlaps parsing and spooling.
pub struct Pump {
    rx: std::sync::mpsc::Receiver<io::Result<Vec<u8>>>,
    cur: Vec<u8>,
    pos: usize,
    done: bool,
}

impl Pump {
    pub fn new<R: Read + Send + 'static>(c: Compression, r: R) -> Pump {
        let (tx, rx) = std::sync::mpsc::sync_channel::<io::Result<Vec<u8>>>(4);
        std::thread::spawn(move || {
            let mut d = match decoder(c, r) {
                Ok(d) => d,
                Err(e) => {
                    let _ = tx.send(Err(e));
                    return;
                }
            };
            loop {
                let mut v = vec![0u8; 1 << 20];
                let mut got = 0;
                while got < v.len() {
                    match d.read(&mut v[got..]) {
                        Ok(0) => break,
                        Ok(n) => got += n,
                        Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                        Err(e) => {
                            let _ = tx.send(Err(e));
                            return;
                        }
                    }
                }
                v.truncate(got);
                let last = got == 0;
                if tx.send(Ok(v)).is_err() || last {
                    return;
                }
            }
        });
        Pump { rx, cur: Vec::new(), pos: 0, done: false }
    }
}

impl Read for Pump {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        loop {
            if self.pos < self.cur.len() {
                let n = buf.len().min(self.cur.len() - self.pos);
                buf[..n].copy_from_slice(&self.cur[self.pos..self.pos + n]);
                self.pos += n;
                return Ok(n);
            }
            if self.done {
                return Ok(0);
            }
            match self.rx.recv() {
                Ok(Ok(v)) => {
                    if v.is_empty() {
                        self.done = true;
                    }
                    self.cur = v;
                    self.pos = 0;
                }
                Ok(Err(e)) => {
                    self.done = true;
                    return Err(e);
                }
                Err(_) => self.done = true,
            }
        }
    }
}

// ---------------------------------------------------------------- numbers

/// Parse a numeric header field: octal ASCII surrounded by spaces/NULs, or
/// GNU base-256 (R-TAR-4).
#[allow(clippy::result_unit_err)]
pub fn parse_numeric(f: &[u8]) -> std::result::Result<i128, ()> {
    if f.is_empty() {
        return Ok(0);
    }
    if f[0] & 0x80 != 0 {
        // GNU base-256: big-endian two's complement over the remaining bits;
        // 0xFF as first byte means negative.
        if f.len() > 15 {
            return Err(());
        }
        let mut u: i128 = 0;
        for (i, &b) in f.iter().enumerate() {
            let b = if i == 0 && f[0] != 0xFF { b & 0x7F } else { b };
            u = (u << 8) | b as i128;
        }
        if f[0] == 0xFF {
            u -= 1i128 << (8 * f.len() as u32);
        }
        return Ok(u);
    }
    let mut i = 0;
    while i < f.len() && (f[i] == b' ' || f[i] == 0) {
        i += 1;
    }
    let mut v: i128 = 0;
    while i < f.len() && (b'0'..=b'7').contains(&f[i]) {
        v = v.checked_mul(8).ok_or(())? + (f[i] - b'0') as i128;
        i += 1;
    }
    while i < f.len() && (f[i] == b' ' || f[i] == 0) {
        i += 1;
    }
    if i != f.len() {
        return Err(());
    }
    Ok(v)
}

fn parse_dec_u(v: &[u8]) -> Option<u128> {
    if v.is_empty() || !v.iter().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let mut x: u128 = 0;
    for &b in v {
        x = x.checked_mul(10)?.checked_add((b - b'0') as u128)?;
    }
    Some(x)
}

/// Parse a pax time value (may be negative and/or fractional), floored.
pub fn parse_pax_time(v: &[u8]) -> Option<i64> {
    let (neg, rest) = match v.first() {
        Some(b'-') => (true, &v[1..]),
        Some(b'+') => (false, &v[1..]),
        _ => (false, v),
    };
    let (int, frac) = match rest.iter().position(|&b| b == b'.') {
        Some(p) => (&rest[..p], &rest[p + 1..]),
        None => (rest, &b""[..]),
    };
    if !frac.iter().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let i = if int.is_empty() && !frac.is_empty() { 0 } else { parse_dec_u(int)? };
    let i = i64::try_from(i).unwrap_or(i64::MAX);
    let has_frac = frac.iter().any(|&b| b != b'0');
    Some(if neg {
        if has_frac {
            -i - 1
        } else {
            -i
        }
    } else {
        i
    })
}

fn cstr(f: &[u8]) -> &[u8] {
    match f.iter().position(|&b| b == 0) {
        Some(p) => &f[..p],
        None => f,
    }
}

// ---------------------------------------------------------------- input

enum Input<'a> {
    Direct { r: BufReader<File>, pos: u64, len: u64 },
    Stream { r: BufReader<Box<dyn Read + 'a>>, pos: u64 },
}

impl Input<'_> {
    fn pos(&self) -> u64 {
        match self {
            Input::Direct { pos, .. } | Input::Stream { pos, .. } => *pos,
        }
    }
    fn reader(&mut self) -> &mut dyn BufRead {
        match self {
            Input::Direct { r, .. } => r,
            Input::Stream { r, .. } => r,
        }
    }
    fn advance(&mut self, n: u64) {
        match self {
            Input::Direct { pos, .. } | Input::Stream { pos, .. } => *pos += n,
        }
    }
    /// Read up to `buf.len()` bytes, stopping only at end of input.
    fn read_full(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let mut got = 0;
        while got < buf.len() {
            let n = match self.reader().read(&mut buf[got..]) {
                Ok(0) => break,
                Ok(n) => n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            };
            got += n;
        }
        self.advance(got as u64);
        Ok(got)
    }
    fn read_exact(&mut self, buf: &mut [u8]) -> Result<()> {
        let n = self.read_full(buf).map_err(|e| io_err("tar", e))?;
        if n != buf.len() {
            return Err(Error::fatal("unexpected end of tar archive"));
        }
        Ok(())
    }
    fn skip(&mut self, n: u64) -> Result<()> {
        if n == 0 {
            return Ok(());
        }
        match self {
            Input::Direct { r, pos, len } => {
                if *pos + n > *len {
                    return Err(Error::fatal("unexpected end of tar archive"));
                }
                r.seek_relative(n as i64).map_err(|e| io_err("tar", e))?;
                *pos += n;
                Ok(())
            }
            Input::Stream { r, pos } => {
                let c = io::copy(&mut r.by_ref().take(n), &mut io::sink()).map_err(|e| io_err("tar", e))?;
                *pos += c;
                if c != n {
                    return Err(Error::fatal("unexpected end of tar archive"));
                }
                Ok(())
            }
        }
    }
    /// Pass `n` bytes to `f` in chunks.
    fn consume(&mut self, mut n: u64, mut f: impl FnMut(&[u8]) -> Result<()>) -> Result<()> {
        while n > 0 {
            let buf = self.reader().fill_buf().map_err(|e| io_err("tar", e))?;
            if buf.is_empty() {
                return Err(Error::fatal("unexpected end of tar archive"));
            }
            let take = (buf.len() as u64).min(n) as usize;
            f(&buf[..take])?;
            self.reader().consume(take);
            self.advance(take as u64);
            n -= take as u64;
        }
        Ok(())
    }
    fn read_vec(&mut self, n: u64) -> Result<Vec<u8>> {
        if n > (1 << 30) {
            return Err(Error::fatal("tar extended header too large"));
        }
        let mut v = vec![0u8; n as usize];
        self.read_exact(&mut v)?;
        Ok(v)
    }
}

fn pad(n: u64) -> u64 {
    (512 - n % 512) % 512
}

// ---------------------------------------------------------------- pax

#[derive(Default, Clone)]
struct Pax {
    kv: HashMap<Vec<u8>, Vec<u8>>,
    /// GNU sparse 0.0: repeated offset/numbytes pairs, in order.
    sparse00: Vec<(Option<u64>, Option<u64>)>,
}

impl Pax {
    fn get(&self, k: &str) -> Option<&[u8]> {
        self.kv.get(k.as_bytes()).map(|v| &v[..])
    }
}

fn parse_pax(data: &[u8], into: &mut Pax) -> Result<()> {
    let bad = || Error::fatal("malformed pax extended header");
    let mut p = 0;
    while p < data.len() {
        if data[p] == 0 {
            break; // NUL padding
        }
        let sp = data[p..].iter().position(|&b| b == b' ').ok_or_else(bad)? + p;
        let len = parse_dec_u(&data[p..sp]).ok_or_else(bad)? as usize;
        if len == 0 || p + len > data.len() || data[p + len - 1] != b'\n' || sp + 1 > p + len - 1 {
            return Err(bad());
        }
        let rec = &data[sp + 1..p + len - 1];
        let eq = rec.iter().position(|&b| b == b'=').ok_or_else(bad)?;
        let (k, v) = (&rec[..eq], &rec[eq + 1..]);
        match k {
            b"GNU.sparse.offset" => {
                into.sparse00.push((Some(parse_dec_u(v).ok_or_else(bad)? as u64), None));
            }
            b"GNU.sparse.numbytes" => {
                let n = parse_dec_u(v).ok_or_else(bad)? as u64;
                match into.sparse00.last_mut() {
                    Some(last) if last.1.is_none() => last.1 = Some(n),
                    _ => return Err(Error::fatal("pax sparse map: numbytes without offset")),
                }
            }
            _ => {
                into.kv.insert(k.to_vec(), v.to_vec());
            }
        }
        p += len;
    }
    Ok(())
}

// ---------------------------------------------------------------- members

/// A sparse map entry: logical offset and number of bytes.
type MapEnt = (u64, u64);

struct Member {
    typeflag: u8,
    name: Vec<u8>,
    linkname: Vec<u8>,
    mode: u16,
    uid: u64,
    gid: u64,
    mtime: i64,
    /// Bytes of member data in the archive.
    size: u64,
    devmajor: u64,
    devminor: u64,
    /// Sparse members: (map, real size, map stored in data (pax 1.0)).
    sparse: Option<Sparse>,
}

enum Sparse {
    Map {
        map: Vec<MapEnt>,
        real: u64,
    },
    /// pax 1.0: map at the start of the data.
    InData {
        real: u64,
    },
}

fn num_field(h: &[u8], what: &str) -> Result<i128> {
    parse_numeric(h).map_err(|_| Error::fatal(format!("invalid {what} field in tar header")))
}

fn unum(v: i128, what: &str) -> Result<u64> {
    if v < 0 {
        return Err(Error::fatal(format!("negative {what} in tar header")));
    }
    Ok(v.min(u64::MAX as i128) as u64)
}

fn checksum_ok(h: &[u8; 512]) -> bool {
    let stored = match parse_numeric(&h[148..156]) {
        Ok(v) => v,
        Err(_) => return false,
    };
    let mut us: i64 = 0;
    let mut ss: i64 = 0;
    for (i, &b) in h.iter().enumerate() {
        let b = if (148..156).contains(&i) { b' ' } else { b };
        us += b as i64;
        ss += (b as i8) as i64;
    }
    stored == us as i128 || stored == ss as i128
}

/// Parse old-GNU sparse entries (offset 12 bytes, numbytes 12 bytes).
fn gnu_sparse_entries(area: &[u8], out: &mut Vec<MapEnt>) -> Result<()> {
    for e in area.chunks_exact(24) {
        if e.iter().all(|&b| b == 0) {
            continue;
        }
        let off = unum(num_field(&e[..12], "sparse offset")?, "sparse offset")?;
        let n = unum(num_field(&e[12..], "sparse numbytes")?, "sparse numbytes")?;
        out.push((off, n));
    }
    Ok(())
}

fn parse_map_list(v: &[u8]) -> Result<Vec<MapEnt>> {
    let bad = || Error::fatal("malformed GNU.sparse.map");
    let nums: Vec<u64> = if v.is_empty() {
        vec![]
    } else {
        v.split(|&b| b == b',').map(|s| parse_dec_u(s).map(|x| x as u64).ok_or_else(bad)).collect::<Result<_>>()?
    };
    if !nums.len().is_multiple_of(2) {
        return Err(bad());
    }
    Ok(nums.chunks(2).map(|c| (c[0], c[1])).collect())
}

/// Validate a sparse map and turn it into regions at `src_off`.
fn regions(map: &[MapEnt], real: u64, src_off: u64, avail: u64) -> Result<Vec<Region>> {
    let mut out = Vec::new();
    let mut end = 0u64;
    let mut cum = 0u64;
    for &(off, n) in map {
        let e = off.checked_add(n).ok_or_else(|| Error::fatal("invalid sparse map"))?;
        if off < end || e > real {
            return Err(Error::fatal("invalid sparse map: regions overlap, are out of order or exceed the size"));
        }
        end = e;
        if n > 0 {
            out.push(Region { off, len: n, src_off: src_off + cum });
        }
        cum += n;
    }
    if cum > avail {
        return Err(Error::fatal("invalid sparse map: more data than the member holds"));
    }
    Ok(out)
}

// ---------------------------------------------------------------- spool

struct Spool {
    w: BufWriter<File>,
    len: u64,
    src: u32,
}

// ---------------------------------------------------------------- reader

/// Copy tar archive `src` (`-` = standard input) into image directory `target`.
pub fn add_tar(ctx: &mut Ctx, src: &std::ffi::OsStr, target: NodeId) -> Result<()> {
    let name = src.to_string_lossy().into_owned();
    let mut magic = [0u8; 6];
    if src == "-" {
        let mut stdin = io::stdin();
        let n = read_up_to(&mut stdin, &mut magic).map_err(|e| io_err("standard input", e))?;
        let comp = detect(&magic[..n]);
        let chained = io::Cursor::new(magic[..n].to_vec()).chain(stdin);
        let r = Pump::new(comp, chained);
        let input = Input::Stream { r: BufReader::with_capacity(1 << 20, Box::new(r)), pos: 0 };
        return read_archive(ctx, input, None, target, &name);
    }
    let bad = || Error::fatal(format!("{name} should be a file"));
    let md = std::fs::metadata(src).map_err(|_| bad())?;
    if !md.is_file() {
        return Err(bad());
    }
    let mut f = File::open(src).map_err(|e| io_err(&name, e))?;
    let n = read_up_to(&mut f, &mut magic).map_err(|e| io_err(&name, e))?;
    f.seek(SeekFrom::Start(0)).map_err(|e| io_err(&name, e))?;
    let comp = detect(&magic[..n]);
    if comp == Compression::None {
        let srcf = f.try_clone().map_err(|e| io_err(&name, e))?;
        let id = ctx.tree.add_source(srcf, name.clone());
        let input = Input::Direct { r: BufReader::with_capacity(1 << 16, f), pos: 0, len: md.len() };
        read_archive(ctx, input, Some(id), target, &name)
    } else {
        let r = Pump::new(comp, f);
        let input = Input::Stream { r: BufReader::with_capacity(1 << 20, Box::new(r)), pos: 0 };
        read_archive(ctx, input, None, target, &name)
    }
}

fn read_up_to(r: &mut impl Read, buf: &mut [u8]) -> io::Result<usize> {
    let mut got = 0;
    while got < buf.len() {
        match r.read(&mut buf[got..]) {
            Ok(0) => break,
            Ok(n) => got += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(got)
}

fn read_archive(ctx: &mut Ctx, mut input: Input, direct: Option<u32>, target: NodeId, name: &str) -> Result<()> {
    let ctxerr = |e: Error| Error::fatal(format!("{name}: {}", e.msg));
    let mut spool: Option<Spool> = None;
    let r = read_members(ctx, &mut input, direct, &mut spool, target);
    if let Some(s) = spool.as_mut() {
        s.w.flush().map_err(|e| io_err("spool file", e))?;
    }
    r.map_err(ctxerr)
}

fn read_members(
    ctx: &mut Ctx,
    input: &mut Input,
    direct: Option<u32>,
    spool: &mut Option<Spool>,
    target: NodeId,
) -> Result<()> {
    let mut global = Pax::default();
    let mut ignored: HashSet<Vec<u8>> = HashSet::new();
    let mut ext: Option<Pax> = None;
    let mut long_name: Option<Vec<u8>> = None;
    let mut long_link: Option<Vec<u8>> = None;
    let mut h = [0u8; 512];
    loop {
        let n = input.read_full(&mut h).map_err(|e| io_err("read", e))?;
        if n == 0 && ext.is_none() && long_name.is_none() && long_link.is_none() {
            break; // end of input at a header boundary
        }
        if n != 512 {
            return Err(Error::fatal("unexpected end of tar archive"));
        }
        if h.iter().all(|&b| b == 0) {
            break;
        }
        if !checksum_ok(&h) {
            return Err(Error::fatal(format!("bad header checksum at offset {}", input.pos() - 512)));
        }
        let typeflag = h[156];
        let size = unum(num_field(&h[124..136], "size")?, "size")?;
        match typeflag {
            b'L' | b'K' => {
                let d = input.read_vec(size)?;
                input.skip(pad(size))?;
                let v = cstr(&d).to_vec();
                if typeflag == b'L' {
                    long_name = Some(v);
                } else {
                    long_link = Some(v);
                }
                continue;
            }
            b'x' | b'g' => {
                let d = input.read_vec(size)?;
                input.skip(pad(size))?;
                if typeflag == b'g' {
                    parse_pax(&d, &mut global)?;
                } else {
                    let mut x = Pax::default();
                    parse_pax(&d, &mut x)?;
                    ext = Some(x);
                }
                continue;
            }
            _ => {}
        }
        let x = ext.take().unwrap_or_default();
        let ln = long_name.take();
        let ll = long_link.take();
        let m = build_member(&h, &x, &global, ln, ll, input)?;
        process_member(ctx, input, direct, spool, target, m, &mut ignored)?;
    }
    Ok(())
}

fn build_member(
    h: &[u8; 512],
    x: &Pax,
    g: &Pax,
    long_name: Option<Vec<u8>>,
    long_link: Option<Vec<u8>>,
    input: &mut Input,
) -> Result<Member> {
    let posix = &h[257..263] == b"ustar\0";
    let gnu = &h[257..263] == b"ustar " && &h[263..265] == b" \0";
    let typeflag = h[156];
    let mut name = cstr(&h[0..100]).to_vec();
    if posix {
        let prefix = cstr(&h[345..500]);
        if !prefix.is_empty() {
            let mut p = prefix.to_vec();
            p.push(b'/');
            p.extend_from_slice(&name);
            name = p;
        }
    }
    if let Some(l) = long_name {
        name = l;
    }
    let mut linkname = cstr(&h[157..257]).to_vec();
    if let Some(l) = long_link {
        linkname = l;
    }
    let pick = |k: &str| x.get(k).or_else(|| g.get(k));
    if let Some(v) = pick("path") {
        name = v.to_vec();
    }
    if let Some(v) = pick("linkpath") {
        linkname = v.to_vec();
    }
    let bad = |k: &str| Error::fatal(format!("invalid pax value for {k}"));
    let mode = (unum(num_field(&h[100..108], "mode")?, "mode")? & 0o7777) as u16;
    let mut uid = unum(num_field(&h[108..116], "uid")?, "uid")?;
    let mut gid = unum(num_field(&h[116..124], "gid")?, "gid")?;
    let mt = num_field(&h[136..148], "mtime")?;
    let mut mtime = mt.clamp(i64::MIN as i128, i64::MAX as i128) as i64;
    let mut size = unum(num_field(&h[124..136], "size")?, "size")?;
    if let Some(v) = pick("uid") {
        uid = parse_dec_u(v).ok_or_else(|| bad("uid"))?.min(u64::MAX as u128) as u64;
    }
    if let Some(v) = pick("gid") {
        gid = parse_dec_u(v).ok_or_else(|| bad("gid"))?.min(u64::MAX as u128) as u64;
    }
    if let Some(v) = pick("mtime") {
        mtime = parse_pax_time(v).ok_or_else(|| bad("mtime"))?;
    }
    if let Some(v) = pick("size") {
        size = parse_dec_u(v).ok_or_else(|| bad("size"))?.min(u64::MAX as u128) as u64;
    }
    let (devmajor, devminor) = if posix || gnu {
        (
            unum(num_field(&h[329..337], "devmajor")?, "devmajor")?,
            unum(num_field(&h[337..345], "devminor")?, "devminor")?,
        )
    } else {
        (0, 0)
    };
    // sparse
    let mut sparse = None;
    if typeflag == b'S' && gnu {
        let mut map = Vec::new();
        gnu_sparse_entries(&h[386..482], &mut map)?;
        let real = unum(num_field(&h[483..495], "real size")?, "real size")?;
        let mut more = h[482] != 0;
        while more {
            let mut e = [0u8; 512];
            input.read_exact(&mut e)?;
            gnu_sparse_entries(&e[..504], &mut map)?;
            more = e[504] != 0;
        }
        sparse = Some(Sparse::Map { map, real });
    } else if typeflag == b'S' {
        return Err(Error::fatal("sparse member without GNU header"));
    } else if matches!(typeflag, b'0' | 0 | b'7') {
        let realsize = |k: &str| -> Result<u64> {
            let v = pick(k).ok_or_else(|| Error::fatal(format!("pax sparse member without {k}")))?;
            Ok(parse_dec_u(v).ok_or_else(|| bad(k))?.min(u64::MAX as u128) as u64)
        };
        if pick("GNU.sparse.major") == Some(b"1") {
            if pick("GNU.sparse.minor").unwrap_or(b"0") != b"0" {
                return Err(Error::fatal("unsupported GNU sparse format version"));
            }
            sparse = Some(Sparse::InData { real: realsize("GNU.sparse.realsize")? });
        } else if let Some(v) = pick("GNU.sparse.map") {
            sparse = Some(Sparse::Map { map: parse_map_list(v)?, real: realsize("GNU.sparse.size")? });
        } else if !x.sparse00.is_empty() {
            let mut map = Vec::new();
            for &(o, n) in &x.sparse00 {
                match (o, n) {
                    (Some(o), Some(n)) => map.push((o, n)),
                    _ => return Err(Error::fatal("pax sparse map: offset without numbytes")),
                }
            }
            sparse = Some(Sparse::Map { map, real: realsize("GNU.sparse.size")? });
        }
        if sparse.is_some() {
            if let Some(v) = pick("GNU.sparse.name") {
                name = v.to_vec();
            }
        }
    }
    Ok(Member { typeflag, name, linkname, mode, uid, gid, mtime, size, devmajor, devminor, sparse })
}

/// Normalise a member name (R-TAR-8). `None` if it has a `..` component.
pub fn normalize(name: &[u8]) -> Option<Vec<&[u8]>> {
    let mut v = Vec::new();
    for c in name.split(|&b| b == b'/') {
        match c {
            b"" | b"." => {}
            b".." => return None,
            c => v.push(c),
        }
    }
    Some(v)
}

fn sat32(v: u64) -> u32 {
    v.min(u32::MAX as u64) as u32
}

fn process_member(
    ctx: &mut Ctx,
    input: &mut Input,
    direct: Option<u32>,
    spool: &mut Option<Spool>,
    target: NodeId,
    m: Member,
    ignored: &mut HashSet<Vec<u8>>,
) -> Result<()> {
    let data_size = m.size;
    let t = m.typeflag;
    if t == b'M' {
        return Err(Error::fatal("multi-volume archives are not supported"));
    }
    if t == b'V' {
        return input.skip(data_size + pad(data_size));
    }
    let is_dir =
        t == b'5' || t == b'D' || (matches!(t, b'0' | 0 | b'7') && m.name.ends_with(b"/") && m.sparse.is_none());
    let comps = match normalize(&m.name) {
        Some(c) => c,
        None => {
            warn(format!("skipping tar member {}: '..' in path", show(&m.name)));
            return input.skip(data_size + pad(data_size));
        }
    };
    let uid = sat32(m.uid);
    let gid = sat32(m.gid);
    if comps.is_empty() {
        if is_dir {
            let mut n = Node::dir(m.mode, uid, gid, m.mtime);
            ctx.squash(&mut n);
            ctx.tree.merge_meta(target, n.perm, n.uid, n.gid, n.mtime);
        } else {
            warn(format!("skipping tar member with an empty name ({})", show(&m.name)));
        }
        return input.skip(data_size + pad(data_size));
    }
    let (base, dirs) = comps.split_last().unwrap();
    // members below a directory member that was ignored (R-IN-3 step 4)
    if !ignored.is_empty() && (1..=dirs.len()).any(|i| ignored.contains(&comps[..i].join(&b'/'))) {
        warn(format!("ignoring {}: below an ignored duplicate entry", show(&m.name)));
        return input.skip(data_size + pad(data_size));
    }
    let parent = ctx.tree.ensure_dirs(target, dirs, &m.name)?;
    let regular = !is_dir && !matches!(t, b'1' | b'2' | b'3' | b'4' | b'6');
    if t == b'1' {
        let lc = match normalize(&m.linkname) {
            Some(c) => c,
            None => {
                warn(format!("skipping hard link {}: '..' in target", show(&m.name)));
                return input.skip(data_size + pad(data_size));
            }
        };
        let tgt = ctx.tree.lookup(target, &lc).ok_or_else(|| {
            Error::fatal(format!("hard link {}: target {} not found", show(&m.name), show(&m.linkname)))
        })?;
        if ctx.tree.node(tgt).is_dir() {
            return Err(Error::fatal(format!(
                "hard link {}: target {} is a directory",
                show(&m.name),
                show(&m.linkname)
            )));
        }
        ctx.tree.add_link(parent, base, tgt);
        return input.skip(data_size + pad(data_size));
    }
    let kind = if is_dir {
        Kind::Dir(Box::default())
    } else {
        match t {
            b'2' => Kind::Symlink(m.linkname.clone().into()),
            b'3' => Kind::Chr(sat32(m.devmajor), sat32(m.devminor)),
            b'4' => Kind::Blk(sat32(m.devmajor), sat32(m.devminor)),
            b'6' => Kind::Fifo,
            _ => {
                if !matches!(t, b'0' | 0 | b'7' | b'S') {
                    warn(format!(
                        "tar member {}: unknown type '{}', treated as a regular file",
                        show(&m.name),
                        t as char
                    ));
                }
                Kind::Reg(Box::new(Reg { size: 0, data: Data::Zero, zmap: None }))
            }
        }
    };
    let perm = if matches!(kind, Kind::Symlink(_)) { 0o777 } else { m.mode };
    let mut node = Node::new(kind, perm, uid, gid, m.mtime);
    ctx.squash(&mut node);
    let added = ctx.tree.add_entry(parent, base, node);
    if is_dir && added == Added::Ignored {
        ignored.insert(comps.join(&b'/'));
    }
    let id = match added {
        Added::New(id) if regular => id,
        _ => return input.skip(data_size + pad(data_size)),
    };
    // regular file data
    let mut consumed = 0u64;
    let (map, real) = match m.sparse {
        None => (vec![(0, data_size)], data_size),
        Some(Sparse::Map { map, real }) => (map, real),
        Some(Sparse::InData { real }) => {
            let (map, used) = read_map_in_data(input, data_size)?;
            consumed = used;
            (map, real)
        }
    };
    let avail = data_size - consumed;
    let start = input.pos();
    let (src, base_off) = match direct {
        Some(src) => (src, start),
        None => {
            if spool.is_none() {
                let f = tempfile::tempfile().map_err(|e| io_err("cannot create spool file", e))?;
                let r = f.try_clone().map_err(|e| io_err("spool file", e))?;
                let src = ctx.tree.add_source(r, "spool".into());
                *spool = Some(Spool { w: BufWriter::with_capacity(1 << 20, f), len: 0, src });
            }
            let s = spool.as_ref().unwrap();
            (s.src, s.len)
        }
    };
    let regs = regions(&map, real, base_off, avail)?;
    let stored: u64 = regs.iter().map(|r| r.len).sum();
    let mut z = if ctx.holes { Some(ZeroMapBuilder::new(ctx.bs)) } else { None };
    if direct.is_none() || z.is_some() {
        // stream the stored data: to the spool and/or the zero map
        let mut ri = 0usize;
        let mut in_reg = 0u64;
        let sp = spool.as_mut();
        let mut sp = if direct.is_none() { sp } else { None };
        input.consume(stored, |chunk| {
            if let Some(s) = sp.as_mut() {
                s.w.write_all(chunk).map_err(|e| io_err("spool file", e))?;
                s.len += chunk.len() as u64;
            }
            if let Some(z) = z.as_mut() {
                let mut c = chunk;
                while !c.is_empty() {
                    let r = &regs[ri];
                    let take = ((r.len - in_reg) as usize).min(c.len());
                    z.feed(r.off + in_reg, &c[..take]);
                    in_reg += take as u64;
                    c = &c[take..];
                    if in_reg == r.len {
                        ri += 1;
                        in_reg = 0;
                    }
                }
            }
            Ok(())
        })?;
    } else {
        input.skip(stored)?;
    }
    input.skip(avail - stored + pad(data_size))?;
    if let Kind::Reg(r) = &mut ctx.tree.node_mut(id).kind {
        r.size = real;
        r.data = if regs.is_empty() { Data::Zero } else { Data::Source { src, regions: regs.into() } };
        r.zmap = z.map(|z| Box::new(z.finish()));
    }
    Ok(())
}

/// pax sparse 1.0: read the decimal map at the start of the member data.
/// Returns the map and the number of bytes it occupies (a multiple of 512).
fn read_map_in_data(input: &mut Input, data_size: u64) -> Result<(Vec<MapEnt>, u64)> {
    let bad = || Error::fatal("malformed GNU sparse 1.0 map");
    let mut nums: Vec<u64> = Vec::new();
    let mut want: Option<u64> = None;
    let mut cur: Vec<u8> = Vec::new();
    let mut used = 0u64;
    let mut blk = [0u8; 512];
    loop {
        if used + 512 > data_size {
            return Err(bad());
        }
        input.read_exact(&mut blk)?;
        used += 512;
        for &b in blk.iter() {
            if b == b'\n' {
                let v = parse_dec_u(&cur).ok_or_else(bad)?;
                cur.clear();
                nums.push(u64::try_from(v).map_err(|_| bad())?);
                if want.is_none() {
                    let c = nums[0];
                    want = Some(c.checked_mul(2).and_then(|x| x.checked_add(1)).ok_or_else(bad)?);
                }
                if Some(nums.len() as u64) == want {
                    break;
                }
            } else {
                cur.push(b);
                if cur.len() > 40 {
                    return Err(bad());
                }
            }
        }
        if Some(nums.len() as u64) == want {
            break;
        }
    }
    let map = nums[1..].chunks(2).map(|c| (c[0], c[1])).collect();
    Ok((map, used))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn numeric_fields() {
        assert_eq!(parse_numeric(b"0000644\0").unwrap(), 0o644);
        assert_eq!(parse_numeric(b"   755 \0").unwrap(), 0o755);
        assert_eq!(parse_numeric(b"\0\0\0\0").unwrap(), 0);
        assert!(parse_numeric(b"12x4").is_err());
        let mut f = [0u8; 12];
        f[0] = 0x80;
        f[11] = 0x10;
        f[10] = 0x01;
        assert_eq!(parse_numeric(&f).unwrap(), 0x110);
        let f = [0xFFu8; 8];
        assert_eq!(parse_numeric(&f).unwrap(), -1);
        let mut f = [0xFFu8; 12];
        f[11] = 0x18;
        f[10] = 0xFC; // -1000 = ...FC18
        assert_eq!(parse_numeric(&f).unwrap(), -1000);
    }

    #[test]
    fn pax_times() {
        assert_eq!(parse_pax_time(b"1792179200.92920064"), Some(1792179200));
        assert_eq!(parse_pax_time(b"-1.5"), Some(-2));
        assert_eq!(parse_pax_time(b"-1000"), Some(-1000));
        assert_eq!(parse_pax_time(b"-1000.000"), Some(-1000));
        assert_eq!(parse_pax_time(b"12abc"), None);
    }

    #[test]
    fn names() {
        assert_eq!(normalize(b"./a//b/"), Some(vec![&b"a"[..], &b"b"[..]]));
        assert_eq!(normalize(b"/abs/x"), Some(vec![&b"abs"[..], &b"x"[..]]));
        assert_eq!(normalize(b"x/../z"), None);
        assert_eq!(normalize(b"./"), Some(vec![]));
    }

    #[test]
    fn magic() {
        assert_eq!(detect(&[0x1f, 0x8b, 8]), Compression::Gzip);
        assert_eq!(detect(b"BZh91"), Compression::Bzip2);
        assert_eq!(detect(&[0xFD, b'7', b'z', b'X', b'Z', 0]), Compression::Xz);
        assert_eq!(detect(&[0x28, 0xB5, 0x2F, 0xFD]), Compression::Zstd);
        assert_eq!(detect(b"ustar"), Compression::None);
    }

    #[test]
    fn sparse_maps() {
        assert!(regions(&[(10, 5), (12, 1)], 100, 0, 10).is_err());
        assert!(regions(&[(10, 5)], 12, 0, 10).is_err());
        let r = regions(&[(10, 5), (20, 0), (30, 2)], 100, 7, 7).unwrap();
        assert_eq!(r, vec![Region { off: 10, len: 5, src_off: 7 }, Region { off: 30, len: 2, src_off: 12 }]);
        assert_eq!(parse_map_list(b"102400,3072,512000,2048,2097152,0").unwrap().len(), 3);
    }
}

//! Command line (SPEC §2): GNU `getopt_long` semantics and option validation.

use crate::error::{Error, Result};
use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::{OsStrExt, OsStringExt};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub const USAGE: &str = "\
Usage: kext2 [options] image
Create an ext2 filesystem image from directories, tar archives and device
tables, without root privileges ('-' as image writes to standard output).

  -x, --starting-image <image>      Start from the content of an existing image
  -d, --root <directory>[:path]     Copy a local directory into path (or root)
  -a, --tarball <file>[:path]       Copy a tar archive (plain, gzip, bzip2, xz or
                                    zstd; '-' = stdin) into path (or root)
  -D, --devtable <file>[:path]      Add or fix up nodes from a device table
  -B, --block-size <bytes>          1024 (default), 2048 or 4096
  -b, --size-in-blocks <blocks>     Exact image size (computed otherwise)
  -r, --readjustment <[+]blocks>    +N: N free blocks added to the computed
                                    size; N: exactly N blocks (as -b N)
  -i, --bytes-per-inode <bytes>     Inode density (only together with -b)
  -N, --number-of-inodes <count>    Minimum number of inodes
  -L, --volume-label <string>       Volume label (up to 16 bytes)
  -m, --reserved-percentage <pct>   Reserved blocks percentage (default 5)
  -o, --creator-os <os>             'linux' (default), 'hurd', 'freebsd',
                                    'lites' or a number
  -g, --block-map <path>            Write a block map file for this path
  -e, --fill-value <value>          Fill unallocated blocks with this byte
  -z, --allow-holes                 Store all-zero blocks as holes
  -f, --faketime                    Every timestamp is 0
  -q, --squash <uid>                Same as \"-U <uid> -P\"
  -U, --squash-uids <uid>           Every file is owned by <uid>:<uid>
  -P, --squash-perms                Clear group and other permissions
      --timestamp <secs>            Every timestamp is <secs>
      --clamp-timestamps <secs>     Filesystem time <secs>; later file times
                                    are clamped to it
  -h, --help                        Show this help
  -V, --version                     Show the version
  -v, --verbose                     Describe the image built

Without a timestamp option the filesystem time is $SOURCE_DATE_EPOCH or the
current time.
";

/// Kind of input layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayerKind {
    Dir,
    Tar,
    DevTable,
}

/// One `-d`, `-a` or `-D` argument.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layer {
    pub kind: LayerKind,
    /// Source path (`-` = standard input for tar and device tables).
    pub src: OsString,
    /// Target directory in the image, as components (empty = root).
    pub path: Vec<Vec<u8>>,
}

/// `-r` (SPEC Appendix C.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Readjust {
    /// `-r +N`: N extra free blocks on top of the automatic size.
    Extra(u64),
    /// `-r N`: exactly N blocks, as if `-b N` had been given.
    Absolute(u64),
}

/// Timestamp policy (R-TIME-1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeMode {
    /// Use `SOURCE_DATE_EPOCH` or the wall clock; file times from the inputs.
    Default,
    /// `-f`: every time is 0.
    Fake,
    /// `--timestamp S`.
    Fixed(i64),
    /// `--clamp-timestamps S`.
    Clamp(i64),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    pub image: OsString,
    pub starting_image: Option<OsString>,
    pub layers: Vec<Layer>,
    pub block_size: u32,
    pub block_size_set: bool,
    pub blocks: Option<u64>,
    pub readjust: Option<Readjust>,
    pub bytes_per_inode: Option<u64>,
    pub inodes: Option<u64>,
    pub label: Option<Vec<u8>>,
    pub reserved_pct: u32,
    pub reserved_set: bool,
    pub creator_os: u32,
    pub creator_os_set: bool,
    pub block_maps: Vec<Vec<u8>>,
    pub fill: u8,
    pub holes: bool,
    pub time: TimeMode,
    pub squash_uid: Option<u32>,
    pub squash_perms: bool,
    pub verbose: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            image: OsString::new(),
            starting_image: None,
            layers: Vec::new(),
            block_size: 1024,
            block_size_set: false,
            blocks: None,
            readjust: None,
            bytes_per_inode: None,
            inodes: None,
            label: None,
            reserved_pct: 5,
            reserved_set: false,
            creator_os: 0,
            creator_os_set: false,
            block_maps: Vec::new(),
            fill: 0,
            holes: false,
            time: TimeMode::Default,
            squash_uid: None,
            squash_perms: false,
            verbose: false,
        }
    }
}

/// Result of parsing the command line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Run(Box<Options>),
    Help,
    Version,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum HasArg {
    No,
    Required,
}

struct LongOpt {
    name: &'static str,
    has_arg: HasArg,
    /// Short-option character or a pseudo character for long-only options.
    val: char,
}

const TIMESTAMP: char = '\u{1}';
const CLAMP: char = '\u{2}';

const LONG: &[LongOpt] = &[
    LongOpt { name: "starting-image", has_arg: HasArg::Required, val: 'x' },
    LongOpt { name: "root", has_arg: HasArg::Required, val: 'd' },
    LongOpt { name: "devtable", has_arg: HasArg::Required, val: 'D' },
    LongOpt { name: "tarball", has_arg: HasArg::Required, val: 'a' },
    LongOpt { name: "block-size", has_arg: HasArg::Required, val: 'B' },
    LongOpt { name: "size-in-blocks", has_arg: HasArg::Required, val: 'b' },
    LongOpt { name: "readjustment", has_arg: HasArg::Required, val: 'r' },
    LongOpt { name: "bytes-per-inode", has_arg: HasArg::Required, val: 'i' },
    LongOpt { name: "number-of-inodes", has_arg: HasArg::Required, val: 'N' },
    LongOpt { name: "volume-label", has_arg: HasArg::Required, val: 'L' },
    LongOpt { name: "reserved-percentage", has_arg: HasArg::Required, val: 'm' },
    LongOpt { name: "creator-os", has_arg: HasArg::Required, val: 'o' },
    LongOpt { name: "block-map", has_arg: HasArg::Required, val: 'g' },
    LongOpt { name: "fill-value", has_arg: HasArg::Required, val: 'e' },
    LongOpt { name: "allow-holes", has_arg: HasArg::No, val: 'z' },
    LongOpt { name: "faketime", has_arg: HasArg::No, val: 'f' },
    LongOpt { name: "squash", has_arg: HasArg::Required, val: 'q' },
    LongOpt { name: "squash-uids", has_arg: HasArg::Required, val: 'U' },
    LongOpt { name: "squash-perms", has_arg: HasArg::No, val: 'P' },
    LongOpt { name: "help", has_arg: HasArg::No, val: 'h' },
    LongOpt { name: "version", has_arg: HasArg::No, val: 'V' },
    LongOpt { name: "verbose", has_arg: HasArg::No, val: 'v' },
    LongOpt { name: "timestamp", has_arg: HasArg::Required, val: TIMESTAMP },
    LongOpt { name: "clamp-timestamps", has_arg: HasArg::Required, val: CLAMP },
];

const SHORT: &str = "x:d:D:a:B:b:r:i:N:L:m:o:g:e:zfq:U:PhVv";

fn short_has_arg(c: char) -> Option<HasArg> {
    let s = SHORT.as_bytes();
    let i = s.iter().position(|&b| b as char == c && b != b':')?;
    Some(if s.get(i + 1) == Some(&b':') { HasArg::Required } else { HasArg::No })
}

fn opt_name(c: char) -> String {
    match c {
        TIMESTAMP => "--timestamp".into(),
        CLAMP => "--clamp-timestamps".into(),
        c => {
            let long = LONG.iter().find(|l| l.val == c).map(|l| l.name).unwrap_or("?");
            format!("-{c}/--{long}")
        }
    }
}

/// Parse an unsigned decimal number in `lo..=hi` (R-CLI-4).
pub fn parse_num(opt: char, arg: &[u8], lo: u64, hi: u64) -> Result<u64> {
    let bad = || {
        Error::usage(format!(
            "invalid value '{}' for {}: expected a decimal number in {lo}..{hi}",
            String::from_utf8_lossy(arg),
            opt_name(opt)
        ))
    };
    if arg.is_empty() || !arg.iter().all(|b| b.is_ascii_digit()) {
        return Err(bad());
    }
    let mut v: u64 = 0;
    for &b in arg {
        v = v.checked_mul(10).and_then(|v| v.checked_add((b - b'0') as u64)).ok_or_else(bad)?;
    }
    if v < lo || v > hi {
        return Err(bad());
    }
    Ok(v)
}

/// Split `src[:path]` at the first `:` and parse the image path (R-CLI-6).
pub fn parse_layer(kind: LayerKind, arg: &[u8]) -> Result<Layer> {
    let (src, path) = match arg.iter().position(|&b| b == b':') {
        Some(i) => (&arg[..i], &arg[i + 1..]),
        None => (arg, &b""[..]),
    };
    let path = parse_image_path(path).map_err(|_| {
        Error::usage(format!("invalid image path in '{}': '..' is not allowed", String::from_utf8_lossy(arg)))
    })?;
    Ok(Layer { kind, src: OsString::from_vec(src.to_vec()), path })
}

/// Split an image path into components, dropping empty and `.` components.
/// A `..` component is an error.
#[allow(clippy::result_unit_err)]
pub fn parse_image_path(path: &[u8]) -> std::result::Result<Vec<Vec<u8>>, ()> {
    let mut out = Vec::new();
    for c in path.split(|&b| b == b'/') {
        match c {
            b"" | b"." => {}
            b".." => return Err(()),
            c => out.push(c.to_vec()),
        }
    }
    Ok(out)
}

fn parse_os(arg: &[u8]) -> Result<u32> {
    let lower: Vec<u8> = arg.to_ascii_lowercase();
    match &lower[..] {
        b"linux" => Ok(0),
        b"hurd" => Ok(1),
        b"freebsd" => Ok(3),
        b"lites" => Ok(4),
        _ => {
            if !arg.is_empty() && arg.iter().all(|b| b.is_ascii_digit()) {
                Ok(parse_num('o', arg, 0, u32::MAX as u64)? as u32)
            } else {
                Err(Error::usage(format!(
                    "invalid creator OS '{}': expected linux, hurd, freebsd, lites or a number",
                    String::from_utf8_lossy(arg)
                )))
            }
        }
    }
}

#[derive(Default)]
struct TimeSeen {
    fake: bool,
    fixed: bool,
    clamp: bool,
}

fn apply(o: &mut Options, ts: &mut TimeSeen, c: char, arg: Option<&[u8]>) -> Result<Option<Action>> {
    let a = || arg.unwrap_or(b"");
    match c {
        'x' => {
            if o.starting_image.is_some() {
                return Err(Error::usage("-x/--starting-image may be given only once"));
            }
            o.starting_image = Some(OsStr::from_bytes(a()).to_os_string());
        }
        'd' => o.layers.push(parse_layer(LayerKind::Dir, a())?),
        'a' => o.layers.push(parse_layer(LayerKind::Tar, a())?),
        'D' => o.layers.push(parse_layer(LayerKind::DevTable, a())?),
        'B' => {
            let v = a();
            let bs = match v {
                b"1024" => 1024,
                b"2048" => 2048,
                b"4096" => 4096,
                _ => return Err(Error::usage("Valid block sizes: 1024, 2048 or 4096.")),
            };
            o.block_size = bs;
            o.block_size_set = true;
        }
        'b' => o.blocks = Some(parse_num(c, a(), 1, u32::MAX as u64)?),
        'r' => {
            let v = a();
            o.readjust = Some(match v.strip_prefix(b"+") {
                Some(n) => Readjust::Extra(parse_num(c, n, 0, u32::MAX as u64)?),
                None => Readjust::Absolute(parse_num(c, v, 1, u32::MAX as u64)?),
            });
        }
        'i' => o.bytes_per_inode = Some(parse_num(c, a(), 1, u64::MAX)?),
        'N' => o.inodes = Some(parse_num(c, a(), 0, u32::MAX as u64)?),
        'L' => {
            let mut l = a().to_vec();
            if l.len() > 16 {
                crate::error::warn(format!("volume label '{}' truncated to 16 bytes", String::from_utf8_lossy(&l)));
                l.truncate(16);
            }
            o.label = Some(l);
        }
        'm' => {
            o.reserved_pct = parse_num(c, a(), 0, 100)? as u32;
            o.reserved_set = true;
        }
        'o' => {
            o.creator_os = parse_os(a())?;
            o.creator_os_set = true;
        }
        'g' => o.block_maps.push(a().to_vec()),
        'e' => o.fill = parse_num(c, a(), 0, 255)? as u8,
        'z' => o.holes = true,
        'f' => {
            ts.fake = true;
            o.time = TimeMode::Fake;
        }
        'q' => {
            o.squash_uid = Some(parse_num(c, a(), 0, 4_294_967_294)? as u32);
            o.squash_perms = true;
        }
        'U' => o.squash_uid = Some(parse_num(c, a(), 0, 4_294_967_294)? as u32),
        'P' => o.squash_perms = true,
        'h' => return Ok(Some(Action::Help)),
        'V' => return Ok(Some(Action::Version)),
        'v' => o.verbose = true,
        TIMESTAMP => {
            ts.fixed = true;
            o.time = TimeMode::Fixed(parse_num(c, a(), 0, i32::MAX as u64)? as i64);
        }
        CLAMP => {
            ts.clamp = true;
            o.time = TimeMode::Clamp(parse_num(c, a(), 0, i32::MAX as u64)? as i64);
        }
        _ => unreachable!("unknown option character"),
    }
    Ok(None)
}

/// Parse the arguments (without the program name).
pub fn parse<I, S>(args: I) -> Result<Action>
where
    I: IntoIterator<Item = S>,
    S: Into<OsString>,
{
    let args: Vec<Vec<u8>> = args.into_iter().map(|s| s.into().into_vec()).collect();
    let mut o = Options::default();
    let mut ts = TimeSeen::default();
    let mut operands: Vec<Vec<u8>> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        i += 1;
        if arg == b"--" {
            operands.extend(args[i..].iter().cloned());
            break;
        }
        if arg.len() > 2 && arg.starts_with(b"--") {
            let body = &arg[2..];
            let (name, value) = match body.iter().position(|&b| b == b'=') {
                Some(p) => (&body[..p], Some(&body[p + 1..])),
                None => (body, None),
            };
            let exact = LONG.iter().find(|l| l.name.as_bytes() == name);
            let opt = match exact {
                Some(l) => l,
                None => {
                    let m: Vec<&LongOpt> = LONG.iter().filter(|l| l.name.as_bytes().starts_with(name)).collect();
                    match m.len() {
                        0 => {
                            return Err(Error::usage(format!(
                                "unrecognized option '--{}'",
                                String::from_utf8_lossy(name)
                            )))
                        }
                        1 => m[0],
                        _ => {
                            return Err(Error::usage(format!(
                                "option '--{}' is ambiguous; possibilities: {}",
                                String::from_utf8_lossy(name),
                                m.iter().map(|l| format!("'--{}'", l.name)).collect::<Vec<_>>().join(" ")
                            )))
                        }
                    }
                }
            };
            let optarg: Option<Vec<u8>> = match (opt.has_arg, value) {
                (HasArg::No, Some(_)) => {
                    return Err(Error::usage(format!("option '--{}' doesn't allow an argument", opt.name)))
                }
                (HasArg::No, None) => None,
                (HasArg::Required, Some(v)) => Some(v.to_vec()),
                (HasArg::Required, None) => {
                    if i < args.len() {
                        i += 1;
                        Some(args[i - 1].clone())
                    } else {
                        return Err(Error::usage(format!("option '--{}' requires an argument", opt.name)));
                    }
                }
            };
            if let Some(a) = apply(&mut o, &mut ts, opt.val, optarg.as_deref())? {
                return Ok(a);
            }
            continue;
        }
        if arg.len() > 1 && arg[0] == b'-' {
            let mut j = 1;
            while j < arg.len() {
                let c = arg[j] as char;
                j += 1;
                let has = match short_has_arg(c) {
                    Some(h) if arg[j - 1].is_ascii() => h,
                    _ => {
                        return Err(Error::usage(format!(
                            "invalid option -- '{}'",
                            String::from_utf8_lossy(&arg[j - 1..j])
                        )))
                    }
                };
                match has {
                    HasArg::No => {
                        if let Some(a) = apply(&mut o, &mut ts, c, None)? {
                            return Ok(a);
                        }
                    }
                    HasArg::Required => {
                        let optarg = if j < arg.len() {
                            let v = arg[j..].to_vec();
                            j = arg.len();
                            v
                        } else if i < args.len() {
                            i += 1;
                            args[i - 1].clone()
                        } else {
                            return Err(Error::usage(format!("option requires an argument -- '{c}'")));
                        };
                        if let Some(a) = apply(&mut o, &mut ts, c, Some(&optarg))? {
                            return Ok(a);
                        }
                    }
                }
            }
            continue;
        }
        operands.push(arg.clone());
    }
    if [ts.fake, ts.fixed, ts.clamp].iter().filter(|&&b| b).count() > 1 {
        return Err(Error::usage("-f/--faketime, --timestamp and --clamp-timestamps are mutually exclusive"));
    }
    match operands.len() {
        0 => return Err(Error::usage("Not enough arguments")),
        1 => {}
        _ => return Err(Error::usage("Too many arguments")),
    }
    o.image = OsString::from_vec(operands.pop().unwrap());
    Ok(Action::Run(Box::new(o)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(args: &[&str]) -> Result<Options> {
        match parse(args.iter().map(|s| s.to_string()))? {
            Action::Run(o) => Ok(*o),
            a => panic!("unexpected {a:?}"),
        }
    }

    #[test]
    fn clustering_and_attached() {
        let o = run(&["-zfb", "300", "img"]).unwrap();
        assert!(o.holes);
        assert_eq!(o.time, TimeMode::Fake);
        assert_eq!(o.blocks, Some(300));
        let o = run(&["-fzb300", "img"]).unwrap();
        assert_eq!(o.blocks, Some(300));
        let o = run(&["-b300", "img", "-b", "400"]).unwrap();
        assert_eq!(o.blocks, Some(400));
    }

    #[test]
    fn permutation_and_dashdash() {
        let o = run(&["-b", "300", "img", "-d", "dir"]).unwrap();
        assert_eq!(o.image, OsString::from("img"));
        assert_eq!(o.layers.len(), 1);
        let o = run(&["--", "-img"]).unwrap();
        assert_eq!(o.image, OsString::from("-img"));
        let o = run(&["-"]).unwrap();
        assert_eq!(o.image, OsString::from("-"));
    }

    #[test]
    fn long_options() {
        let o = run(&["--size-in-blocks=300", "--root", "a:/x/./y/", "img"]).unwrap();
        assert_eq!(o.blocks, Some(300));
        assert_eq!(o.layers[0].src, OsString::from("a"));
        assert_eq!(o.layers[0].path, vec![b"x".to_vec(), b"y".to_vec()]);
        let o = run(&["--size", "300", "img"]).unwrap();
        assert_eq!(o.blocks, Some(300));
        assert!(run(&["--b", "300", "img"]).is_err());
        assert!(run(&["--bogus", "img"]).is_err());
        assert!(run(&["--faketime=1", "img"]).is_err());
        let o = run(&["--squash=7", "img"]).unwrap();
        assert_eq!(o.squash_uid, Some(7));
        assert!(o.squash_perms);
        let o = run(&["--timestamp", "5", "img"]).unwrap();
        assert_eq!(o.time, TimeMode::Fixed(5));
    }

    #[test]
    fn next_word_taken_as_argument() {
        // -U takes "-P" as its argument, which is not a number.
        assert!(run(&["-U", "-P", "img"]).is_err());
        assert!(run(&["-q", "-d", "x", "img"]).is_err());
    }

    #[test]
    fn operands_and_errors() {
        assert_eq!(run(&[]).unwrap_err().msg, "Not enough arguments");
        assert_eq!(run(&["a", "b"]).unwrap_err().msg, "Too many arguments");
        assert!(run(&["-Z", "img"]).is_err());
        assert!(run(&["img", "-b"]).is_err());
        assert!(matches!(parse(["-h"]), Ok(Action::Help)));
        assert!(matches!(parse(["-V", "x", "y"]), Ok(Action::Version)));
    }

    #[test]
    fn numbers() {
        assert!(run(&["-b", "0", "img"]).is_err());
        assert!(run(&["-b", "+5", "img"]).is_err());
        assert!(run(&["-b", "0x10", "img"]).is_err());
        assert!(run(&["-b", "4294967296", "img"]).is_err());
        assert!(run(&["-m", "101", "img"]).is_err());
        assert!(run(&["-e", "256", "img"]).is_err());
        assert!(run(&["-U", "4294967295", "img"]).is_err());
        assert_eq!(run(&["-U", "4294967294", "img"]).unwrap().squash_uid, Some(4294967294));
        assert!(run(&["--timestamp", "2147483648", "img"]).is_err());
        assert!(run(&["-B", "512", "img"]).is_err());
        assert_eq!(run(&["-B", "4096", "img"]).unwrap().block_size, 4096);
    }

    #[test]
    fn readjust_forms() {
        assert_eq!(run(&["-r", "+1000", "img"]).unwrap().readjust, Some(Readjust::Extra(1000)));
        assert_eq!(run(&["-r", "+0", "img"]).unwrap().readjust, Some(Readjust::Extra(0)));
        assert_eq!(run(&["--readjustment=+5", "img"]).unwrap().readjust, Some(Readjust::Extra(5)));
        assert_eq!(run(&["-r40000", "img"]).unwrap().readjust, Some(Readjust::Absolute(40000)));
        assert_eq!(run(&["-r", "1", "-r", "+2", "img"]).unwrap().readjust, Some(Readjust::Extra(2)));
        for bad in ["-5", "+", "++5", "+-5", "0", "+4294967296", "4294967296", "1x", "+0x10", " 5"] {
            assert!(run(&["-r", bad, "img"]).is_err(), "{bad}");
        }
        // no other option gains a sign
        assert!(run(&["-b", "+5", "img"]).is_err());
        assert!(run(&["-N", "+5", "img"]).is_err());
    }

    #[test]
    fn creator_os_and_label() {
        assert_eq!(run(&["-o", "HURD", "img"]).unwrap().creator_os, 1);
        assert_eq!(run(&["-o", "FreeBSD", "img"]).unwrap().creator_os, 3);
        assert_eq!(run(&["-o", "7", "img"]).unwrap().creator_os, 7);
        assert!(run(&["-o", "masix", "img"]).is_err());
        assert!(run(&["-o", "4294967296", "img"]).is_err());
        let o = run(&["-L", "0123456789abcdefXYZ", "img"]).unwrap();
        assert_eq!(o.label.unwrap(), b"0123456789abcdef".to_vec());
    }

    #[test]
    fn time_exclusive() {
        assert!(run(&["-f", "--timestamp", "1", "img"]).is_err());
        assert!(run(&["-f", "-f", "img"]).is_ok());
        assert!(run(&["--timestamp", "1", "--timestamp", "2", "img"]).is_ok());
    }

    #[test]
    fn layer_paths() {
        let l = parse_layer(LayerKind::Dir, b"co:lon").unwrap();
        assert_eq!(l.src, OsString::from("co"));
        assert_eq!(l.path, vec![b"lon".to_vec()]);
        let l = parse_layer(LayerKind::Tar, b"a:b:c").unwrap();
        assert_eq!(l.src, OsString::from("a"));
        assert_eq!(l.path, vec![b"b:c".to_vec()]);
        assert!(parse_layer(LayerKind::Dir, b"a:/x/../y").is_err());
        assert!(run(&["-x", "a", "-x", "b", "img"]).is_err());
    }
}

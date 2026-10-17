//! Command line (SPEC §2): GNU `getopt_long` semantics and option validation.

use crate::error::{Error, Result};
use std::ffi::OsString;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub const USAGE: &str = "\
Usage: kext2 [options] image
Create an ext2 filesystem image (4 KiB blocks) from directories, tar archives
and device tables, without root privileges ('-' as image writes to standard
output).

  -d, --root <directory>[:path]     Copy a local directory into path (or root)
  -a, --tarball <file>[:path]       Copy a tar archive (plain, gzip, bzip2, xz or
                                    zstd; '-' = stdin) into path (or root)
  -D, --devtable <file>[:path]      Add or fix up nodes from a device table
      --free-space <bytes>          Free space to leave in the image, with one
                                    free inode per 16 KiB of it (suffixes K, M,
                                    G, T: powers of 1024)
  -L, --volume-label <string>       Volume label (up to 16 bytes)
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

/// Timestamp policy (R-TIME-1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TimeMode {
    /// Use `SOURCE_DATE_EPOCH` or the wall clock; file times from the inputs.
    #[default]
    Default,
    /// `-f`: every time is 0.
    Fake,
    /// `--timestamp S`.
    Fixed(i64),
    /// `--clamp-timestamps S`.
    Clamp(i64),
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Options {
    pub image: OsString,
    pub layers: Vec<Layer>,
    /// `--free-space`, in bytes.
    pub free_space: u64,
    pub label: Option<Vec<u8>>,
    pub time: TimeMode,
    pub squash_uid: Option<u32>,
    pub squash_perms: bool,
    pub verbose: bool,
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
const FREE_SPACE: char = '\u{3}';

const LONG: &[LongOpt] = &[
    LongOpt { name: "root", has_arg: HasArg::Required, val: 'd' },
    LongOpt { name: "devtable", has_arg: HasArg::Required, val: 'D' },
    LongOpt { name: "tarball", has_arg: HasArg::Required, val: 'a' },
    LongOpt { name: "free-space", has_arg: HasArg::Required, val: FREE_SPACE },
    LongOpt { name: "volume-label", has_arg: HasArg::Required, val: 'L' },
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

const SHORT: &str = "d:D:a:L:fq:U:PhVv";

fn short_has_arg(c: char) -> Option<HasArg> {
    let s = SHORT.as_bytes();
    let i = s.iter().position(|&b| b as char == c && b != b':')?;
    Some(if s.get(i + 1) == Some(&b':') { HasArg::Required } else { HasArg::No })
}

fn opt_name(c: char) -> String {
    match c {
        TIMESTAMP => "--timestamp".into(),
        CLAMP => "--clamp-timestamps".into(),
        FREE_SPACE => "--free-space".into(),
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
/// On Windows, the colon of a drive prefix (`C:\`, `C:/`) is not a separator.
pub fn parse_layer(kind: LayerKind, arg: &[u8]) -> Result<Layer> {
    let skip = if cfg!(windows) && drive_prefix(arg) { 2 } else { 0 };
    let (src, path) = match arg[skip..].iter().position(|&b| b == b':') {
        Some(i) => (&arg[..skip + i], &arg[skip + i + 1..]),
        None => (arg, &b""[..]),
    };
    let path = parse_image_path(path).map_err(|_| {
        Error::usage(format!("invalid image path in '{}': '..' is not allowed", String::from_utf8_lossy(arg)))
    })?;
    Ok(Layer { kind, src: crate::host::os_string(src.to_vec()), path })
}

fn drive_prefix(arg: &[u8]) -> bool {
    arg.len() >= 3 && arg[0].is_ascii_alphabetic() && arg[1] == b':' && (arg[2] == b'\\' || arg[2] == b'/')
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

/// Parse a byte count: a decimal number with an optional binary suffix
/// (`K`, `M`, `G`, `T`, optionally followed by `iB`).
pub fn parse_size(opt: char, arg: &[u8]) -> Result<u64> {
    let digits = arg.iter().take_while(|b| b.is_ascii_digit()).count();
    let (num, suffix) = arg.split_at(digits);
    let shift = match suffix.to_ascii_uppercase().as_slice() {
        b"" => 0,
        b"K" | b"KIB" => 10,
        b"M" | b"MIB" => 20,
        b"G" | b"GIB" => 30,
        b"T" | b"TIB" => 40,
        _ => {
            return Err(Error::usage(format!(
                "invalid value '{}' for {}: expected a number of bytes, optionally with K, M, G or T",
                String::from_utf8_lossy(arg),
                opt_name(opt)
            )))
        }
    };
    let max = MAX_FREE_SPACE >> shift;
    Ok(parse_num(opt, num, 0, max)? << shift)
}

/// Largest `--free-space`: 16 TiB, the size limit of ext2 with 4 KiB blocks.
pub const MAX_FREE_SPACE: u64 = 1 << 44;

#[derive(Default)]
struct TimeSeen {
    fake: bool,
    fixed: bool,
    clamp: bool,
}

fn apply(o: &mut Options, ts: &mut TimeSeen, c: char, arg: Option<&[u8]>) -> Result<Option<Action>> {
    let a = || arg.unwrap_or(b"");
    match c {
        'd' => o.layers.push(parse_layer(LayerKind::Dir, a())?),
        'a' => o.layers.push(parse_layer(LayerKind::Tar, a())?),
        'D' => o.layers.push(parse_layer(LayerKind::DevTable, a())?),
        FREE_SPACE => o.free_space = parse_size(c, a())?,
        'L' => {
            let mut l = a().to_vec();
            if l.len() > 16 {
                crate::error::warn(format!("volume label '{}' truncated to 16 bytes", String::from_utf8_lossy(&l)));
                l.truncate(16);
            }
            o.label = Some(l);
        }
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
    let args: Vec<Vec<u8>> = args.into_iter().map(|s| crate::host::os_bytes(&s.into())).collect();
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
    o.image = crate::host::os_string(operands.pop().unwrap());
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
        let o = run(&["-fPU", "300", "img"]).unwrap();
        assert!(o.squash_perms);
        assert_eq!(o.time, TimeMode::Fake);
        assert_eq!(o.squash_uid, Some(300));
        let o = run(&["-fU300", "img"]).unwrap();
        assert_eq!(o.squash_uid, Some(300));
        let o = run(&["-U300", "img", "-U", "400"]).unwrap();
        assert_eq!(o.squash_uid, Some(400));
    }

    #[test]
    fn permutation_and_dashdash() {
        let o = run(&["-U", "300", "img", "-d", "dir"]).unwrap();
        assert_eq!(o.image, OsString::from("img"));
        assert_eq!(o.layers.len(), 1);
        let o = run(&["--", "-img"]).unwrap();
        assert_eq!(o.image, OsString::from("-img"));
        let o = run(&["-"]).unwrap();
        assert_eq!(o.image, OsString::from("-"));
    }

    #[test]
    fn long_options() {
        let o = run(&["--free-space=300", "--root", "ab:/x/./y/", "img"]).unwrap();
        assert_eq!(o.free_space, 300);
        assert_eq!(o.layers[0].src, OsString::from("ab"));
        assert_eq!(o.layers[0].path, vec![b"x".to_vec(), b"y".to_vec()]);
        let o = run(&["--free", "300", "img"]).unwrap();
        assert_eq!(o.free_space, 300);
        assert!(run(&["--squash-", "300", "img"]).is_err());
        assert!(run(&["--bogus", "img"]).is_err());
        assert!(run(&["--faketime=1", "img"]).is_err());
        let o = run(&["--squash=7", "img"]).unwrap();
        assert_eq!(o.squash_uid, Some(7));
        assert!(o.squash_perms);
        let o = run(&["--timestamp", "5", "img"]).unwrap();
        assert_eq!(o.time, TimeMode::Fixed(5));
    }

    #[test]
    fn dropped_options() {
        for opt in ["-B", "-b", "-r", "-N", "-i", "-m", "-o", "-e", "-g", "-x"] {
            assert!(run(&[opt, "1", "img"]).is_err(), "{opt}");
        }
        assert!(run(&["-z", "img"]).is_err());
        assert!(run(&["--block-size", "4096", "img"]).is_err());
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
        assert!(run(&["-U", "4294967295", "img"]).is_err());
        assert_eq!(run(&["-U", "4294967294", "img"]).unwrap().squash_uid, Some(4294967294));
        assert!(run(&["-U", "+5", "img"]).is_err());
        assert!(run(&["-U", "0x10", "img"]).is_err());
        assert!(run(&["--timestamp", "2147483648", "img"]).is_err());
        assert!(run(&["--timestamp", "-1", "img"]).is_err());
    }

    #[test]
    fn free_space() {
        let fs = |v: &str| run(&["--free-space", v, "img"]).map(|o| o.free_space);
        assert_eq!(run(&["img"]).unwrap().free_space, 0);
        assert_eq!(fs("0").unwrap(), 0);
        assert_eq!(fs("12345").unwrap(), 12345);
        assert_eq!(fs("4k").unwrap(), 4096);
        assert_eq!(fs("3M").unwrap(), 3 << 20);
        assert_eq!(fs("2GiB").unwrap(), 2 << 30);
        assert_eq!(fs("16T").unwrap(), 16 << 40);
        for bad in ["", "-1", "+5", "1.5G", "5X", "K", "17T", "1 G", "0x10"] {
            assert!(fs(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn label() {
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
        assert!(parse_layer(LayerKind::Dir, b"ab:/x/../y").is_err());
    }

    #[test]
    fn drive_prefixes() {
        // A one-letter source followed by `:\` or `:/` is a Windows drive.
        let l = parse_layer(LayerKind::Dir, b"C:\\src:/opt").unwrap();
        let l2 = parse_layer(LayerKind::Dir, b"a:/x").unwrap();
        if cfg!(windows) {
            assert_eq!((l.src, l.path), (OsString::from("C:\\src"), vec![b"opt".to_vec()]));
            assert_eq!((l2.src, l2.path), (OsString::from("a:/x"), vec![]));
        } else {
            assert_eq!(l.src, OsString::from("C"));
            assert_eq!((l2.src, l2.path), (OsString::from("a"), vec![b"x".to_vec()]));
        }
    }
}

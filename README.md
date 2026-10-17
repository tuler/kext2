# kext2

kext2 builds ext2 filesystem images from host directories, tar archives and
device tables, without root privileges. The images are byte-for-byte
reproducible and hold the same file content as `xgenext2fs` 1.5.6 would
store. kext2 started with the xgenext2fs command line; it now keeps only the
options that matter for reproducible root filesystems (see
[Differences from xgenext2fs](#differences-from-xgenext2fs)).

It was written in a clean room from [`docs/SPEC.md`](docs/SPEC.md) and the
public ext2 documentation (see [`docs/CLEANROOM.md`](docs/CLEANROOM.md)).

You can use it as:

- a **command line tool** (`kext2`);
- a **Rust library** (`kext2`);
- a **Node.js native module** ([`@tuler/kext2`](crates/kext2-node/README.md)).

## Installing

Once released, prebuilt CLI binaries are attached to each GitHub release (the
`cargo-kext2-vX.Y.Z` ones), for Linux (x86_64, arm64, static musl), macOS
(x86_64, arm64) and Windows (x86_64). With Cargo:

```bash
cargo install kext2
```

## Build

```bash
cargo build --release        # target/release/kext2
cargo test --release --workspace
(cd crates/kext2-node && npm run build && npm test)   # Node.js binding
```

It is a Cargo workspace: `crates/kext2` (library and CLI) and
`crates/kext2-node` (the N-API binding, built with
[napi-rs](https://napi.rs)). Rust 1.87 or later, 2021 edition. It builds on
macOS, Linux and Windows. On Windows, host files have no owner, permission
bits or inode identity: files are 0644 (0444 read-only), directories 0755,
owned by 0:0, and hard links are not detected. The dependencies are
permissively licensed: `flate2` (with `zlib-rs`), `bzip2` (with
`libbz2-rs-sys`), `liblzma`, `zstd` and `tempfile`. The decompressors are
the Cargo features `gzip`, `bzip2`, `xz` and `zstd`, all on by default.

## Node.js

```ts
import { build, buildBuffer, kext2 } from '@tuler/kext2'

// the command line, in process
await kext2(['--faketime', '--free-space', '64M', '--root', 'rootfs', 'out.ext2'])

// typed options; builds run on the libuv thread pool
const info = await build('out.ext2', {
  faketime: true,
  freeSpace: 64 << 20,
  layers: [{ type: 'tar', path: 'rootfs.tar' }, { type: 'dir', path: 'extra', target: '/opt' }],
})
const image: Buffer = await buildBuffer({ freeSpace: 10 << 20, faketime: true })
```

See [`crates/kext2-node/README.md`](crates/kext2-node/README.md).

## Usage

```
kext2 [options] image
```

`image` is the output file. `-` writes the image to standard output, which
can be a pipe. A block device is written in place.

The image always has 4 KiB blocks. Its size is computed from the content:
the smallest image that holds it, plus `--free-space`.

| Option | Meaning |
|---|---|
| `-d, --root DIR[:PATH]` | Copy the host directory `DIR` into the image directory `PATH` (default `/`). |
| `-a, --tarball TAR[:PATH]` | Copy a tar archive into `PATH`. `-` means standard input. The archive can be plain, gzip, bzip2, xz or zstd; the format is detected from its content. |
| `-D, --devtable TABLE[:PATH]` | Create or fix up the nodes listed in a device table. `-` means standard input. |
| `--free-space BYTES` | Leave at least `BYTES` free (rounded up to whole blocks), and one free inode per 16 KiB of it. `K`, `M`, `G` and `T` (or `KiB` … `TiB`) are powers of 1024. Default 0. |
| `-L, --volume-label S` | Volume label, at most 16 bytes. |
| `-f, --faketime` | Every timestamp is 0. |
| `--timestamp S` | Every timestamp is S. |
| `--clamp-timestamps S` | The filesystem time is S, and file times later than S become S. |
| `-U, --squash-uids UID` | Files from `-d`/`-a` are owned by `UID:UID`. |
| `-P, --squash-perms` | Clear the group and other permission bits of files from `-d`/`-a`. |
| `-q, --squash UID` | Same as `-U UID -P`. |
| `-v, --verbose` | Print the geometry and phase timings to standard error. |
| `-h, --help`, `-V, --version` | Show the help text or the version. |

Options follow GNU `getopt_long` rules. You can mix options and the operand,
cluster short options (`-fPU1000`), and abbreviate long options
(`--free 64M`). When an option is given more than once, the last value wins.
`-d`, `-a` and `-D` add up in order. Numbers must be plain decimal (with a
size suffix for `--free-space`) and in range; anything else is a usage error.

Without a timestamp option, the filesystem time comes from
`$SOURCE_DATE_EPOCH`, or the current time if it is unset. File times always
come from the inputs.

Examples:

```bash
# a reproducible root filesystem from a container export
SOURCE_DATE_EPOCH=1700000000 kext2 -a rootfs.tar.zst rootfs.ext2

# layers: a base tree, an overlay under /opt, device nodes, 1 GiB free
kext2 --free-space 1G -d base -d overlay:/opt -D devices.txt image.ext2

# stream the image from a pipe into a pipe
zstd -dc rootfs.tar.zst | kext2 -f -a - - | gzip > rootfs.ext2.gz
```

Exit status: 0 on success, 1 on any error. On failure, nothing is left at the
output path. A file that was already there stays unchanged, because the
image is written to a temporary file next to it and renamed at the end.

## Performance

Building a 1 GiB root filesystem (9,610 files) with 4 KiB blocks, compared
with xgenext2fs 1.5.6. Best of 3 runs on an Apple M1 Max, with
`compare/bench.py`:

| input | xgenext2fs | kext2 | speedup |
|---|---:|---:|---:|
| directory | 2.36 s | 0.24 s | 9.8× |
| tar | 2.15 s | 0.18 s | 11.8× |
| tar.gz | 2.55 s | 0.79 s | 3.2× |

kext2 is faster because it plans the whole layout first and then writes the
image in block order from several threads. Decompression bounds the tar.gz
case. The price is memory: xgenext2fs stays at about 3 MiB, while kext2
uses about 45 MiB, mostly write buffers (see [Design](#design)). Both tools
store the same file content, and both images pass `e2fsck`. See
[`compare/BENCH.md`](compare/BENCH.md) and
[`compare/MEMORY.md`](compare/MEMORY.md) for more cases.

## Design

kext2 works in three phases (SPEC §3):

1. **Scan** (`crates/kext2/src/input/`). The layers build one in-memory tree in
   command-line order. Children are kept in hash maps, and the first entry for
   a path wins (SPEC R-IN-3). Regular files are not read here. kext2 records
   where their data can be read again: the host path, an offset in an
   uncompressed tar, or an offset in a spool file for compressed or piped
   archives. Decompression runs on its own thread.
2. **Plan** (`crates/kext2/src/plan.rs`, `geometry.rs`, `layout.rs`, `dirent.rs`). It
   sorts every directory once, checks the limits, and computes the exact
   size and geometry (SPEC §7).
   Then a single forward data cursor places every inode and block (SPEC §9).
   Each file's blocks, indirect blocks included, form one contiguous run.
   Top-level directories are spread over the block groups when there is free
   space. The plan is a list of extents in data-index order.
3. **Write** (`crates/kext2/src/write.rs`). The image is cut into 4 MiB chunks. Each chunk
   is a pure function of the plan, so several threads can render chunks at
   once: metadata, directory blocks, indirect blocks and file data read with
   `pread`. One writer thread writes the chunks in block order. In a regular
   output file, large zero regions stay holes when the image is mostly empty
   or sparse. A pipe gets every byte. The bytes never depend on the number
   of threads (`KEXT2_THREADS` sets it; the default is up to 8).

Memory has two parts:
- **Per file:** about 0.3 KiB for the tree and the plan.
- **Write buffers:** about 40 MiB with 8 threads, 12 MiB with
  `KEXT2_THREADS=1` (slower, same image).

It does not grow with the size of the data: file data is streamed and
allocations are stored as runs. On a 1 GiB
root filesystem the peak footprint is about 45 MiB.
[`compare/MEMORY.md`](compare/MEMORY.md) has the measurements. To see the
heap by phase, build with `--features memstats` (development only).

Module map:

| Module | Contents |
|---|---|
| `cli` | getopt_long parser, option validation |
| `input::dir`, `input::tar`, `input::devtable` | the three layer types |
| `tree` | nodes, the insertion rule, implicit directories |
| `geometry` | `has_super`, GEOMETRY and SIZE (SPEC §7.3–7.4), data-index → block mapping |
| `layout` | block-map layout sequences, indirect block counts, size limits |
| `dirent` | directory entry packing |
| `plan` | limits, lost+found, sizing, inode and block allocation |
| `format` | superblock, group descriptor and inode encoding |
| `write` | chunk renderer and output modes |

## Differences from xgenext2fs

kext2 follows SPEC §13 and the reduced option set of SPEC Appendix C.2.

The options are not those of xgenext2fs. kext2 keeps the inputs (`-d`, `-a`,
`-D`), the timestamp options, `-L`, `-U`, `-P` and `-q`, and adds
`--free-space`. It does not have:

* `-B`: blocks are always 4 KiB.
* `-b`, `-r`, `-N`, `-i`: the size comes from the content plus
  `--free-space`, and the inode count from the files plus one inode per
  16 KiB of `--free-space`.
* `-m`: no blocks are reserved for root, so all free space is usable.
  lost+found always exists, with 4 blocks (16 KiB) as mke2fs makes it.
* `-o`: the creator OS is Linux, and `filetype` is always set.
* `-e`: free blocks are zero.
* `-z`: every file is stored dense. Zero ranges in tar sparse members are
  still read as zeros.
* `-x`, `-g`: no starting image and no block-map files.

The differences that change file content:

* uid and gid keep all 32 bits (xgenext2fs keeps 16).
* Device numbers of 256 and above use the new encoding.
* A name longer than 255 bytes, an empty symlink target, a target of 4096
  bytes or more, or a hard link to a directory is an error. xgenext2fs writes
  a corrupt image instead.
* The `:path` part of `-d`, `-a` and `-D` is honoured.
* A tar member that duplicates an existing path is ignored. xgenext2fs also
  copies its mode and owner onto the parent directory.
* A tar member whose parent directory is missing gets that parent created
  (mode 0755, owner 0:0). A leading `/` and `.` components are removed from
  tar names; members with `..` are skipped.
* ctime and atime equal mtime.
* Times outside the signed 32-bit range are clamped, and a malformed
  `SOURCE_DATE_EPOCH` is an error.
* lost+found is always created.
* A device-table line of type `l`, or with out-of-range values, is skipped
  with a warning.

The differences that do not change file content:

* `-U` and the numeric options are validated.
* `-q UID` works.
* An unreadable file is an error, not a crash.
* No corrupt image is left behind after a failure.
* Standard input and standard output work without extra options.
* On disk, kext2 sets `filetype`, `sparse_super` and, when needed,
  `large_file`. It also sets errors=continue and max mount count −1, uses
  the allocation policy of SPEC §9 and sorts directory entries.
* Huge directories and huge files take linear time.

## Comparison harness

`compare/` runs kext2, mkext2 and xgenext2fs from the outside and inspects
their images in a Docker container. The checks are `e2fsck -fn`, a read-only
kernel mount, a full walk of the tree, and reproducibility. kext2 runs the
cases without `-B` (its blocks are always 4 KiB) and skips the cases that use
an option it does not have. See [`compare/README.md`](compare/README.md).

```bash
docker build -t kext2-tools compare/docker   # once
cargo build --release
python3 compare/run.py                       # correctness + reproducibility
python3 compare/bench.py 1024 --out compare/BENCH.md
```

## Releasing

Releases are automated with [Sampo](https://github.com/bruits/sampo)
changesets and published from CI with OIDC trusted publishing; see
[RELEASING.md](RELEASING.md).

## License

Apache-2.0, see [`LICENSE`](LICENSE).

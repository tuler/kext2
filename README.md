# kext2

kext2 builds ext2 filesystem images from host directories, tar archives and
device tables, without root privileges. It accepts the command line of
`xgenext2fs` 1.5.6, produces the same file content, and writes images that are
byte-for-byte reproducible.

It was written in a clean room from [`docs/SPEC.md`](docs/SPEC.md) and the
public ext2 documentation (see [`docs/CLEANROOM.md`](docs/CLEANROOM.md)).

## Build

```bash
cargo build --release        # target/release/kext2
cargo test --release
```

Rust 2021 edition. It builds on macOS and Linux. The dependencies are
permissively licensed: `flate2` (with `zlib-rs`), `bzip2` (with
`libbz2-rs-sys`), `liblzma`, `zstd` and `tempfile`.

## Usage

```
kext2 [options] image
```

`image` is the output file. `-` writes the image to standard output, which
can be a pipe. A block device is written in place.

| Option | Meaning |
|---|---|
| `-d, --root DIR[:PATH]` | Copy the host directory `DIR` into the image directory `PATH` (default `/`). |
| `-a, --tarball TAR[:PATH]` | Copy a tar archive into `PATH`. `-` means standard input. The archive can be plain, gzip, bzip2, xz or zstd; the format is detected from its content. |
| `-D, --devtable TABLE[:PATH]` | Create or fix up the nodes listed in a device table. `-` means standard input. |
| `-x, --starting-image IMG` | Start from the content of an existing ext2 image. The new image takes its block size, block count, inode count, reserved count, label, OS and UUID. |
| `-B, --block-size N` | Block size: 1024 (default), 2048 or 4096. |
| `-b, --size-in-blocks N` | Exact image size. Without `-b` the size is computed from the content. |
| `-r, --readjustment [+]N` | `+N`: N extra free blocks on top of the computed size; `N`: exactly N blocks, like `-b N`. Ignored when `-b` is given. |
| `-N, --number-of-inodes N` | Minimum number of inodes. |
| `-i, --bytes-per-inode N` | One inode per N bytes of the image (only with `-b`). |
| `-m, --reserved-percentage N` | Reserved block percentage, 0–100 (default 5). |
| `-L, --volume-label S` | Volume label, at most 16 bytes. |
| `-o, --creator-os OS` | `linux`, `hurd`, `freebsd`, `lites` or a number. |
| `-e, --fill-value N` | Fill free blocks with byte N. |
| `-z, --allow-holes` | Store all-zero blocks as holes. |
| `-f, --faketime` | Every timestamp is 0. |
| `--timestamp S` | Every timestamp is S. |
| `--clamp-timestamps S` | The filesystem time is S, and file times later than S become S. |
| `-U, --squash-uids UID` | Files from `-d`/`-a` are owned by `UID:UID`. |
| `-P, --squash-perms` | Clear the group and other permission bits of files from `-d`/`-a`. |
| `-q, --squash UID` | Same as `-U UID -P`. |
| `-g, --block-map PATH` | Write `PATH`'s block list to `./<PATH with / → _>.blk`. |
| `-v, --verbose` | Print the geometry and phase timings to standard error. |
| `-h, --help`, `-V, --version` | Show the help text or the version. |

Options follow GNU `getopt_long` rules. You can mix options and the operand,
cluster short options (`-zfb300`), and abbreviate long options (`--size 300`).
When an option is given more than once, the last value wins. `-d`, `-a`, `-D`
and `-g` add up in order. Numbers must be plain decimal and in range; anything
else is a usage error.

Without a timestamp option, the filesystem time comes from
`$SOURCE_DATE_EPOCH`, or the current time if it is unset. File times always
come from the inputs.

Examples:

```bash
# a reproducible root filesystem from a container export
SOURCE_DATE_EPOCH=1700000000 kext2 -B 4096 -a rootfs.tar.zst rootfs.ext2

# layers: a base tree, an overlay under /opt, device nodes, explicit size
kext2 -B 4096 -b 262144 -d base -d overlay:/opt -D devices.txt image.ext2

# stream the image from a pipe into a pipe
zstd -dc rootfs.tar.zst | kext2 -f -a - - | gzip > rootfs.ext2.gz
```

Exit status: 0 on success, 1 on any error. On failure, nothing is left at the
output path. A file that was already there stays unchanged, because the
image is written to a temporary file next to it and renamed at the end.

## Design

kext2 works in three phases (SPEC §3):

1. **Scan** (`src/input/`). The layers build one in-memory tree in
   command-line order. Children are kept in hash maps, and the first entry for
   a path wins (SPEC R-IN-3). Regular files are not read here. kext2 records
   where their data can be read again: the host path, an offset in an
   uncompressed tar, or an offset in a spool file for compressed or piped
   archives. With `-z`, it also reads each file once to record which blocks
   are not zero. Decompression runs on its own thread.
2. **Plan** (`src/plan.rs`, `geometry.rs`, `layout.rs`, `dirent.rs`). It
   sorts every directory once, checks the limits, decides on lost+found and
   the reserved count, and computes the exact size and geometry (SPEC §7).
   Then a single forward data cursor places every inode and block (SPEC §9).
   Each file's blocks, indirect blocks included, form one contiguous run.
   Top-level directories are spread over the block groups when there is free
   space. The plan is a list of extents in data-index order.
3. **Write** (`src/write.rs`). The image is cut into 4 MiB chunks. Each chunk
   is a pure function of the plan, so several threads can render chunks at
   once: metadata, directory blocks, indirect blocks and file data read with
   `pread`. One writer thread writes the chunks in block order. In a regular
   output file, large zero regions stay holes when the image is mostly empty
   or sparse. A pipe gets every byte. The bytes never depend on the number
   of threads (`KEXT2_THREADS` sets it; the default is up to 8).

Memory grows with the number of nodes and names. It does not grow with the
size of the data: file data is streamed, allocations are stored as runs, and
zero-block maps are run lists. On a 1 GiB root filesystem the peak RSS is
about 40 MiB.

Module map:

| Module | Contents |
|---|---|
| `cli` | getopt_long parser, option validation |
| `input::dir`, `input::tar`, `input::devtable` | the three layer types |
| `image` | reads a starting image for `-x` |
| `tree` | nodes, the insertion rule, implicit directories |
| `geometry` | `has_super`, GEOMETRY and SIZE (SPEC §7.3–7.4), data-index → block mapping |
| `layout` | block-map layout sequences, indirect block counts, size limits |
| `dirent` | directory entry packing |
| `plan` | limits, lost+found, sizing, inode and block allocation |
| `format` | superblock, group descriptor and inode encoding |
| `write` | chunk renderer and output modes |
| `blockmap` | `-g` files |
| `runs` | run-length block sets, zero detection |

## Differences from xgenext2fs

kext2 follows SPEC §13. The differences that change file content:

* uid and gid keep all 32 bits (xgenext2fs keeps 16).
* Device numbers of 256 and above use the new encoding.
* A name longer than 255 bytes, an empty symlink target, a target of `bs`
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
* The automatic size is exact, so tiny trees work. Holes from `-z` do not
  count toward the size.
* A device-table line of type `l`, or with out-of-range values, is skipped
  with a warning.

The differences that do not change file content:

* `-r N` sets the block count but not the inode count; `-r +N` adds N free blocks (SPEC Appendix C.1).
* A `-b` value that is too small is an error that reports the minimum.
* `-m`, `-e`, `-o`, `-U` and the numeric options are validated.
* `-q UID` works.
* An unreadable file is an error, not a crash.
* No corrupt image is left behind after a failure.
* Standard input and standard output work without extra options.
* On disk, kext2 sets `filetype` (except for Hurd), `sparse_super` and, when
  needed, `large_file`. It also sets errors=continue and max mount count −1,
  uses the allocation policy of SPEC §9 and sorts directory entries.
* `-g` lists one entry for every logical block.
* Huge directories and huge files take linear time.

## Comparison harness

`compare/` runs kext2, mkext2 and xgenext2fs from the outside and inspects
their images in a Docker container. The checks are `e2fsck -fn`, a read-only
kernel mount, a full walk of the tree, and reproducibility. See
[`compare/README.md`](compare/README.md).

```bash
docker build -t kext2-tools compare/docker   # once
cargo build --release
python3 compare/run.py                       # correctness + reproducibility
python3 compare/bench.py 1024 --out compare/BENCH.md
```

## License

Apache-2.0, see [`LICENSE`](LICENSE).

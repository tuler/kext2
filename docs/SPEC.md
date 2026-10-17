# kext2 — Requirements and Design Specification

Status: draft 1 (2026-10-16), amended by Appendix C (C.2, 2026-10-17, narrows
the command line and supersedes the rules it names). Audience: the kext2 implementer (Rust) and the
verifier. This document, together with Dave Poirier's "The Second Extended
File System" and the Linux kernel `Documentation/filesystems/ext2.rst` and
`Documentation/filesystems/ext4/*.rst`, is the complete basis for the
implementation. It was written under the clean-room rules of
[`CLEANROOM.md`](CLEANROOM.md): the on-disk rules come from the Linux
kernel's ext2 driver (cited as *provenance*, e.g. `super.c:ext2_fill_super`,
never copied), and everything about the command line comes from running
xgenext2fs 1.5.6 as a black box (cited as `O-n`, see §12).

## 0. Conventions

* **MUST / MUST NOT / SHOULD / MAY** have their RFC 2119 meaning.
* Requirements are numbered `R-<AREA>-<n>`. Black-box observations of
  xgenext2fs are `O-<n>` (§12). Deliberate divergences from xgenext2fs are
  `D-<n>` (§13). Acceptance tests are `AT-<area>-<n>` (§14). Open questions are
  `Q-<n>` (§15).
* All on-disk integers are little-endian. "Block" means a filesystem block of
  `bs` bytes. `ceil(a/b)` is integer division rounded up; `roundup(a, k)` is
  `ceil(a/k)·k`. Block numbers are absolute (counted from byte 0 of the image).
* `T_fs` is the *filesystem time* (§6.1).
* "Provenance" names the kernel source file and function whose behaviour a
  rule satisfies. It is there so the verifier can audit the rule; the
  implementer MUST NOT need (and MUST NOT read) the kernel sources.

## 1. Goals and non-goals

kext2 builds an ext2 filesystem image from host directories, tar archives
and device tables, without root privileges.

| Goal | Summary |
|---|---|
| G1 Compatibility | Accept the xgenext2fs 1.5.6 command line (§2) and produce an image with the same *content* (§14.2) as xgenext2fs for the same command line, except for the deliberate divergences of §13. |
| G2 Validity | Every image MUST mount read-only on Linux and pass `e2fsck -fn` with zero findings, for block sizes 1024/2048/4096, any number of block groups, every block-map indirection level, files larger than 2 GiB and 4 GiB, huge directories and empty images. |
| G3 Reproducibility | Output bytes depend only on input *content*, options and `T_fs` (§9). |
| G4 Speed | Linear-time planning, one sequential read of each input file's data, contiguous file data (§10). |

Non-goals: extended attributes and ACLs (never written; pax `SCHILY.xattr.*`
and host xattrs are ignored), journalling, ext3/ext4 features, `dir_index`,
modification of an image in place (`-x` re-lays out the whole image, §11.1).

## 2. Command line

### 2.1 Syntax

```
kext2 [options] image
```

R-CLI-1. Options are parsed with GNU `getopt_long` semantics, as xgenext2fs
does (O-1): options and the operand may be interleaved (permutation); `--`
ends option processing; short options may be clustered (`-zfb 300`,
`-fzb300`); a short option's required argument may be attached (`-b300`) or be
the next word (`-b 300`) — the next word is taken even if it begins with `-`
(so `-U -P` sets the squash uid from the string `-P`, which is then rejected by
R-CLI-4); long options accept `--name=value` and `--name value`; any
unambiguous prefix of a long option name is accepted (`--size 300`), an
ambiguous one is a usage error (`--b`). When an option that takes a single
value is given more than once, the last occurrence wins. `-d`, `-a`, `-D` and
`-g` accumulate in command-line order.

R-CLI-2. Exactly one operand (the image path) is required unless `-h` or `-V`
is given. No operand: message `Not enough arguments` and exit 1. More than one
operand: message `Too many arguments` and exit 1. An unknown option or a
missing option argument: usage error, exit 1.

R-CLI-3. Option table. Short option string: `x:d:D:a:B:b:r:i:N:L:m:o:g:e:zfq:U:PhVv`.

| Short | Long | Argument | kext2 semantics (normative details in the referenced sections) |
|---|---|---|---|
| `-x` | `--starting-image` | image | Start from the content and geometry of an existing ext2 image (§11.1, phase 2). |
| `-d` | `--root` | `src[:path]` | Layer: copy host directory `src` into image directory `path` (default `/`) (§4, §5.1). |
| `-a` | `--tarball` | `src[:path]` | Layer: copy tar archive `src` (`-` = standard input; compressed archives allowed) into `path` (§5.2). |
| `-D` | `--devtable` | `src[:path]` | Layer: create or fix up nodes listed in device table `src` (`-` = standard input) relative to `path` (§5.3). |
| `-B` | `--block-size` | bytes | Block size: `1024` (default), `2048` or `4096`; anything else: error `Valid block sizes: 1024, 2048 or 4096.`, exit 1 (O-10). |
| `-b` | `--size-in-blocks` | blocks | Exact image size in blocks (R-SIZE-5). Without `-b` the size is computed (R-SIZE-4). |
| `-r` | `--readjustment` | blocks | Extra free blocks added on top of the automatic size (R-SIZE-4). Ignored (with a warning) when `-b` is given. (D-10) |
| `-i` | `--bytes-per-inode` | bytes | Inode density; effective only together with `-b` (§7.1). |
| `-N` | `--number-of-inodes` | count | Minimum total number of inodes (§7.1). |
| `-L` | `--volume-label` | string | Volume label, at most 16 bytes; longer labels are truncated to 16 bytes with a warning (O-11). |
| `-m` | `--reserved-percentage` | percent | Reserved-block percentage, integer 0–100, default 5 (§7.2). |
| `-o` | `--creator-os` | os | `linux` (default, 0), `hurd` (1), `freebsd` (3), `lites` (4), case-insensitive, or a decimal number 0–4294967295 (O-12). |
| `-g` | `--block-map` | path | Write a block-map file for image path `path` (§11.2, phase 2). |
| `-e` | `--fill-value` | byte | Fill every free block with this byte value, decimal 0–255 (R-OUT-3). |
| `-z` | `--allow-holes` | — | Store all-zero file blocks as holes (§8.6). |
| `-f` | `--faketime` | — | Every timestamp is 0 (§6.1). |
| `-q` | `--squash` | uid | Same as `-U uid -P` (§5.4). |
| `-U` | `--squash-uids` | uid | Every node from `-d`/`-a` layers gets owner `uid:uid` (§5.4). |
| `-P` | `--squash-perms` | — | Clear group and other permission bits of nodes from `-d`/`-a` layers (§5.4). |
| `-h` | `--help` | — | Print usage to standard error, exit 0. |
| `-V` | `--version` | — | Print `kext2 <version>` to standard output, exit 0. |
| `-v` | `--verbose` | — | Print a human-readable summary (parameters, geometry, counts) to standard error. Format is free; it MUST NOT affect the image. |
| — | `--timestamp` | secs | Every timestamp (filesystem and files) is `secs` (§6.1). |
| — | `--clamp-timestamps` | secs | Filesystem time is `secs`; file times later than `secs` become `secs` (§6.1). |

R-CLI-4. Numeric arguments are unsigned decimal integers (`[0-9]+`, no sign,
no `0x`, no trailing characters) and MUST be range-checked; anything else is a
usage error, exit 1, message naming the option. Ranges: `-b` 1…2³²−1; `-i`
1…2⁶⁴−1; `-N` 0…2³²−1; `-m` 0…100; `-r` 0…2³²−1; `-e` 0…255; `-U`/`-q`
0…4294967294; `--timestamp`/`--clamp-timestamps` 0…2³¹−1. (xgenext2fs uses
`atoi`-like parsing and silently accepts garbage, see O-5, O-13, O-16; D-19.)

R-CLI-5. `-f`, `--timestamp` and `--clamp-timestamps` are mutually exclusive;
giving two different ones is a usage error (exit 1). Repeating the same one
follows R-CLI-1 (last wins).

R-CLI-6. `src[:path]` is split at the **first** `:` (O-23). `src` is the
part before it; `path` (possibly empty) the part after it. `path` is an image
path: components are separated by `/`, empty and `.` components are ignored,
a `..` component is a usage error; the empty path and `/` mean the root
directory. (xgenext2fs parses but ignores `path`; kext2 honours it: D-4.)

R-CLI-7. Exit status: 0 on success; 1 on any error (usage, unreadable input,
invalid input, image too small, I/O error). kext2 MUST NOT crash on any input.

R-CLI-8. Messages go to standard error, prefixed `kext2: `. Warnings are
prefixed `kext2: warning: `. For each layer kext2 SHOULD print, like
xgenext2fs (O-37), `copying from directory <src>`, `copying from tar archive
<src>` or `nodes fixup and creation from device table <src>`. Message wording
is not normative and is not compared by the verifier.

R-CLI-9. Environment: only `SOURCE_DATE_EPOCH` (§6.1) influences the result.
Locale, time zone, umask, `TMPDIR` (used only for spooling) and the number of
CPUs MUST NOT influence the image bytes.

## 3. Processing model

R-MODEL-1. kext2 works in three phases:

1. **Scan.** Build an in-memory tree of nodes from the starting image (if
   `-x`), then from every layer in command-line order (§4–§5). Record, for
   every regular file, where its data can be read again (host path, offset in
   a seekable tar, or offset in a spool file) and, with `-z`, its zero-block
   map. No image bytes are written in this phase.
2. **Plan.** Validate limits (§8.10), decide lost+found (§7.2), compute the
   inode count, the geometry and the block count (§7), and assign every inode
   number and every block (§9.2). The plan is a pure function of the tree,
   the options and `T_fs`.
3. **Write.** Emit the image (§10), reading each regular file's data exactly
   once, in allocation order.

R-MODEL-2. A *node* has: type (regular, directory, symlink, character device,
block device, fifo, socket); permission bits (12 bits: `07777`); uid and gid
(32-bit); mtime (signed 64-bit seconds before clamping, §6); for regular
files size and data; for symlinks the target bytes; for devices major and
minor; for directories a map from name (byte string) to node. Several names
may refer to the same non-directory node (hard links). Atime and ctime are not
stored: they are derived (§6.2).

R-MODEL-3. Directory children are kept in a hash map (or equivalent) during
the scan — lookups MUST be O(1) amortised or O(log n) — and sorted by name
(unsigned byte-wise lexicographic order, shorter prefix first) only once, in
the plan phase.

## 4. Building the tree: layers

R-IN-1. The initial tree is a single root directory with mode `040755`, uid 0,
gid 0, mtime `T_fs`, flagged *implicit* (R-IN-4), plus, when `-m` is not 0,
the provisional lost+found of R-RES-1. With `-x` the initial tree is the
starting image's tree (§11.1).

R-IN-2. Layers (`-d`, `-a`, `-D`) are applied to the one tree in
command-line order. Each layer has a *target directory* given by its `path`
(R-CLI-6). If the target directory does not exist it is created as an
implicit directory, together with any missing ancestors (D-4). If the target
or one of its ancestors exists but is not a directory, kext2 fails (exit 1).

R-IN-3. **Insertion rule (first wins).** When a `-d` or `-a` layer
supplies an entry for image path `P` = parent `Q` + name
`n`:

1. `Q` is looked up component by component **without following symlinks**
   (O-25). If a component of `Q` is missing it is created as an implicit
   directory (D-6). If a component exists and is not a directory (a symlink
   to a directory counts as "not a directory"), kext2 fails with exit 1,
   message `cannot add '<P>': '<component>' is not a directory` (O-25).
2. If `n` does not exist in `Q`, the entry is created.
3. If `n` exists and both the existing node and the new entry are
   directories, the directories are merged: the existing node is kept and the
   new entry's children are inserted into it by this same rule. If the
   existing directory is *implicit*, it adopts the new entry's permission
   bits, uid, gid and mtime and stops being implicit (except the root
   directory: it adopts permission bits, uid and gid but its times stay
   `T_fs`, O-39). If it is not implicit, the new entry's own metadata is
   discarded.
4. Otherwise (`n` exists and at least one of the two is not a directory)
   the new entry is ignored, with the warning `ignoring duplicate entry <n>`
   (O-24). If the ignored entry is a directory, its whole subtree is ignored.

Rationale: this reproduces xgenext2fs for `-d` layers exactly (O-24). For
tar archives xgenext2fs implements the same "existing entry wins" rule but
also mis-applies the ignored entry's mode/owner to the parent directory; kext2
does not reproduce that bug (D-5). See Q-1 for the alternative "last wins"
semantics.

R-IN-4. **Implicit directories** are directories that kext2 creates because a
path needs them (root, layer targets, missing tar parents). They have mode
`040755`, uid 0, gid 0, mtime `T_fs`; `-U`/`-P` do not apply to them.

R-IN-5. Device-table layers do not use R-IN-3; they use the fix-up rule of
§5.3.

R-IN-6. If the output image file is itself encountered as a regular file
inside a `-d` source tree (same device and inode number), that entry is
skipped with a warning. If the output path designates the same file as a
`-a`, `-D` or `-x` input, kext2 fails with exit 1 before writing anything.

## 5. Input formats

### 5.1 Host directories (`-d src[:path]`)

R-DIR-1. `src` MUST be a directory (following a symlink given on the command
line is allowed); otherwise error `<src> should be a directory`, exit 1
(O-28). The directory `src` itself contributes no metadata: its
children are inserted into the target directory (O-39: the image root never
takes the host root's mode, owner or mtime).

R-DIR-2. The tree under `src` is walked with `lstat` semantics: symlinks are
never followed. For every entry kext2 records:

| Field | Value |
|---|---|
| type | from `st_mode`: regular, directory, symlink, char device, block device, fifo, socket. Other types (e.g. whiteouts): skipped with a warning. |
| permission bits | `st_mode & 07777`; for symlinks always `0777` (O-21) |
| uid, gid | `st_uid`, `st_gid` (32 bits, D-1) |
| mtime | `st_mtime` seconds (sub-second part dropped, i.e. floor) |
| size, data | regular files: `st_size` and contents |
| target | symlinks: `readlink` bytes (no terminating NUL) |
| major, minor | devices: decoded from `st_rdev` with the host's major/minor macros |

R-DIR-3. Hard links: non-directory entries with `st_nlink > 1` that share
`(st_dev, st_ino)` with an entry already inserted by **any** `-d` layer of this
run become additional names of the same node (O-29). The link count written
to the image is the number of names in the image, never `st_nlink`.

R-DIR-4. Unreadable directories or files (open/read failure), and files whose
size or type changed between the scan and the write phase, are fatal errors
(exit 1). (xgenext2fs crashes on an unreadable file, O-28; D-20.)

R-DIR-5. Directory read order MUST NOT matter (children are sorted in the
plan phase). Sockets and fifos are copied as such; devices as such (only their
numbers are read).

### 5.2 Tar archives (`-a src[:path]`)

R-TAR-1. `src` names a regular file or `-` (standard input). Anything else:
error `<src> should be a file`, exit 1. Standard input (and any non-seekable
or compressed source) MAY be spooled to a temporary file so that file data can
be read in the write phase; spooling MUST NOT change the result. (xgenext2fs
cannot size images from standard input, O-30; D-22.)

R-TAR-2. **Compression** is detected from the first bytes of `src`, never
from its name (O-26):

| Magic (hex) | Format |
|---|---|
| `1F 8B` | gzip (all concatenated members are decoded) |
| `42 5A 68` (`BZh`) | bzip2 (all concatenated streams) |
| `FD 37 7A 58 5A 00` | xz (all concatenated streams) |
| `28 B5 2F FD` | zstd (all frames; skippable frames ignored) |
| anything else | uncompressed tar |

A corrupt compressed stream is a fatal error.

R-TAR-3. **Records.** The archive is a sequence of 512-byte headers, each
followed by its data padded to a multiple of 512 bytes. A header whose 512
bytes are all zero ends the archive (a second zero block, trailing data, or a
missing second block are all accepted and ignored). A header whose checksum
(sum of its bytes with the 8-byte checksum field counted as spaces, computed
both with unsigned and with signed bytes; either may match) does not match is
a fatal error. A premature end of input is a fatal error.

R-TAR-4. **Header fields** (offsets within the 512-byte header):

| Offset | Size | Field |
|---|---|---|
| 0 | 100 | name (NUL-terminated if shorter) |
| 100 | 8 | mode |
| 108 | 8 | uid |
| 116 | 8 | gid |
| 124 | 12 | size |
| 136 | 12 | mtime |
| 148 | 8 | checksum |
| 156 | 1 | typeflag |
| 157 | 100 | linkname |
| 257 | 6 | magic: `ustar\0` (POSIX) or `ustar ` + version `" \0"` (GNU / old GNU); anything else = v7 |
| 263 | 2 | version |
| 265 | 32 | uname (ignored) |
| 297 | 32 | gname (ignored) |
| 329 | 8 | devmajor |
| 337 | 8 | devminor |
| 345 | 155 | prefix (POSIX ustar only; joined as `prefix/name` when non-empty) |
| 386 | 96 | old GNU sparse map (4 × {offset 12, numbytes 12}) — GNU magic only |
| 482 | 1 | old GNU "is extended" flag |
| 483 | 12 | old GNU real size |

Numeric fields are octal ASCII, optionally surrounded by spaces/NULs; or GNU
base-256 if the first byte has bit 7 set (big-endian two's complement over the
remaining bits; byte `0xFF` first means negative). uname/gname are ignored:
numeric ids are used (O-26).

R-TAR-5. **Extended headers.**
* GNU `L` (long name) and `K` (long link name): the data (NUL-terminated) is
  the name/linkname of the next non-meta header.
* POSIX `x` (per-file) and `g` (global) pax headers: records
  `"<len> <key>=<value>\n"`. Keys used: `path`, `linkpath`, `size`, `uid`,
  `gid`, `mtime` (decimal, may be negative and/or fractional; floor to whole
  seconds), and the GNU sparse keys of R-TAR-7. Values are raw bytes (no
  charset conversion). `g` records apply to every following member unless
  overridden by `x`/header values. Unknown keys (including `atime`, `ctime`,
  `SCHILY.*`, `LIBARCHIVE.*`, `hdrcharset`) are ignored.
* Precedence: pax `x` > pax `g` > GNU `L`/`K` > header fields.

R-TAR-6. **Typeflags.**

| Typeflag | Meaning in kext2 |
|---|---|
| `0`, NUL, `7` | regular file (a name ending in `/` with these flags is a directory) |
| `1` | hard link to `linkname` (R-TAR-9) |
| `2` | symlink to `linkname` |
| `3` / `4` | character / block device (`devmajor`, `devminor`) |
| `5` | directory |
| `6` | fifo |
| `S` | GNU sparse regular file (R-TAR-7) |
| `D` | GNU dumpdir: directory (its data is ignored) |
| `L`, `K`, `x`, `g` | meta headers (R-TAR-5) |
| `V` | volume label: ignored |
| `M` | multi-volume continuation: fatal error |
| anything else | regular file, with a warning |

R-TAR-7. **Sparse files.** The logical file is the real size with holes; the
member data contains only the listed data regions, concatenated in map order.
Supported encodings (all verified against GNU tar 1.35 output, O-26):
* old GNU `S`: map in header bytes 386–481 (up to 4 entries), continued in
  512-byte extension headers (21 entries of 24 bytes, flag at byte 504) while
  the "is extended" flag is set; real size at 483;
* pax 0.0: repeated `GNU.sparse.offset` / `GNU.sparse.numbytes` pairs,
  `GNU.sparse.size` = real size;
* pax 0.1: `GNU.sparse.map` = comma-separated offset,numbytes list,
  `GNU.sparse.size`;
* pax 1.0: `GNU.sparse.major=1`, `GNU.sparse.minor=0`, `GNU.sparse.name` (the
  real name), `GNU.sparse.realsize`; the map is at the start of the member data
  as decimal numbers separated by `\n` (count, then offset/numbytes pairs),
  padded to 512 bytes.
Regions MUST be within the real size and in increasing, non-overlapping
order; otherwise fatal error. Bytes not covered by a region are zero. Holes
are not automatically holes in the image: only `-z` decides (§8.6).

R-TAR-8. **Names.** The member name is normalised: split on `/`; drop empty
and `.` components (this removes leading `/`, `./` and duplicate slashes);
if any component is `..` the member is skipped with a warning (D-7). The
resulting component list (possibly empty) is the image path relative to the
layer target. An empty list denotes the target directory itself: if the
member is a directory it applies to the target directory per R-IN-3 step 3
(this is how `./` sets the root's mode, uid and gid, O-39); a non-directory
member with an empty name is skipped with a warning.

R-TAR-9. **Hard links** (typeflag `1`): `linkname` is normalised like a name
(relative to the layer target; a leading `/` is stripped). It MUST resolve —
without following symlinks — to an existing non-directory node, otherwise
fatal error (exit 1): a missing target (O-25: xgenext2fs fails too) or a
directory target (kernel/e2fsck forbid directory hard links, O-20; D-3). Any
data attached to a hard-link member is ignored. If the link's own name already
exists, R-IN-3 step 4 applies (ignored).

R-TAR-10. **Metadata.** Permission bits `mode & 07777` (symlinks: `0777`,
O-21); uid/gid from header or pax, 32-bit (D-1); mtime from header or pax;
size from header/pax/sparse real size.

### 5.3 Device tables (`-D src[:path]`)

R-DT-1. `src` names a regular file or `-`. Format: text lines. A line whose
first non-blank character is `#`, or that is blank, is ignored. Otherwise the
line is split on blanks (spaces/tabs) into tokens:

```
name type mode uid gid major minor start inc count
```

R-DT-2. Parsing (reproduces O-27 exactly):
* `name` (token 1), `type` (token 2) and `mode` (token 3) are required.
  `type` MUST be a single character. `mode` MUST begin with an octal digit;
  its value is the longest prefix of octal digits (`0`–`7`); if the token
  continues with any other character, parsing ends there (all later fields
  are 0; e.g. `6a0 5 5` gives mode `06`, uid 0, gid 0, as O-27 shows). If a
  required token is missing or malformed the line is skipped with the warning
  `device table line <n> skipped: bad format for entry '<name>'`.
* Tokens 4–10 (`uid gid major minor start inc count`) are optional unsigned
  decimal numbers. Parsing proceeds left to right; the first token that is
  absent, is `-`, or does not start with a decimal digit **ends** parsing: it
  and every later field are 0. A token that starts with digits followed by
  other characters yields the leading digits' value and ends parsing.
  Extra tokens after `count` are ignored.
* `mode` is masked with `07777` (O-27).
* Values out of range (uid/gid > 4294967294, major > 4095, minor > 1048575,
  see §8.10) skip the line with a warning (D-25).

R-DT-3. Types: `f` regular file (empty), `d` directory, `c` character device,
`b` block device, `p` fifo, `s` socket. `l` (symlink) lines are skipped with a
warning because an empty symlink target is invalid (O-20, D-24). Any other
type: line skipped with warning `bad type '<t>' for entry '<name>'`.

R-DT-4. `name` is an image path relative to the layer target (a leading `/`
is ignored; `.` and empty components dropped). If it contains a `..`
component, or its parent directory does not exist (device tables never create
parents), or the parent is not a directory, the line is skipped with the
warning `device table line <n> skipped: can't find directory '<parent>' to
create '<base>'` (O-27).

R-DT-5. Series: if `count` = 0 the line describes one node named `name` with
the given major/minor. If `count` > 0 it describes the nodes `name<i>` (decimal
`i` appended without separator) for every integer `i` with `start ≤ i < count`,
in increasing `i`, with minor number `minor + i·inc − start` and the given
major (O-27; note `count` is an exclusive upper bound, not a count — Q-9). If
`start ≥ count` nothing is described. A computed minor outside 0…1048575
skips that node with a warning.

R-DT-6. For each described node at path `P`:
* if `P` exists (any type, including the root and nodes created earlier in the
  same table): **fix-up** — set its permission bits to `mode`, its uid and gid;
  keep its type, its times, its content, its device numbers (O-27). This is
  the only way a symlink's permission bits can differ from `0777`.
* otherwise create a node of the given type with permission bits `mode`,
  `uid`, `gid`, device numbers (for `c`/`b`), size 0, mtime = the device-table
  file's own `st_mtime` (for `-`, `T_fs`) (O-27). A `d` node is an empty
  directory (non-implicit).

R-DT-7. `-U`, `-P` and `-q` never apply to device-table creations or fix-ups
(O-22).

### 5.4 Squashing (`-U`, `-P`, `-q`)

R-SQ-1. `-U u`: every node inserted by a `-d` or `-a` layer (and the target
directory's metadata applied from a tar root entry, R-TAR-8) gets uid = gid =
`u` (O-22). `-P`: for those nodes, unless the node is a symlink, permission
bits become `mode & ~077` (group and other bits cleared; setuid, setgid,
sticky and owner bits kept) (O-22). `-q u` = `-U u` + `-P`. Squashing is
applied when the node is inserted; nodes ignored by R-IN-3 are not touched.

R-SQ-2. Squashing never applies to: the default root metadata, implicit
directories, lost+found (§7.2), device-table creations and fix-ups.

## 6. Timestamps

### 6.1 Filesystem time

R-TIME-1. `T_fs` is chosen in this order:
1. `-f`: 0;
2. `--timestamp S`: S;
3. `--clamp-timestamps S`: S;
4. `SOURCE_DATE_EPOCH` set and non-empty: its value, which MUST be a decimal
   integer in 0…2³¹−1, otherwise fatal error (D-9; xgenext2fs uses `atoi`,
   O-16);
5. otherwise the current wall-clock time in whole seconds, clamped to
   0…2³¹−1.

R-TIME-2. `SOURCE_DATE_EPOCH` alone does not clamp file times (same as
xgenext2fs, O-16).

### 6.2 Inode times

R-TIME-3. Every inode has `i_atime = i_ctime = i_mtime = t`, `i_dtime = 0`,
where `t` is (D-8; xgenext2fs uses the host ctime for `-d` and 0 for tar,
O-17):

| Node | `t` before clamping |
|---|---|
| root directory | `T_fs` (always, O-39) |
| lost+found, implicit directories | `T_fs` |
| from `-d` | host `st_mtime` |
| from `-a` | member mtime |
| created by `-D` | device-table file mtime (R-DT-6) |
| from `-x` (other than the root) | the starting image's `i_mtime` (signed) |

R-TIME-4. Then, in this order: with `-f` every `t` is 0; with `--timestamp S`
every `t` is S; with `--clamp-timestamps S` every `t > S` becomes S. Finally
every `t` is clamped to the signed 32-bit range −2³¹…2³¹−1 (the kernel reads
inode times as signed 32-bit values and limits them to that range:
`inode.c:ext2_iget`, `super.c:ext2_fill_super` `s_time_min/s_time_max`;
D-9) and stored as its 32-bit two's-complement pattern.

R-TIME-5. Superblock times: `s_wtime = s_lastcheck = T_fs`; `s_mtime = 0`
(never mounted). No other time is stored anywhere.

## 7. Inodes, reservation, geometry and size

Definitions used below: `bs` block size; `fdb` = 1 if `bs` = 1024 else 0
(`s_first_data_block`); `ipb = bs/128` inodes per block; `p = bs/4` block
numbers per indirect block.

### 7.1 Content requirements and inode count

R-SIZE-1. After the tree is final (R-RES-3 applied), compute:
* `U` = number of distinct inodes other than the root (a hard-linked node
  counts once; lost+found counts when it exists);
* `need_inodes = 10 + U` (inodes 1–10 are reserved, R-FMT-9);
* `D` = total number of blocks the content occupies = the sum over all
  distinct inodes, including the root, of `nb(x)` where `nb(x)` is the length
  of x's layout sequence (§8.5): data blocks plus indirect blocks for regular
  files and directories, 1 for a slow symlink, 0 otherwise.

R-SIZE-2. Requested inode count `I`:
* without `-b`: `I = max(need_inodes, N)` where `N` is the `-N` value (0 if
  absent); `-i` is ignored (O-8: xgenext2fs ignores `-i` without `-b`);
* with `-b B`: `I = max(need_inodes, N, floor(B·bs / i))` where the last term
  is present only if `-i i` is given (O-8).
The geometry rounds `I` up (§7.3); the final `s_inodes_count` is `G·ipg ≥ I`.

### 7.2 Reserved blocks and lost+found

R-RES-1. **Provisional lost+found.** If `m > 0` (the `-m` value, default 5)
and `-x` is not given, the root receives, before the first layer, a child
`lost+found`: a directory with permission bits `0700`, uid 0, gid 0, times
`T_fs`, not implicit, marked *generated*. Layers then treat it like any
existing node (O-4: xgenext2fs creates it before the layers): by R-IN-3 an
input `lost+found` directory merges into it and its own metadata is
discarded, an input non-directory `lost+found` is ignored; device tables may
fix it up or create nodes inside it. It is never squashed. If `m = 0` no
lost+found is generated (an input lost+found is then an ordinary directory).

R-RES-2. **Reserved count.** After all layers, the generated lost+found is
*pristine* if it has no children and was not fixed up. Let `D0`, `U0` be `D`
and `U` (R-SIZE-1) of the tree in which a pristine lost+found is left out and
a non-pristine one counts as an ordinary directory (packed size), and
`N0 = SIZE(D0, 0, max(10 + U0, N))` (§7.4; the `-i` term is never used for
`N0`). The reserved block count is `R = floor(N0 · m / 100)`. `R` is computed
this way also when `-b` is given (O-3, O-5).

R-RES-3. **Keeping it.** If `R > 0`, the generated lost+found stays; its
directory data has `max(16, packed)` blocks where `packed` is the number of
blocks its entries need (R-DIRENT-4); the blocks after the packed ones are
empty directory blocks (§8.7) (O-4: xgenext2fs uses 16 blocks for every
block size). With `p ≥ 256`, 16 data blocks need one indirect block, so
`nb(lost+found) ≥ 17`. If `R = 0`, a pristine lost+found is removed, and a
non-pristine one stays as an ordinary directory of packed size (xgenext2fs
would not have created it, so it would carry the input's metadata instead —
a corner case of tiny images, Q-8). lost+found therefore exists iff `R > 0`
in every case without an input lost+found (O-4).

R-RES-4. After geometry is fixed, `s_r_blocks_count = min(R, floor(B/2))`
(e2fsck rejects a superblock whose reserved count exceeds half the blocks,
observed with xgenext2fs `-m 200`, O-5; D-14).

### 7.3 Geometry

R-GEO-1. `has_super(g)` is true for g = 0, g = 1, and every g that is a power
of 3, 5 or 7 (`balloc.c:ext2_group_sparse`, `ext2_bg_has_super`; kext2 always
sets `sparse_super`, §8.1).

R-GEO-2. `GEOMETRY(B, I)` → VALID (with the fields below) or INVALID (with a
capacity `C`):

```
N      := B − fdb                       # blocks covered by groups (must be ≥ 1)
Gmin   := max(1, ceil(I / (8·bs)))      # an inode bitmap holds 8·bs bits
bpg    := min(8·bs, roundup(ceil(N / Gmin), 8))
loop while bpg ≥ 8:
    G     := ceil(N / bpg)
    ipg   := max(16, roundup(ceil(I / G), ipb))
    itb   := ipg / ipb                  # inode-table blocks per group
    gdb   := ceil(G·32 / bs)            # group-descriptor blocks
    ov(g) := (has_super(g) ? 1 + gdb : 0) + 2 + itb
    size(g) := bpg for g < G−1;  size(G−1) := N − (G−1)·bpg
    C     := N − Σ_g ov(g)
    if ipg ≤ 8·bs:
        if bpg ≤ itb + 3 or C ≤ 0: return INVALID(C)
        if size(g) > ov(g) for every g: return VALID
    bpg := bpg − 8
return INVALID(C)
```

Each rule's reason:
* `bpg ≤ 8·bs`, `ipg ≤ 8·bs`, `ipg ≥ ipb`, `bpg > itb + 3`, `s_inodes_count
  = G·ipg`, groups = `ceil((B − fdb)/bpg)`: checked by
  `super.c:ext2_fill_super`.
* `bpg` and `ipg` multiples of 8: e2fsprogs reads each group's bitmap as
  `bpg/8` (resp. `ipg/8`) bytes; a prototype with `bpg` = 1001 or 1167 made
  e2fsck report bitmap differences (verified during this specification).
* `ipg` multiple of `ipb`: the kernel computes the inode-table length as
  `ipg/ipb` rounded down and would read inodes past it.
* `ipg ≥ 16`: inodes 1–11 (reserved inodes and lost+found) MUST be in group 0;
  with `ipg` = 8 e2fsck reported reserved inodes 9 and 10 as invalid
  (verified); xgenext2fs also never uses fewer than 16 (O-2).
* Every group, including the last, holds its own metadata plus at least one
  more block (`ext2_check_descriptors` requires bitmaps and inode table inside
  the group). Lowering `bpg` by 8 grows the last group by `8·(G−1)` blocks and
  keeps `G` (or increases it) until this holds.
* When the inode count dictates the group count (`Gmin > ceil(N/(8·bs))`)
  groups are made smaller and evenly sized so that `ipg ≤ 8·bs`, as
  xgenext2fs does (O-34); otherwise groups are full (`8·bs`) and the last
  group takes the remainder.
* A single-group filesystem gets `bpg = roundup(N, 8)` (may exceed `N`), as
  xgenext2fs does (O-2); verified accepted by kernel and e2fsck.

R-GEO-3. Group `g` starts at block `start(g) = fdb + g·bpg`. Its layout:
`[superblock copy][GDT copy (gdb blocks)]` if `has_super(g)`, then block
bitmap, inode bitmap, inode table (`itb` blocks), then data blocks up to
`start(g) + size(g) − 1`. For g = 0 with `bs` = 1024 the superblock is block 1
(block 0 is the boot block, outside every group); with `bs` ≥ 2048 the
primary superblock is at byte 1024 of block 0.

R-GEO-4. The **data area** is the concatenation, for g = 0…G−1, of each
group's data blocks in increasing order. A data index `c` ∈ [0, C) maps to a
block; `C` is the capacity.

R-GEO-5. Hard limits (fatal error when exceeded): `B ≤ 2³²−1`; `G·ipg ≤ 2³²−1`;
GEOMETRY VALID.

### 7.4 Size

R-SIZE-3. `SIZE(D, F, I)` returns a block count:

```
B := fdb + D + F + 4 + ceil(I / ipb)
loop:
    r := GEOMETRY(B, I)
    if r is VALID and r.C ≥ D + F: return B
    B := B + max(1, D + F − r.C)
    if B > 2³²−1: fatal error "filesystem too large"
```

The increment is the capacity deficit, so the result is the smallest `B` in
practice (adding one block never adds more than one block of capacity, except
across inode-table rounding steps); equality with this algorithm, not
absolute minimality, is normative.

R-SIZE-4. **Without `-b`**: `B = SIZE(D, R + X, I)` where `X` is the `-r`
value (0 if absent). Thus the image has at least `R + X` free blocks (D-10,
D-12; xgenext2fs instead uses `N0 + R`, which can be too small, O-3).

R-SIZE-5. **With `-b B`**: the image has exactly `B` blocks. If
`GEOMETRY(B, I)` is INVALID or its capacity is `< D`, kext2 fails with exit 1
and the message `image too small: <B> blocks requested, at least <M> needed`
where `M = SIZE(D, 0, I)` (D-11; xgenext2fs silently raises `-b` to its own
estimate, O-6, see Q-2). If the failure is caused by the inode count (GEOMETRY
INVALID for every larger `B` with `D` fitting), the message SHOULD say "too
many inodes". `-r` is ignored with a warning.

R-SIZE-6. Free space is not required to cover `R` when `-b` is given; `R` is
only clamped (R-RES-4).

R-SIZE-7. Holes (`-z`) are excluded from `D`, so with `-z` the computed size
reflects allocated blocks, not file sizes (D-12; xgenext2fs sizes by file
size, O-14).

Worked example (1 KiB blocks, `-d t1` where t1 holds one 6-byte file, a
fifo, a symlink, a hard link and an empty subdirectory): `D0 = 3` (root dir,
file, subdir), `U0 = 4`, `need = 14`; `SIZE(3, 0, 14)`: start B = 1+3+0+4+2 =
10 → N = 9, Gmin = 1, bpg = 16, G = 1, ipg = 16, itb = 2, gdb = 1, ov = 6,
C = 3 ≥ 3 → `N0 = 10`; `R = floor(10·5/100) = 0` → no lost+found; `B = 10`.
Blocks: 0 boot, 1 superblock, 2 GDT, 3 block bitmap, 4 inode bitmap, 5–6 inode
table, 7 root dir, 8 file, 9 subdir. (xgenext2fs produces the same block
count for this tree, O-2.)

## 8. On-disk format

### 8.1 Revision and features

R-FMT-1. Revision 1 (`EXT2_DYNAMIC_REV`), minor revision 0, 128-byte inodes,
first non-reserved inode 11.
* Rev 0 would forbid feature flags (the kernel warns, `ext2_fill_super`) and
  `filetype`/`sparse_super` are wanted; rev > 1 is not supported by the
  kernel (`ext2_setup_super`).
* Inode size 128 (`EXT2_GOOD_OLD_INODE_SIZE`): accepted by the kernel (power
  of two, ≥ 128, ≤ bs: `ext2_fill_super`), needs no `i_extra_isize`
  handling, and is what xgenext2fs writes (O-2). Consequence: inode times are
  signed 32-bit seconds (R-TIME-4).

R-FMT-2. Feature flags (all three words): only bits the kernel lists as
supported (`ext2.h`: `EXT2_FEATURE_COMPAT_SUPP`, `..._INCOMPAT_SUPP`,
`..._RO_COMPAT_SUPP`) and that e2fsck accepts.

| Word | Bit | Name | kext2 | Why |
|---|---|---|---|---|
| compat | 0x0008 | ext_attr | never | no xattrs (§1) |
| incompat | 0x0002 | filetype | set, **except** when `s_creator_os` = 1 (Hurd) | lets `readdir` return the entry type without reading the inode (`dir.c:ext2_readdir`); not set for Hurd for compatibility with the Hurd translator (Q-12); xgenext2fs never sets it (D-23) |
| incompat | 0x0010 | meta_bg | never | not needed below the limits of R-GEO-5 |
| ro_compat | 0x0001 | sparse_super | always | backups only in groups 0, 1, 3ⁿ, 5ⁿ, 7ⁿ (`balloc.c:ext2_bg_has_super`); less overhead; xgenext2fs does not set it (D-23) |
| ro_compat | 0x0002 | large_file | iff some regular file has `i_size > 2³¹−1` | the kernel sets it when it first writes such a file (`inode.c:ext2_write_inode`); e2fsck requires it then; same as xgenext2fs (O-35) |
| ro_compat | 0x0004 | btree_dir | never | — |

All other bits are 0.

### 8.2 Superblock

R-FMT-3. The primary superblock is the 1024 bytes at byte offset 1024 of the
image. Field values (`ext2.h:struct ext2_super_block`; offsets in hex):

| Off | Size | Field | Value |
|---|---|---|---|
| 0x00 | 4 | s_inodes_count | `G·ipg` |
| 0x04 | 4 | s_blocks_count | `B` |
| 0x08 | 4 | s_r_blocks_count | R-RES-4 |
| 0x0C | 4 | s_free_blocks_count | Σ group free blocks |
| 0x10 | 4 | s_free_inodes_count | Σ group free inodes |
| 0x14 | 4 | s_first_data_block | `fdb` (1 for 1 KiB blocks, else 0) |
| 0x18 | 4 | s_log_block_size | log2(bs) − 10 |
| 0x1C | 4 | s_log_frag_size | same as s_log_block_size (kernel requires equality) |
| 0x20 | 4 | s_blocks_per_group | `bpg` |
| 0x24 | 4 | s_frags_per_group | `bpg` |
| 0x28 | 4 | s_inodes_per_group | `ipg` |
| 0x2C | 4 | s_mtime | 0 |
| 0x30 | 4 | s_wtime | `T_fs` |
| 0x34 | 2 | s_mnt_count | 0 |
| 0x36 | 2 | s_max_mnt_count | 0xFFFF (−1: no mount-count check; the kernel only warns when the signed value is ≥ 0, `ext2_setup_super`; xgenext2fs writes 20, D-23) |
| 0x38 | 2 | s_magic | 0xEF53 |
| 0x3A | 2 | s_state | 1 (`EXT2_VALID_FS`) |
| 0x3C | 2 | s_errors | 1 (`EXT2_ERRORS_CONTINUE` = `EXT2_ERRORS_DEFAULT`; the kernel maps 0 to remount-ro, `ext2_set_options`; xgenext2fs writes 0, D-23) |
| 0x3E | 2 | s_minor_rev_level | 0 |
| 0x40 | 4 | s_lastcheck | `T_fs` |
| 0x44 | 4 | s_checkinterval | 0 |
| 0x48 | 4 | s_creator_os | `-o` value (default 0) |
| 0x4C | 4 | s_rev_level | 1 |
| 0x50 | 2 | s_def_resuid | 0 |
| 0x52 | 2 | s_def_resgid | 0 |
| 0x54 | 4 | s_first_ino | 11 |
| 0x58 | 2 | s_inode_size | 128 |
| 0x5A | 2 | s_block_group_nr | group number of this copy (0 in the primary) |
| 0x5C | 4 | s_feature_compat | 0 |
| 0x60 | 4 | s_feature_incompat | R-FMT-2 |
| 0x64 | 4 | s_feature_ro_compat | R-FMT-2 |
| 0x68 | 16 | s_uuid | all zero (R-FMT-4) |
| 0x78 | 16 | s_volume_name | `-L` bytes, NUL-padded (no terminator needed at 16 bytes) |
| 0x88 | 64 | s_last_mounted | zero |
| 0xC8 | 4 | s_algorithm_usage_bitmap | 0 |
| 0xCC | 1+1+2 | s_prealloc_blocks, s_prealloc_dir_blocks, padding | 0 |
| 0xD0 | 16+4+4+4 | journal uuid, inum, dev, last_orphan | 0 |
| 0xEC | 16 | s_hash_seed | zero (no `dir_index`) |
| 0xFC | 1+1+2 | s_def_hash_version, padding | 0 |
| 0x100 | 4 | s_default_mount_opts | 0 |
| 0x104 | 4 | s_first_meta_bg | 0 |
| 0x108 | 760 | reserved | zero. In particular ext4's `s_mkfs_time` (0x108) stays 0: ext2's structure declares this area reserved. |

R-FMT-4. **UUID** = 16 zero bytes. Rationale: reproducible by construction,
identical to xgenext2fs (O-2), accepted by the kernel (it mounts and reports
the all-zero UUID) and by `e2fsck -fn` (verified). Users who need a unique
UUID can run `tune2fs -U` afterwards. A content-derived UUID was considered and
rejected for now (it needs a hash over all data before the superblock can be
final; see Q-5).

R-FMT-5. **Backup superblocks**: in every group `g ≥ 1` with
`has_super(g)`, the 1024-byte superblock copy is at byte 0 of block
`start(g)`; the rest of that block is zero. Every copy is identical to the
primary except `s_block_group_nr = g`.

### 8.3 Group descriptor table and bitmaps

R-FMT-6. The GDT is `gdb` blocks: G descriptors of 32 bytes, then zeros. It
follows the superblock block in each `has_super` group (group 0: block
`fdb + 1`; group g: block `start(g) + 1`). All copies are identical.

| Off | Size | Field | Value |
|---|---|---|---|
| 0 | 4 | bg_block_bitmap | block of this group's block bitmap |
| 4 | 4 | bg_inode_bitmap | block of this group's inode bitmap |
| 8 | 4 | bg_inode_table | first inode-table block |
| 12 | 2 | bg_free_blocks_count | zero bits among the first `size(g)` bits of the block bitmap |
| 14 | 2 | bg_free_inodes_count | zero bits among the first `ipg` bits of the inode bitmap |
| 16 | 2 | bg_used_dirs_count | directories whose inode is in this group (root, lost+found included) |
| 18 | 14 | pad, reserved | 0 |

R-FMT-7. **Block bitmap** of group g (one block): bit `i` (byte `i/8`, bit
`i mod 8`, least significant first) describes block `start(g) + i`. Set:
metadata blocks of the group (superblock/GDT copies, bitmaps, inode table),
every allocated data or indirect block, and every bit `i ≥ size(g)` up to
`8·bs − 1` (padding; e2fsck reports unset padding, observed). Clear: free
blocks.

R-FMT-8. **Inode bitmap** of group g: bit `j` describes inode
`g·ipg + j + 1`. Set: inodes 1–10 (reserved; e2fsck counts them as used),
every used inode, and every bit `j ≥ ipg` up to `8·bs − 1` (padding). Clear:
free inodes.

### 8.4 Inodes

R-FMT-9. The inode table of group g holds `ipg` inodes of 128 bytes
(`itb` blocks). Inode number `n` is at index `(n−1) mod ipg` of group
`(n−1) div ipg` (`inode.c:ext2_get_inode`). Unused inode slots and the
reserved inodes 1 and 3–10 are all zero (inode 1, the bad-blocks inode, is
all zero as in xgenext2fs; accepted by e2fsck). Inode 2 is the root
directory. The generated lost+found (R-RES-1), when present, is inode 11.

R-FMT-10. Inode fields (`ext2.h:struct ext2_inode`, Linux `osd` variants):

| Off | Size | Field | Value |
|---|---|---|---|
| 0 | 2 | i_mode | type bits \| permission bits (07777). Types: 0x8000 regular, 0x4000 directory, 0xA000 symlink, 0x2000 char, 0x6000 block, 0x1000 fifo, 0xC000 socket |
| 2 | 2 | i_uid | uid & 0xFFFF |
| 4 | 4 | i_size | size & 0xFFFFFFFF (§8.5–8.9) |
| 8 | 4 | i_atime | R-TIME-3/4 |
| 12 | 4 | i_ctime | R-TIME-3/4 |
| 16 | 4 | i_mtime | R-TIME-3/4 |
| 20 | 4 | i_dtime | 0 |
| 24 | 2 | i_gid | gid & 0xFFFF |
| 26 | 2 | i_links_count | R-FMT-11 |
| 28 | 4 | i_blocks | `nb(x) · (bs/512)` — every allocated block of the inode, data and indirect, counted in 512-byte units (`inode.c` block accounting; e2fsck checks it) |
| 32 | 4 | i_flags | 0 |
| 36 | 4 | osd1 | 0 (Hurd: translator, must be 0) |
| 40 | 60 | i_block[15] | §8.5–8.9 |
| 100 | 4 | i_generation | 0 (the kernel uses a random value for new inodes, `ialloc.c:ext2_new_inode`; replaced by 0 for reproducibility) |
| 104 | 4 | i_file_acl | 0 |
| 108 | 4 | i_size_high / i_dir_acl | regular files: size >> 32; all other types: 0 (`inode.c:ext2_iget` uses it as the size high word only for regular files) |
| 112 | 4 | i_faddr | 0 |
| 116 | 1+1+2 | frag, fsize, pad (Hurd: mode_high) | 0 |
| 120 | 2 | i_uid_high | uid >> 16 (same offset for Linux and Hurd) |
| 122 | 2 | i_gid_high | gid >> 16 |
| 124 | 4 | reserved (Hurd: author) | 0 |

32-bit ids are read back by the kernel as `low | high << 16` unless mounted
with `nouid32` (`inode.c:ext2_iget`). (xgenext2fs truncates to 16 bits, O-18;
D-1.)

R-FMT-11. Link counts: a directory has 2 + number of child directories
(`.` plus its entry in the parent, plus each child's `..`); the root likewise
(its `..` points to itself and counts as its parent entry). Any other inode:
the number of directory entries naming it. A count above 32000
(`EXT2_LINK_MAX`, the kernel's `s_max_links`) is a fatal error (§8.10).

### 8.5 Block map, file data and holes

R-FMT-12. Block addressing (`inode.c:ext2_block_to_path`, `ext2_get_branch`):
logical block `L` of an inode maps through `i_block`:
* `L < 12`: `i_block[L]` (direct);
* `L − 12 < p`: `i_block[12]` → indirect block, entry `L − 12`;
* `L − 12 − p < p²`: `i_block[13]` → double-indirect, entry `(L−12−p) div p`
  → indirect, entry `(L−12−p) mod p`;
* else (`< p³`): `i_block[14]` → triple-indirect, entries
  `M div p²`, `(M div p) mod p`, `M mod p` with `M = L − 12 − p − p²`.
Indirect blocks are arrays of `p` 32-bit block numbers. A 0 entry at any level
is a hole (the kernel stops the lookup and returns zeros). Entries beyond the
last needed one are 0.

R-FMT-13. **Layout sequence** of an inode with set `S` of allocated logical
blocks (sorted increasingly): for each `L ∈ S` in order, first every indirect
node on `L`'s path that has not yet been emitted, from the top level down
(triple, double, single), then `L` itself. The inode's blocks are assigned
consecutive data indices in this order (§9.2), so every indirect block
immediately precedes the first data block it maps — the same order the
kernel's allocator produces when a file is written sequentially
(`inode.c:ext2_alloc_branch` allocates the missing indirect blocks and the
data blocks as one run). For a dense file: `d0…d11, IND, d12…d(11+p),
DIND, IND, …, TIND, DIND, IND, …`. An indirect block exists iff it maps at
least one allocated block (xgenext2fs also allocates empty indirect blocks for
holes, O-14; kext2 does not, D-23). `nb(x) = |S| +` number of indirect blocks.

R-FMT-14. Regular files: `S` = all `L < ceil(size/bs)`, except with `-z`
(§8.6). Each data block holds the file bytes `[L·bs, (L+1)·bs)`, the part past
`size` zero-filled. `i_size` = size (low/high words, R-FMT-10).

R-FMT-15. **Maximum file size** (`super.c:ext2_max_size`: the block tree
limit and the 2³²−1 sector limit of `i_blocks`, counting indirect blocks):

| bs | max size (bytes) | max logical blocks |
|---|---|---|
| 1024 | 17 247 252 480 | 16 843 020 |
| 2048 | 275 415 851 008 | 134 480 396 |
| 4096 | 2 196 873 666 560 | 536 346 110 |

A regular file larger than this (even sparse) is a fatal error.

### 8.6 Holes (`-z`)

R-FMT-16. Without `-z` every logical block below `ceil(size/bs)` is
allocated; source holes (sparse host files, tar sparse maps) are written as
zero blocks. With `-z`, a logical block is **not** allocated iff its `bs`
bytes (zero-padded past `size`) are all zero, whatever their origin (O-14:
xgenext2fs also turns explicit zero data into holes). This decision is
content-based so it is reproducible; an implementation MAY use
`SEEK_DATA`/`SEEK_HOLE` or the tar sparse map to avoid reading holes, but the
result MUST equal the content-based definition. A file of size > 0 may end up
with no allocated block at all (all-zero file); that is valid.

### 8.7 Directories

R-DIRENT-1. Directory entry format (`ext2.h:struct ext2_dir_entry_2`):

| Off | Size | Field |
|---|---|---|
| 0 | 4 | inode (0 = unused entry) |
| 4 | 2 | rec_len |
| 6 | 1 | name_len |
| 7 | 1 | file_type (with `filetype`; 0 without it) |
| 8 | name_len | name (no NUL) |

file_type: 1 regular, 2 directory, 3 char device, 4 block device, 5 fifo,
6 socket, 7 symlink. Without the `filetype` feature (Hurd) byte 7 MUST be 0
because readers then take name_len as a 16-bit field.

R-DIRENT-2. `rec_len(n) = (8 + n + 3) & ~3` (`EXT2_DIR_REC_LEN`). Every entry
satisfies the checks of `dir.c:ext2_check_folio`: `rec_len ≥ 12`, `rec_len`
multiple of 4, `rec_len ≥ rec_len(name_len)`, the entry does not cross a block
boundary, the inode number ≤ `s_inodes_count`, and the entries of each block
chain exactly to the end of the block. The directory `i_size` is a multiple
of `bs` (else `ext2_check_folio` reports a bad size).

R-DIRENT-3. Entry order in every directory: `.` (the directory itself), `..`
(its parent; the root's parent is itself), then every child in byte-wise
lexicographic order of names (R-MODEL-3), lost+found included. (xgenext2fs
uses input order; directory entry order is not content, §14.2.)

R-DIRENT-4. Packing: entries are placed in order; an entry is placed in the
current block if `used + rec_len(name_len) ≤ bs`, otherwise a new block is
started. Within a block every entry has `rec_len = rec_len(name_len)` except
the last, whose `rec_len` extends to the end of the block. `packed` is the
number of blocks used (≥ 1). For lost+found, blocks `packed … max(16,packed)−1`
are *empty directory blocks*: one entry with inode 0, rec_len = bs,
name_len 0, file_type 0, rest zero (the representation e2fsck and the kernel
use for an empty block). Directory `i_size = nblocks·bs`; `S` = all
`nblocks` logical blocks (never holes); indirect blocks per R-FMT-13.

R-DIRENT-5. Names are 1–255 bytes, contain neither `/` nor NUL, and are not
`.` or `..` (`EXT2_NAME_LEN`, `namei.c:ext2_lookup` refuses longer names). A
violating name is a fatal error (xgenext2fs writes such names and corrupts the
directory, O-20; D-3).

### 8.8 Symbolic links

R-FMT-17. Target length `len` MUST be 1 ≤ `len` ≤ `bs − 1` (the kernel
refuses longer targets: `namei.c:ext2_symlink` requires `len + 1 ≤ bs`; an
empty target is reported invalid by e2fsck, O-20). Violations are fatal
(D-3). `i_size = len`.
* **Fast symlink** iff `len ≤ 59` (`len + 1 ≤ 60 = sizeof(i_block)`,
  `namei.c:ext2_symlink`): the target bytes are stored in `i_block` (bytes
  40…40+len−1 of the inode), the remaining bytes of `i_block` zero,
  `i_blocks = 0`. The kernel recognises a fast symlink by `i_blocks = 0`
  (`inode.c:ext2_inode_is_fast_symlink`).
* **Slow symlink** otherwise: one data block holding the target followed by
  zeros (which supplies the terminating NUL the kernel's page symlink
  expects), `i_block[0]` = that block, `i_blocks = bs/512`. (xgenext2fs: 59
  fast, 60 and 61 slow, O-20.)

### 8.9 Devices, fifos, sockets

R-FMT-18. Character/block devices (`inode.c:ext2_write_inode`, decoding in
`ext2_iget`, encodings from `include/linux/kdev_t.h`): `i_size = 0`,
`i_blocks = 0`;
* if `major < 256` and `minor < 256` (old-valid): `i_block[0] = major·256 +
  minor`, all other `i_block` words 0;
* otherwise: `i_block[0] = 0`, `i_block[1] = (minor & 0xFF) | (major << 8) |
  ((minor & ~0xFF) << 12)`, all other words 0.
The kernel decodes `i_block[0]` with the old 8:8 encoding when it is non-zero
and `i_block[1]` with the new encoding otherwise; device 0:0 therefore encodes
as all zeros. `major ≤ 4095` and `minor ≤ 1048575` (kernel `dev_t` has 12/20
bits); larger values are fatal (D-2; xgenext2fs keeps only the low 8 bits of
each, O-19).

R-FMT-19. Fifos and sockets: `i_size = 0`, `i_blocks = 0`, `i_block` all zero.

### 8.10 Limits (all fatal, exit 1, with a message naming the path)

| Id | Limit | Source |
|---|---|---|
| R-LIM-1 | name 1–255 bytes, no `/` or NUL, not `.`/`..` | R-DIRENT-5 |
| R-LIM-2 | symlink target 1…bs−1 bytes | R-FMT-17 |
| R-LIM-3 | regular file size ≤ max (R-FMT-15) | `ext2_max_size` |
| R-LIM-4 | link count ≤ 32000 (also: ≤ 31998 subdirectories per directory) | `EXT2_LINK_MAX` |
| R-LIM-5 | uid, gid ≤ 4294967294 (4294967295 is the invalid id `(uid_t)−1`) | — |
| R-LIM-6 | device major ≤ 4095, minor ≤ 1048575 | `kdev_t.h` |
| R-LIM-7 | directory size ≤ 2³²−bs bytes | `i_size` is 32-bit for directories |
| R-LIM-8 | `B ≤ 2³²−1`, `s_inodes_count ≤ 2³²−1`, GEOMETRY valid | R-GEO-5 |
| R-LIM-9 | hard link to a directory | R-TAR-9 |

Device-table lines violating R-LIM-5/6 are skipped with a warning instead
(R-DT-2).

## 9. Allocation policy and reproducibility

### 9.1 Design rationale

The kernel allocates in two places, and kext2 adapts each to a build that
knows the whole tree in advance:

* **Inodes** (`ialloc.c`). `find_group_other` puts a non-directory inode in
  its parent directory's group if that group has free inodes and blocks, else
  searches other groups (a hash, then linearly). `find_group_orlov` spreads
  top-level directories over the groups with the fewest directories and
  above-average free space, starting at a **random** group, and keeps deeper
  directories near their parent unless the group is too full.
* **Blocks** (`inode.c:ext2_find_goal`, `ext2_find_near`, `balloc.c`). The goal
  for a file's next block is the block after its previous one; for a file's
  first block, a position in the inode's own group offset by a per-**process**
  "colour" (`pid % 16`); indirect blocks are allocated just before the data
  they map.

kext2 keeps the intent — data contiguous and in file order, inodes in the
group where their data is, top-level directories spread over the
filesystem when there is room — and removes every source of non-determinism:

| Kernel behaviour | kext2 replacement |
|---|---|
| Orlov's random start group for top-level directories | the k-th top-level directory (in name order) starts its data at group `floor(k·G/T)`, if there is room (R-ALLOC-3, phase B) |
| goal colour from `pid % 16` | none: data starts at the cursor |
| per-group free-space heuristics, debts, reservation windows | a single cursor over the data area; groups are filled in order |
| random `i_generation` | 0 |
| order of system calls / readdir order | sorted names, fixed traversal |

Because the whole data area is filled by one cursor that only moves forward,
there is no fragmentation: the capacity needed is exactly `D` (§7.1), which is
what makes the sizing of §7.4 exact, and every file's blocks form one run
(interrupted only where a group's metadata lies in the way).

### 9.2 Algorithm

R-ALLOC-1. State: a data cursor `c` (data index, R-GEO-4), initially 0; for
each group a next-free inode index `next[g]`, initially 0, except
`next[0] = 10` (inodes 1–10 reserved); `rem` = number of blocks still to
allocate, initially `D`.

R-ALLOC-2. Primitives:
* `cursor_group()` = the group containing data index `c`, or `G−1` if
  `c = C`.
* `alloc_inode(g0)`: for k = 0…G−1, g = (g0 + k) mod G: if `next[g] < ipg`,
  return inode `g·ipg + next[g] + 1` and increment `next[g]`. (Deterministic
  form of `find_group_other`'s fallback search; it cannot fail because
  `G·ipg ≥ I ≥ need_inodes`.)
* `place(x)`: if x already has an inode number (hard link), do nothing.
  Otherwise give x inode `alloc_inode(cursor_group())`, then assign data
  indices `c, c+1, …, c+nb(x)−1` to x's layout sequence (R-FMT-13) and advance
  `c` and decrease `rem` by `nb(x)`. If x is a directory, increment
  `bg_used_dirs_count` of its inode's group.

R-ALLOC-3. Order:
1. The root gets inode 2 and the first `nb(root)` data indices.
2. If the root's child `lost+found` is the generated node (R-RES-1), it is
   placed next with `alloc_inode(0)` instead of the cursor group, so it gets
   inode 11 (`ipg ≥ 16`), then `PROCESS(lost+found, false)`.
3. `PROCESS(root, true)`, skipping that lost+found.

`PROCESS(d, top)`, children of `d` in name order (R-DIRENT-3):
* Phase A: `place(x)` for every child x that is not a directory.
* Phase B: let `T` be the number of directory children of `d` handled in this
  phase. For the k-th of them (k = 0, 1, …, in name order), x:
  * if `top`: let `t` = the data index of the first data block of group
    `floor(k·G/T)`; set `c := max(c, min(t, C − rem))` (spread top-level
    directories over the groups, but never so far that the rest of the content
    would not fit);
  * `place(x)`, then `PROCESS(x, false)`.

R-ALLOC-4. Consequences (normative because they follow from R-ALLOC-3):
inode numbers increase in allocation order within each group; the files of a
directory get consecutive inode numbers and contiguous data, directly after
the directory's own blocks; a hard-linked inode is placed at the first of its
names in this traversal; spreading can only use the free blocks
(`C − rem − c` of them), so an image sized without `-b` distributes just its
`R + X` free blocks between the top-level subtrees (with `-m 0` and no `-r`
it is fully packed), while with a large `-b` each top-level subtree starts in
its own region with free space after it for later growth (the effect Orlov
aims at).

Example (from the prototype): `-d big1` (20 top-level directories × 30 files,
28 MB) with `-b 100000` (13 groups) spreads the 20 directories over all 13
groups, each group keeping 4000–7000 free blocks; without `-b` the same
content is packed into 4 groups.

### 9.3 Reproducibility

R-REPRO-1. The image bytes MUST be a deterministic function of: the logical
content of the inputs (§3: the tree, including data bytes, names, types,
permission bits, ids, mtimes, symlink targets, device numbers and hard-link
structure), the order and kind of layers, the options, and `T_fs`.

R-REPRO-2. They MUST NOT depend on: input paths and the names of `-d`
sources; directory read order; file creation order; host inode numbers and
device numbers (other than for detecting hard links); host atime and ctime;
tar member order (except where R-IN-3 or R-TAR-9 make order semantically
relevant: duplicates, hard-link targets); whether a tar is compressed and
with what; whether it is read from a file or from standard input; the number
of threads; locale; time zone; umask; the wall clock (when `-f`,
`--timestamp`, `--clamp-timestamps` or `SOURCE_DATE_EPOCH` is given); memory
contents (every byte not specified here is zero).

R-REPRO-3. Concretely fixed values: UUID zero, hash seed zero, `s_mkfs_time`
area zero, `s_mtime` 0, `s_mnt_count` 0, `i_generation` 0, padding bytes 0,
unused inode slots zero, slack after file data and directory entries zero,
unused indirect-block entries zero, free blocks zero (or the `-e` value).

R-REPRO-4. An implementation MAY use threads (e.g. to read or hash files in
parallel) but MUST produce the same bytes as the sequential algorithm.

## 10. Writing the image

### 10.1 Output file

R-OUT-1. The output is exactly `B·bs` bytes. If the image path is a regular
file or does not exist, kext2 SHOULD write a temporary file in the same
directory and rename it over the path on success; on any failure the
temporary file is removed and a pre-existing file at the path is left
unchanged (xgenext2fs leaves a corrupt image behind, O-36; D-21). If the path
is a block device it is written in place (every block, including zero
blocks, MUST then be written explicitly). `-` means standard output (R-OUT-4).

R-OUT-2. Regions that are zero (free blocks with fill 0, unused inode-table
space, gaps) MAY be left as holes in a regular output file (xgenext2fs does
this too: a 9 GB image occupied 9.2 MB, O-14).

R-OUT-3. With `-e v`, every block whose block-bitmap bit is clear (free) is
filled with byte `v` (O-13). Nothing else is affected: block 0 / the bytes of
block 0 outside the superblock, metadata blocks, slack inside allocated blocks
and holes of files (which are not blocks) stay zero.

R-OUT-4. Because the plan fixes every block before writing, the image can be
emitted strictly in increasing block order (metadata of each group, then its
data blocks in allocation order, which is also file order). kext2 MUST support
`-` (standard output, possibly a pipe) by emitting sequentially.
(xgenext2fs fails on a pipe, O-30.)

### 10.2 Write phase

R-OUT-5. For each group in order: superblock and GDT copies (if
`has_super`), block bitmap, inode bitmap, inode table, then the data area in
data-index order. A data index holds either a directory block (generated from
the plan), an indirect block (generated from the plan), a slow-symlink block,
or a file data block (read from the input). Each regular file is read once,
sequentially from offset 0, while its blocks are written.

R-OUT-6. If while writing a file kext2 reads a different number of bytes than
the scan recorded, or (with `-z`) a block's zero/non-zero status differs from
the scan, it fails (exit 1).

### 10.3 Performance requirements

R-PERF-1. Planning is O(E log E) for E directory entries (one sort per
directory) and O(I + G) otherwise. No operation may be quadratic in the
number of entries of a directory or of the filesystem (xgenext2fs needs 70 s
for 20 000 files in one directory, O-33).

R-PERF-2. Memory is O(number of nodes + total name bytes + G), independent
of data size. A file's allocation is stored as runs, not per-block lists;
zero-block maps for `-z` are run-length or bit maps.

R-PERF-3. Each input file's data is read once in the write phase (plus once
in the scan phase only when `-z` needs the zero-block map or the source is a
non-seekable/compressed stream that must be spooled or decompressed twice).
Writes are sequential and in large chunks (≥ 64 KiB). Holes in the input and
in the output are skipped, never materialised in memory.

R-PERF-4. Targets: see AT-PERF (§14.5).

## 11. Phase 2 features

### 11.1 Starting image (`-x image`)

R-X-1. (Phase 2.) `-x` may be given once; a second `-x` is a usage error.
The starting image MUST be an ext2 filesystem with `s_magic` 0xEF53,
revision 0 or 1, inode size 128…bs (power of two), and features ⊆ {filetype,
sparse_super, large_file} (anything else, including `ext_attr` with xattr
blocks in use, journal, extents: fatal error).

R-X-2. Its whole tree (everything reachable from inode 2, hard-link
structure included) becomes the initial tree (R-IN-1), node attributes taken
from the inodes (32-bit ids, signed mtime, data read through the block map,
holes preserved as zero data, fast/slow symlinks, both device encodings).
lost+found from the starting image, if any, is ordinary content. Entries from
the image count as non-implicit (R-IN-3), i.e. they win over later layers
(O-31).

R-X-3. Parameters are taken from the starting image and the corresponding
options are ignored with a warning (O-31): block size (`-B`), block count
(`-b`; xgenext2fs ignores it — kext2 keeps `B` from the image but fails if the
new content does not fit), inode count (`-N`, `-i`: the image's
`s_inodes_count` is used as `N`), `-m`/`-r` (`R` = the image's
`s_r_blocks_count`, and no lost+found is generated), `-L`, `-o`, UUID.

R-X-4. The output is a complete re-layout by §7–§10 (never an in-place
update), so it is valid even if the starting image had a different layout.
If the output path is the starting image, R-IN-6 applies (the starting image
MUST be fully read — or spooled — before the output replaces it; the temporary
file + rename of R-OUT-1 achieves this).

### 11.2 Block maps (`-g path`)

R-G-1. (Phase 2.) After the image is written, for each `-g` path (an image
path resolved component by component from the root, `..` resolved to the
parent, symlinks not followed), kext2 writes a file in the current working
directory named after the path with every `/` replaced by `_` plus `.blk`
(`-g /d3/f5` → `_d3_f5.blk`; `-g sub` → `sub.blk`) (O-32). Content (ASCII):
the inode's `i_size` in decimal, `:`, then for every logical block
`L = 0 … ceil(i_size/bs) − 1` a space and the block number holding it (0 for a
hole), then `\n`. Fast symlinks, devices, fifos and sockets list no blocks
(`5:\n`, `0:\n`). Indirect blocks are not listed. A path that does not exist
is a fatal error (exit 1) (O-32). (xgenext2fs omits some hole entries,
D-26.)

## 12. Black-box observations of xgenext2fs 1.5.6

All observations were made on macOS (arm64) with `/opt/homebrew/bin/xgenext2fs`
(reports `xgenext2fs 1.5.6`), inspecting images with e2fsprogs 1.47.2
(`dumpe2fs`, `debugfs`, `e2fsck -fn`) and loop mounts (Linux 7.0, ext2 via the
ext4 driver) in the `kext2-tools` container. Test trees (built with shell
commands; `tar` archives built with GNU tar 1.35 or Python `tarfile`):

* `t1`: `a.txt` ("hello\n"), `hard` (hard link to a.txt), `link -> a.txt`,
  `fifo`, empty directory `sub`.
* `one`: a single 2-byte file `f`. `empty`: an empty directory. `s2`: `d/f`.
* `big1`: 20 directories × 30 random files of 0 B–300 KB (28.8 MB).
* `hd`: one directory with 20 000 empty files (31-byte names).
* `gnu.tar`, `posix.tar`, …: a tree with a setuid file owned 100000:70000,
  devices 1:3, 8:0, 300:70000, 4095:1048575, a fifo, a socket (dropped by
  tar), a 255-byte name, a 300+-byte path, symlinks of 200 and 1500 bytes, a
  hard link, a 20 MiB sparse file, archived as `gnu`, `oldgnu`, `posix`
  (pax), `ustar`, `v7`, and gzip/bzip2/xz/zstd-compressed copies.

| Id | Command (abbreviated) | Observation |
|---|---|---|
| O-1 | `xgenext2fs --help`, `-h`, `-V`, no args, `a b`, `-Z`, `--bogus`, `--b 300`, `--size 300`, `-b 100 -b 200`, `-zfb 300`, `-b300` | Help text (as in R-CLI-3) on **stderr**, exit 0; `-V` prints `xgenext2fs 1.5.6` on stdout, exit 0. No operand: "Not enough arguments", exit 1; two operands: "Too many arguments", exit 1; unknown option / ambiguous prefix: exit 1. GNU getopt_long: unambiguous prefixes accepted, clustering, attached arguments, permutation (`-b 300 img -d dir` works), last value wins. |
| O-2 | `xgenext2fs -d t1 x.img` | 10 blocks of 1 KiB, 16 inodes, 1 group, `s_blocks_per_group` 16 (= roundup(B−1, 8)), `s_inodes_per_group` 16, rev 1, **no features**, `s_errors` 0, `s_max_mnt_count` 20, `s_state` 1, UUID all zero, `s_wtime = s_lastcheck` = now, `s_mtime` 0, first_ino 11, inode size 128, creator OS 0, no lost+found. Bitmap padding bits set. Inode 1 and 3–10 all zero. Root 040755 0:0. Mounts, `e2fsck -fn` clean. With `-b 100000`: 13 groups, 16 inodes per group minimum (`-B 4096`: 32). |
| O-3 | `-d big1`; `-d big1 -m 0/1/10/50/100` | Size = N0 + floor(N0·m/100) with N0 = 27974 (the size it computes with `-m 0`); `s_r_blocks_count` = floor(N0·m/100) (1398 for m=5). Its N0 is an estimate: `-d one`, `-d s1` (only `d/`), `-d empty`, no input at all, and 1–50 files in the root all fail with "couldn't allocate a block (no free space)" or "too few blocks". |
| O-4 | `-b 1000 -m 50` (empty), `-d big1`, `-B 2048/4096`, `-d lf1 -m 10` (input `lost+found/x`), `-d lf2` (input file named lost+found) | lost+found exists iff `s_r_blocks_count > 0`; mode 040700, 0:0, times = fs time, 16 blocks for every block size (16/32/64 KiB, plus 1 indirect block), first block `.`/`..`, 15 empty blocks; it is created before the layers (inode 11 in small images): an input lost+found directory is merged into it (its own metadata lost, "ignoring duplicate entry lost+found"); an input non-directory lost+found is dropped. With reserved = 0 an input lost+found is an ordinary directory. |
| O-5 | `-m 101`, `-m -1`, `-m abc`, `-d big1 -m 200` | `-m` is not validated (101 accepted; −1 and abc → 0). With `-m 200` the reserved count exceeds half the blocks and e2fsck refuses the superblock ("superblock could not be read or does not describe a valid ext2 filesystem"). `-m 100` (exactly half) is accepted. |
| O-6 | `-d big1 -b 27000` / `28000` / `29371` / `29373`; `-b 1` | A `-b` below its auto size is raised: "number of blocks too low, increasing to 29372", exit 0. `-b 1`: "increasing to 6", then "too few blocks", exit 1. |
| O-7 | `-d big1 -N 100/630/631/1000`; `-b 100 -N 0/1/10/11` | `s_inodes_count` = max(N, needed) rounded up to a multiple of (groups × 8 or 16); below need: "number of inodes too low, increasing to 631". Its "needed" count is 10 + entries + 1. |
| O-8 | `-b 40000 -i 4096/1024/100000/128/127/0`; `-d big1 -i 4096` (no -b); `-d big1 -b 1000 -i 1024` | With `-b`: inodes = max(needed, -N, B·bs/i) rounded (10000 for 4096). Without `-b`, `-i` has no effect. `-i` uses the requested `-b` even when `-b` is then raised. `-i 128` produces an image whose free-block count underflows (4294967094) — corrupt. `-i 0` crashes (signal). |
| O-9 | `-r 100`, `-r 1000`, `-r 100 -b 200`, `-b 40000 -r 100`, `-d big1 -r 0/1/10/1000/60000`, `-r 100 -N 50` | Contrary to its help text, `-r N` sets the block count to N when `-b` is absent and the inode count to N when `-N` is absent ("too few inodes" for small N, "couldn't allocate a block" when N is below the content). |
| O-10 | `-B 512/3000/8192/65536/2048` | Only 1024, 2048, 4096 accepted; else "Valid block sizes: 1024, 2048 or 4096.", exit 1. Default 1024. |
| O-11 | `-L hello`, `-L 0123456789abcdefXYZ` | Label stored NUL-padded; truncated to 16 bytes silently. |
| O-12 | `-o hurd/freebsd/lites/7/masix/foo/Linux/HURD/1x/4294967296` | Names linux=0, hurd=1, freebsd=3, lites=4, case-insensitive; numbers parsed leniently (`1x` → 1; `4294967296` → 0); unknown names (`masix`, `foo`) → 0. e2fsck accepts any value. |
| O-13 | `-b 300 -e 65` / `0x41` / `300` / `-1` / `abc` | Free blocks filled with the low byte of `atoi(arg)` (65 → 'A', 300 → 0x2C, −1 → 0xFF, `0x41` and `abc` → 0). Boot block, inode-table slack and directory slack stay zero. |
| O-14 | `-z -d sp` (sparse files, an all-zero non-sparse file), `-a big.tar -z -B 4096` (5 GiB + 3 GiB sparse) | Without `-z` holes are written as zero blocks. With `-z` every all-zero block becomes a hole, including explicit zero data (100 KiB of zeros → no data block), but an indirect block is still allocated for every 256/512/1024-block range even when it maps only holes. Auto size ignores holes (2.2 M blocks of 4 KiB for 8 GiB of holes); the output file itself is sparse (9 GB apparent, 9.2 MB on disk). `large_file` set. |
| O-15 | `-f -b 200 -d tt` | Every timestamp 0: superblock and all inodes, including file mtimes. |
| O-16 | `SOURCE_DATE_EPOCH=1000000000 / abc / "" / -5 / 99999999999 / 12abc` | SDE sets `s_wtime`, `s_lastcheck` and the root/lost+found times; parsed with `atoi` semantics and truncated to 32 bits (abc and empty → 0, 12abc → 12, −5 → 4294967291). It does not clamp file times; file ctimes stay host ctimes. |
| O-17 | `-b 200 -d tt` (files with old mtime/atime), tar inputs | `-d`: mtime = host mtime, atime = mtime (host atime ignored), **ctime = host ctime**. Tar: mtime from the member, atime = mtime, **ctime = 0**. Negative tar mtime −1000 stored as 0xFFFFFC18; 3000000000 stored as is (the kernel reads it as negative). |
| O-18 | tar with uid 100000 gid 70000, uid 3000000000 gid 4294967294; `-U 100000` | Ids truncated to 16 bits (34464:4464; 24064:65534); high words not written. |
| O-19 | tar with devices 300:70000 and 4095:1048575 | Stored as 44:112 and 255:255 in the old 8:8 encoding — device numbers lost. |
| O-20 | tar with a 300-byte name, a 1500-byte symlink (1 KiB blocks), an empty symlink target, a hard link to a directory; `-D` line of type `l`; symlinks of 59/60/61 bytes | All written, producing e2fsck errors: "has filetype set" for the long name (name_len written as 16 bits), "symlink too big"/"invalid" for 1500 and empty targets, "is a link to directory". 59 → fast symlink; 60, 61 → slow (1 block). |
| O-21 | tar symlink with mode 0755; `-d` symlinks | Symlinks always get 0777 from `-d`/`-a`. |
| O-22 | `-U 5`, `-U5`, `--squash-uids=5`, `-U` (no arg), `-U -P`, `-P`, `-q 7`, `-q7`, `--squash=7`, `-U 4294967295`, `-U -1`, `-U 9 -D table`, tar root entry with `-P` | `-U N`: uid = gid = N for `-d`/`-a` nodes (also for a tar `./` entry), not for the default root, lost+found or device-table nodes. `-U` consumes the next word (`-U -P` → uid 0, no `-P`). `-U 4294967295` and `-U -1` mean "no squash". `-P`: `mode & ~077` for every non-symlink from `-d`/`-a` (setuid/sticky kept, 04750 → 04700, 01777 → 01700), not for device-table nodes. Short `-q` crashes (SIGSEGV); `--squash=N` works as `-U N -P`. |
| O-23 | `-d one:/a/b`, `-d s2 -d one:/d`, `-a r4.tar:/d`, `-D t:/d`, `-d co:lon` (dir named `co:lon` and dir `co`) | The `:path` suffix is accepted but **ignored**: content always goes to the root. The argument is split at the first `:` (`co:lon` copies `co`). |
| O-24 | `-d LA -d LB` with overlapping names; tars with duplicate members (`dup` twice, `f2d` file then dir, `d2f` dir then file, `d` then `d/`) | Across `-d` layers and within one: the first entry wins, later ones get "ignoring duplicate entry <name>"; directories merge and keep the first one's metadata; children of an ignored non-directory are not added. For tar members the existing entry also wins (silently), **but the ignored member's mode/uid/gid are applied to the parent directory** (root became 0644, or uid 4, etc.). |
| O-25 | tar members `a/b/c.txt` (no `a/b`), `/abs/x`, `../up/y`, `x/../z`; hard link to a later member; hard link to a missing name; `-d` with symlink `lib -> usr/lib` then tar `lib/x` | Missing parent: member skipped with "can't find directory 'a/b' to create 'c.txt'", exit 0. Leading `/`, `..` are not normalised (skipped unless the literal path exists). A hard link whose target does not exist yet: fatal "path target not found in filesystem", exit 1 and a corrupt image left behind. Symlinks are not followed: "can't add 'x' to a non-directory", exit 1. Absolute link names (`/target`) are resolved from the root. |
| O-26 | `-a gnu/oldgnu/posix/ustar/v7.tar`, `-a posix.tar.{gzip,bzip2,xz,zstd}` (no telling extension), pax sparse 0.0/0.1/1.0, GNU sparse; tar with uname set | All read; compression detected by content; GNU long names and sparse members restored with correct data (md5 equal to GNU tar extraction); uname/gname ignored (numeric ids used). |
| O-27 | device tables (see R-DT-*), e.g. `/dev/hda b 640 0 6 3 1 1 1 4`, `/dev/x c 600 0 0 10 5 2 3 8`, `/dev/w c 600 0 0 10 0 0 0 3`, `/x c`, `/x c 600`, `/x c 6a0 5 5 1 1`, `/x c 170600 0 0 1 1`, `/x cc 600`, `/x q …`, `/x c 600 -1 0 …`, `/x c 600 0 0 a b`, `/x c 600 0 0 1 1 - 1 2`, `/x c 600 0 0 1 1 -1 1 2`, fix-ups over `-d` content | Series: names `name<i>` for start ≤ i < count (hda1..hda3 for start 1, count 4); minor = minor + i·inc − start (x2..x7 → 9,12,…,24); inc 0 gives equal minors; count 0 → single node; start ≥ count → nothing. Missing trailing fields are 0; `-` or a non-number ends parsing; mode is octal, masked to 07777; `6a0` → mode 06, later fields 0; `/x c` and `/x cc …` and non-octal mode → "bad format", line skipped; unknown type → "bad type", skipped; `l` creates an empty (invalid) symlink; `s` creates a socket; `f` an empty file; uid −1 → 65535. Missing parent → "can't find directory", skipped; parents are not created. Existing path: permission bits, uid, gid replaced; type, mtime, data kept (a symlink got 0600). New nodes get mtime = the table file's mtime (0 with `-f`). Not affected by `-U`/`-P`. Warnings printed twice. |
| O-28 | `-d` tree with an unreadable directory / unreadable file / socket / dangling symlink; `-d nonexist`, `-d one/f` (a file), `-a nonexist.tar`, `-D nonexist.txt` | Unreadable directory: "ud: Permission denied", exit 1. Unreadable file: crash (SIGSEGV). Socket copied as socket; dangling symlink copied. Missing or wrong-type sources: "<src> should be a directory" / "<src> should be a file", exit 1. |
| O-29 | `-d HA -d HB` where `HB/x` is a hard link of `HA/y`; `HA/z` with an outside link | Hard links detected across layers (same inode). Link counts = names in the image (z: 1, y: 3). |
| O-30 | image `-`; `-a -` from a pipe; `-D -` | Image `-`: "copy_file: ftruncate: Invalid argument", exit 1. Stdin inputs: "Cannot count the required inodes for input from stdin -- use the -N or -i options …"; works only with enough `-b`/`-N`. |
| O-31 | `-x base.img -d s2` with `-b`, `-N`, `-B`, `-L`, `-m` | Geometry, label and reserved count come from the starting image; those options are ignored; existing entries win ("ignoring duplicate entry"); running out of inodes is fatal. |
| O-32 | `-g /a.txt`, `-g /d3/f5`, `-g /`, `-g /link`, `-g sub`, `-g /sub/../a.txt`, `-g /nonexist`, `-z … -g /hole_end` | Writes `<cwd>/_a.txt.blk` containing `6: 8\n` (size, colon, data block numbers); `/` → `_.blk`; `sub` → `sub.blk`; `..` resolved; fast symlink `5:\n`; empty file `0:\n`; indirect blocks not listed; holes listed as 0 but some hole ranges omitted (1024-block file listed 781 entries). Missing path: "path /nonexist not found in filesystem", exit 1. |
| O-33 | `time xgenext2fs -d hd`; `-a gnu.tar` with a 5 GiB sparse file (1 KiB blocks, no `-z`); `-a rootfs.tar` (Debian trixie container export, 143 MB, 4901 entries) | 70 s for 20 000 files in one directory (quadratic); > 12 min CPU for the 5 GiB file (killed); 0.7 s for the rootfs tar. |
| O-34 | `-d hd` (20 012 inodes) | 3 groups of 1160 blocks (`bpg` reduced so that `ipg` = 6672 ≤ 8192); mounts. |
| O-35 | `-a big.tar -z -B 4096` | `large_file` set (only then); triple-indirect blocks used for the 5 GiB file; mounts, data correct. |
| O-36 | any fatal error after the image was opened | A partial or corrupt image file is left at the output path. |
| O-37 | all runs | Progress lines ("copying from directory X", "copying from tar archive X", "nodes fixup and creation from device table X") and warnings on stderr. `-v` adds a dump of the geometry and every inode. |
| O-38 | `-d big1` | Directory inodes spread over groups (e.g. d0 = 321, d1 = 490, d11 = 24): an Orlov-like policy. Directory entries in input order. |
| O-39 | tar `./` entry with mode 0700, uid 5, gid 6, mtime 1985; `-d` of a directory with mode 0711 | Tar root entry sets the root's mode, uid and gid but not its times (root times = fs time). `-d` never transfers the source directory's own metadata (root stays 040755 0:0). |

## 13. Deliberate divergences from xgenext2fs

Content-affecting divergences (the verifier must exclude these situations
from content-equivalence corpora or expect the stated difference):

| Id | Situation | xgenext2fs | kext2 | Why |
|---|---|---|---|---|
| D-1 | uid/gid ≥ 65536 (inputs, `-U`) | low 16 bits (O-18) | full 32 bits | kernel reads `i_uid_high`/`i_gid_high` (`ext2_iget`) |
| D-2 | device major or minor ≥ 256 | low 8 bits, old encoding (O-19) | new encoding in `i_block[1]` | kernel `ext2_write_inode`/`ext2_iget` |
| D-3 | name > 255 bytes, symlink target empty or ≥ bs, hard link to a directory | written, image corrupt (O-20) | fatal error | kernel/e2fsck reject them |
| D-4 | `-d/-a/-D src:path` | `path` ignored, content at root (O-23) | content placed under `path` (created if missing) | documented meaning of the option |
| D-5 | tar member duplicating an existing path | ignored, but its mode/uid/gid applied to the parent (O-24) | ignored (warning); parent untouched | the parent rewrite is a bug |
| D-6 | tar member whose parent directory is missing | member skipped (O-25) | parent created as implicit directory (0755, 0:0, `T_fs`) | tar archives without directory members are common; dropping files silently is worse |
| D-7 | tar names with leading `/`, `.` components, `..` | not normalised (O-25) | `/` and `.` removed; `..` members skipped with a warning | safe, conventional normalisation |
| D-8 | ctime | host ctime (`-d`), 0 (tar) (O-17) | ctime = atime = mtime | reproducibility (host ctime is a wall-clock value) |
| D-9 | timestamps outside int32; bad `SOURCE_DATE_EPOCH` | low 32 bits; `atoi` (O-16, O-17) | clamped to −2³¹…2³¹−1; malformed SDE is an error | kernel reads inode times as signed 32-bit |
| D-12 | auto size: tiny trees, `-z` holes | estimate may be too small → fatal "couldn't allocate" (O-3); holes counted (O-14) | exact (§7.4); holes not counted | correctness; "smallest image" |
| D-24 | device-table type `l` | creates an empty, invalid symlink (O-27) | line skipped with a warning | validity |
| D-25 | device-table values out of range (uid −1 → 65535 etc.) | truncated | line skipped with a warning | validity |

Divergences that do not affect content (geometry, metadata, CLI behaviour):

| Id | Situation | xgenext2fs | kext2 |
|---|---|---|---|
| D-10 | `-r N` | sets block count and inode count to N (O-9) | adds N free blocks to the automatic size; ignored with `-b` |
| D-11 | `-b` below the needed size | raised silently (O-6) | fatal error naming the minimum (Q-2) |
| D-14 | `-m` > 100 or garbage | accepted, image may be invalid (O-5) | usage error; reserved count clamped to B/2 |
| D-15 | `-e` non-decimal or > 255 | `atoi`, low byte (O-13) | usage error |
| D-16 | `-o` unknown name | 0 (O-12) | usage error |
| D-17 | `-q uid` (short form) | crash (O-22) | works like `--squash` |
| D-18 | `-U 4294967295`, `-U -1` | "no squash" (O-22) | usage error |
| D-19 | non-numeric option values | `atoi` (0) | usage error |
| D-20 | unreadable input file | crash (O-28) | fatal error, exit 1 |
| D-21 | failure after output opened | corrupt image left (O-36) | no output (temporary file removed) |
| D-22 | `-a -`, `-D -`, image `-` | need `-b`/`-N`; image `-` fails (O-30) | fully supported |
| D-23 | on-disk details | no features, `s_errors` 0, `s_max_mnt_count` 20, every group has a superblock backup, empty indirect blocks for holes, Orlov-like inode spreading, entries in input order (O-2, O-14, O-38) | `filetype` (not for Hurd) + `sparse_super` (+ `large_file`), `s_errors` 1, `s_max_mnt_count` 0xFFFF, layout of §9, sorted entries |
| D-26 | `-g` holes | some hole entries omitted (O-32) | one entry per logical block |
| D-27 | huge directories, huge files with 1 KiB blocks | quadratic / very slow (O-33) | linear (R-PERF-1) |

## 14. Acceptance tests (for the verifier)

All tests run the binaries from outside and inspect images with e2fsprogs
1.47.x and Linux loop mounts (the `kext2-tools` container; mounts need
`--privileged`). "Clean" means: `e2fsck -fn image` exits 0 and its output
contains none of `Fix?`, `Clear?`, `Relocate?`, `Truncate?`, `IGNORED`,
`WARNING`, `invalid`, `differences`. Use `T_fs` values in the past (e2fsck
complains about future superblock times).

### 14.1 Validity

AT-VAL-1. For each block size 1024, 2048, 4096 and each corpus below, with
and without `-z`, default options and `-m 0`, the image is clean, mounts with
`mount -t ext2 -o loop,ro` without kernel errors in `dmesg`, and a full walk of
the mounted tree (read every file, `readlink` every symlink, `stat`
everything) succeeds and yields the expected content (§14.2 manifest equals
the manifest of the input).

Corpora (the verifier builds them; sizes indicative):
1. empty input (no layer at all) and an empty directory;
2. `t1`-like tiny tree (file, hard link, symlink, fifo, empty dir);
3. multi-group tree (≥ 4 groups at 1 KiB: ≥ 30 MB of files in ≥ 20 top-level
   directories);
4. a directory with 100 000 entries, and a directory with 31 998
   subdirectories;
5. sparse files exercising every level: data at logical blocks 0, 11, 12,
   12+p−1, 12+p, 12+p+p²−1, 12+p+p², and the last block allowed by R-FMT-15
   minus a margin (with `-z` these are tiny; without `-z` only for 1 KiB
   blocks where TIND starts at 64.3 MiB);
6. files of 2³¹−1, 2³¹, 2³² and 2³²+1 bytes (sparse, with `-z`) and one dense
   file of 2.1 GiB (no `-z`): `large_file` present iff a file > 2³¹−1;
7. symlinks of length 1, 59, 60, bs−1; devices 0:0, 1:3, 255:255, 256:0,
   0:256, 4095:1048575; fifos; sockets; uid/gid 0, 65535, 65536, 4294967294;
   permission bits 07777 and 0;
8. inode-heavy tree (≥ 40 000 empty files, 1 KiB blocks: forces
   `Gmin > 1` and smaller groups);
9. tar archives: v7, ustar, GNU (long names, `S` sparse), pax (long paths,
   sparse 0.0/0.1/1.0, big uid), each also gzip/bzip2/xz/zstd-compressed, and
   a member list without directory members (implicit parents);
10. device table with series, fix-ups, every type;
11. layering: `-d A -d B -a C.tar -D T` with overlaps, `:path` targets.

AT-VAL-2. Geometry edge cases are clean: `-b` values `fdb + k·8·bs + r` for
r ∈ {1, 2, 3, small values below the last group's overhead} and k ∈ {1, 2, 3,
26, 27} (exercises the `bpg − 8` adjustment and sparse-super groups 25, 27);
`-b` with `-N` up to the maximum that fits; `-b` with `-i` 1024 … 1 MiB;
`-m 100`; `-o hurd` (no `filetype`); `-e 255`; `-L` with 16 bytes.

AT-VAL-3. `dumpe2fs -h` shows: revision 1, features exactly as R-FMT-2,
"Errors behavior: Continue", "Maximum mount count: -1", "Filesystem state:
clean", inode size 128, first inode 11, UUID `<none>`, block/inode counts
consistent with §7.

AT-VAL-4. Errors: each limit of §8.10 triggered by a crafted input makes
kext2 exit 1 without leaving an output file; `-b` too small exits 1 with the
minimum in the message; `-b M` with the reported `M` succeeds.

AT-VAL-5. Size compatibility (SHOULD): for every corpus where `xgenext2fs -b
N` succeeds without raising `N` (O-6), `kext2 -b N` with the same options also
succeeds (kext2's metadata overhead is never larger; sorted directory packing
can differ by at most one block per directory). Without `-b`, kext2's block
count SHOULD NOT exceed xgenext2fs's by more than 1 % on corpora where
xgenext2fs's automatic size works.

AT-VAL-6. Backup superblocks: `e2fsck -fn -b <start(1)> -B <bs> image`
SHOULD report no problems either.

### 14.2 Content equivalence with xgenext2fs

AT-EQ-0. **Manifest.** Mount the image read-only and walk it (or use
debugfs). For every path (root = `/`), record:
* type; permission bits (`st_mode & 07777`); uid; gid;
* for regular files: size and SHA-256 of the data;
* for symlinks: target bytes;
* for devices: major, minor;
* mtime (seconds);
* a hard-link class: paths of non-directories are grouped by inode number
  and the group is represented by its smallest path; record the class and
  `st_nlink` (which MUST equal the class size);
* for directories: the sorted list of child names and `st_nlink`.
Not compared: inode numbers, block numbers, `st_blocks`, directory sizes,
directory entry order, atime, ctime, and all superblock/geometry fields
(counts, UUID, features, reserved count).

AT-EQ-1. For every corpus of §14.1 that avoids the content-affecting
divergences of §13 (D-1…D-9, D-12 failures, D-24, D-25: ids < 65536, device
numbers < 256, valid names and symlinks, tar archives with all parent
directories present, no duplicate tar members, no `:path`, trees on which
xgenext2fs's auto size succeeds or with an explicit `-b` large enough for
both), kext2 and xgenext2fs, run with the same command line, produce equal
manifests. This includes options `-U`, `-P`, `--squash`, `-f`, `-z`, `-m`,
`-N`, `-i`, `-B`, `-D` tables, `-d` layering with duplicates (first wins),
`SOURCE_DATE_EPOCH` (root mtime) and lost+found (present iff the reserved
count is non-zero; borderline tiny trees, where the two programs' `N0`
differ, are excluded — Q-8).

AT-EQ-2. For the divergent cases the expected kext2 manifest is the input's
true content (e.g. the full uid, the true device numbers, the file placed
under `:path`), and xgenext2fs's manifest differs exactly as §13 states.

### 14.3 Reproducibility matrix

AT-REP-1. With `SOURCE_DATE_EPOCH` (or `--timestamp`, or `-f`) fixed, the
SHA-256 of the image MUST be identical across all variations of each row:

| Variation | How |
|---|---|
| input location | the same tree copied to two different absolute paths (and different `-d` names) |
| creation order | the tree created twice, files written in forward and in reverse/random order (different readdir order, different host inode numbers) |
| host metadata noise | different atimes/ctimes (touch -a, chmod twice), different host inode/device numbers (copy to another filesystem/tmpfs) |
| tar packaging | same members, different member order (no duplicates, hard links after targets); uncompressed vs gzip vs bzip2 vs xz vs zstd; file vs `-a -` from a pipe; GNU vs pax format when both express the same metadata |
| `-d` vs `-a` | `-d tree` vs `-a tree.tar` (tar created with `--numeric-owner`, no `./` member) |
| environment | `LC_ALL=C` vs `LC_ALL=en_US.UTF-8`; `TZ=UTC` vs `TZ=Asia/Kathmandu`; umask 022 vs 077; different `TMPDIR` |
| time | two runs at different wall-clock times (≥ 2 s apart) |
| parallelism | different CPU counts (`taskset`/`docker --cpus`) and any implementation thread setting |
| repetition | 20 consecutive runs |
| output | regular file vs `-` to a pipe (`cat > file`) |

AT-REP-2. Without any timestamp option or SDE, two runs differ only in the
superblock time fields (0x30, 0x40 of every superblock copy) and in the times
of the root, lost+found and implicit directories.

AT-REP-3. `--timestamp S`: every inode time and `s_wtime`/`s_lastcheck`
equals S. `--clamp-timestamps S`: no time exceeds S, earlier times unchanged,
`s_wtime = S`. `-f`: all zero.

### 14.4 Command-line behaviour

AT-CLI-1. Every option of R-CLI-3 is accepted in short, long, `--long=value`
and abbreviated-long forms; clustering and permutation work (R-CLI-1).
AT-CLI-2. Usage errors (unknown option, missing operand, two operands, bad
numbers per R-CLI-4, conflicting time options) exit 1 and create no output.
AT-CLI-3. `-h` exits 0 with usage on stderr; `-V` prints `kext2 <version>`
on stdout and exits 0.
AT-CLI-4. Device-table parsing reproduces the O-27 cases exactly (node names,
types, modes, ids, device numbers, which lines are skipped).

### 14.5 Performance (AT-PERF)

Reference corpus: a ~1 GiB root-filesystem-like tree (e.g. the union of a few
Debian/Ubuntu container exports with their package caches), ≥ 30 000 entries,
with a mix of small files and large binaries, provided as a directory and as
an uncompressed and a zstd-compressed tar. Measure wall time and peak RSS,
warm page cache, image on local disk, median of 5 runs, against xgenext2fs
1.5.6 and mkext2 0.1.1 run with the same options (`-B 4096`, auto size).

| Id | Target |
|---|---|
| AT-PERF-1 | kext2 wall time ≤ the faster of xgenext2fs and mkext2 on every input form, and ≤ 1.5 × the time of copying the input data once (`cat` of every file to /dev/null + writing an image-sized file) |
| AT-PERF-2 | one directory with 100 000 entries: ≤ 2 s (1 KiB blocks) |
| AT-PERF-3 | a 5 GiB sparse file with 1 KiB blocks: with `-z` ≤ 2 s; without `-z` limited by writing 5 GiB (≤ 1.5 × `dd` of 5 GiB to the same disk) |
| AT-PERF-4 | doubling the number of files (same size distribution) at most doubles the time (+20 %) |
| AT-PERF-5 | peak RSS ≤ 64 MiB + 1 KiB × number of entries, independent of file sizes |

## 15. Open questions

| Id | Question | Current decision |
|---|---|---|
| Q-1 | Duplicate paths across/within layers: keep "first wins" (xgenext2fs for `-d`, and for tar apart from its parent bug) or switch to tar/overlay "last wins", which layered root filesystems usually expect? | First wins (R-IN-3), for drop-in compatibility. |
| Q-2 | Too-small `-b`: error (requested) or xgenext2fs's silent raise to the automatic size (O-6)? | Error naming the minimum (R-SIZE-5). |
| Q-3 | `:path`: honour it (documented meaning) or ignore it like xgenext2fs 1.5.6 (O-23) for bug compatibility? | Honour (D-4). |
| Q-4 | Missing tar parents: create (D-6) or skip like xgenext2fs? Should `-U`/`-P` apply to such implicit directories? | Create; not squashed (R-IN-4). |
| Q-5 | UUID: all zero (chosen) or a deterministic content-derived UUID (e.g. from a hash of the manifest), optionally with a `--uuid` option? | All zero (R-FMT-4). |
| Q-6 | Follow symlinks in parent components when layering (e.g. `/lib -> usr/lib` in a base layer, then a tar adding `lib/x`)? xgenext2fs fails. | Not followed; fatal (R-IN-3). |
| Q-7 | Apply `-i` in automatic-size mode (a fixed point between size and inode tables) instead of ignoring it like xgenext2fs? | Ignored (R-SIZE-2). |
| Q-8 | lost+found depends on `R = floor(N0·m/100) > 0`; for tiny trees (N0 below about 100/m blocks) kext2's `N0` can differ from xgenext2fs's estimate, so lost+found may appear in one image and not the other; and when `R = 0` while the input itself supplies `lost+found` or a device table touches it, kext2 keeps the provisional directory with generated metadata (R-RES-3) where xgenext2fs would show the input's. Accept, or define `N0` differently? | Accept; excluded from AT-EQ. |
| Q-9 | Device-table series: reproduce xgenext2fs's exclusive `count` and `minor + i·inc − start` (R-DT-5), or the widely documented "count nodes starting at start, minor + (i−start)·inc"? | Reproduce xgenext2fs exactly. |
| Q-10 | ctime: `= mtime` (chosen) or `= T_fs` (closer to "inode changed when the image was built")? | `= mtime` (R-TIME-3). |
| Q-11 | `-r` together with `-b`: ignore with a warning (chosen) or make it an error? | Ignore. |
| Q-12 | Hurd (`-o hurd`): e2fsck 1.47 accepts `filetype` with creator OS Hurd (tested), but compatibility of the Hurd's own ext2 translator was not tested. | No `filetype` for Hurd. |
| Q-13 | Extended attributes (pax `SCHILY.xattr.*`, host xattrs, file capabilities) are silently dropped, as by xgenext2fs. Warn? | Silently ignored. |
| Q-14 | Should hard-link detection in `-d` layers be limited to one layer? xgenext2fs links across layers (O-29). | Across all `-d` layers. |
| Q-15 | e2fsck reports superblock times in the future; images built with a future `--timestamp`/SDE are valid for the kernel but not "clean" on a machine whose clock is earlier. | Documented only (§14). |

## Appendix A. Provenance index (kernel, torvalds/linux @ 70dd4ee7)

| Rule | Kernel location |
|---|---|
| superblock acceptance: magic, revision, inode size, frag = block size, `bpg ≤ 8·bs`, `bpg > itb+3`, `ipb ≤ ipg ≤ 8·bs`, groups and `s_inodes_count` consistency, device size | `fs/ext2/super.c:ext2_fill_super` |
| feature masks | `fs/ext2/ext2.h` (`EXT2_FEATURE_*_SUPP`) |
| errors behaviour, resuid/resgid defaults | `super.c:ext2_set_options` |
| mount-count warning, max-mount default | `super.c:ext2_setup_super` |
| group descriptors inside their group | `super.c:ext2_check_descriptors`; bitmap validation `balloc.c:ext2_valid_block_bitmap` |
| maximum file size | `super.c:ext2_max_size` |
| sparse-super groups | `balloc.c:ext2_group_sparse`, `ext2_bg_has_super`, `ext2_bg_num_gdb` |
| GDT location | `super.c:descriptor_loc` |
| inode location | `inode.c:ext2_get_inode` |
| inode decoding (32-bit ids, signed times, size high word for regular files only, device decoding, fast symlink test) | `inode.c:ext2_iget`, `ext2_inode_is_fast_symlink` |
| inode encoding (device encodings, large_file) | `inode.c:ext2_write_inode`; `include/linux/kdev_t.h` |
| block map | `inode.c:ext2_block_to_path`, `ext2_get_branch`; goal policy `ext2_find_near`, `ext2_find_goal`, `ext2_alloc_branch` |
| inode allocation policies | `ialloc.c:find_group_orlov`, `find_group_other`, `ext2_new_inode` |
| directory entry rules | `dir.c:ext2_check_folio`, `ext2_make_empty`, `ext2_readdir`; `ext2.h` (`EXT2_DIR_REC_LEN`) |
| name length, symlink creation | `namei.c:ext2_lookup`, `ext2_symlink`; `include/linux/ext2_fs.h` (`EXT2_NAME_LEN`, `EXT2_LINK_MAX`) |

## Appendix B. Validation of this specification

During the writing of this specification a throwaway Python prototype
(outside the repository, not part of kext2) implemented §7–§9 and §8 from the
draft text. Its images were `e2fsck -fn` clean and mounted on Linux for:
`t1`, empty, `big1` at 1/2/4 KiB, 20 000 files in one directory (3 groups via
`Gmin`), `-b 100000` (13 groups, spreading), `-b 40000` with forced
`bpg` = 1000, 2 KiB blocks with 184 groups, sparse files with triple-indirect
blocks (`-z`, 1 KiB), dense 13 MiB files, symlinks of 59/60/61/1023 bytes,
devices 300:70000 and 4095:1048575 (read back correctly), uid 100000,
files of 2³¹−1, 2³¹ and 5 GiB (`large_file`), creator OS Hurd. It also
showed two pitfalls now written into the rules: `bpg` not a multiple of 8
breaks e2fsck's bitmap reading, and `ipg` < 16 puts reserved inodes outside
group 0.

## Appendix C. Resolutions of the open questions

2026-10-16, maintainer: every "Current decision" in §15 is accepted as written
(Q-1 … Q-15). The implementer follows them; changes go through this appendix.

### C.1 Amendment: `-r` (2026-10-16, maintainer)

Black-box check of xgenext2fs 1.5.6 (`-f -d src`, a tree whose automatic
size is 10 blocks): `-r 100` gives a 100-block image, `-r +100` a 110-block
image. So xgenext2fs accepts an optional leading `+`, and D-10 / R-SIZE-4 /
R-CLI-4 are amended:

- `-r +N` (N ≥ 0): without `-b`, the automatic size computed with N extra free
  blocks, as R-SIZE-4 describes for `X = N`.
- `-r N` (no sign): the image has exactly N blocks, as if `-b N` had been
  given (so a too-small N is the R-SIZE-5 error). Unlike xgenext2fs (O-9) it
  does not change the inode count.
- With `-b`, `-r` in either form is ignored with a warning (Q-11).
- No other option gains a sign; `-r -N` is a usage error.

### C.2 Amendment: reduced option set (2026-10-17, maintainer)

kext2 no longer accepts the xgenext2fs command line. It keeps what
reproducible root filesystems need and drops low-level controls. This
amendment supersedes every rule below that names a removed option; the
content rules (§4–§6, §8, §9) are unchanged.

**Command line (§2).** The options are `-d`, `-a`, `-D`, `-L`, `-f`,
`--timestamp`, `--clamp-timestamps`, `-U`, `-P`, `-q`, `-v`, `-h`, `-V` and
the new `--free-space` (long only). `-B`, `-b`, `-r`, `-N`, `-i`, `-m`, `-o`,
`-e`, `-z`, `-x` and `-g` are unrecognised options (usage error, R-CLI).
Parsing still follows GNU `getopt_long` (R-CLI-1 … R-CLI-3).

R-CLI-C2-1. `--free-space BYTES`: `BYTES` is a decimal number, optionally
followed by `K`, `M`, `G` or `T` (case-insensitive, optionally followed by
`iB`), meaning 2^10, 2^20, 2^30 or 2^40. The value is at most 2^44 (16 TiB);
anything else is a usage error. Default 0. The last occurrence wins.

**Block size.** `bs` is always 4096, so `fdb = 0` (block 0 holds the
superblock at byte 1024), `ipb = 32`, `p = 1024`, and the R-FMT-15 limit is
2,196,873,666,560 bytes. GEOMETRY (R-GEO-2) and SIZE (R-SIZE-3) are
unchanged with these constants.

R-RES-C2-1 (replaces R-RES-1 … R-RES-4). Before the first layer, the root
receives the generated `lost+found` exactly as R-RES-1 describes for `m > 0`
(it is always created; layers merge into it or are ignored as R-IN-3 says).
Its directory data has `max(4, packed)` blocks (16 KiB, as mke2fs makes it;
with `p = 1024` no indirect block). No blocks are reserved:
`s_r_blocks_count = 0`.

R-SIZE-C2-1 (replaces R-SIZE-2, R-SIZE-4, R-SIZE-5). Let `S` be the
`--free-space` value, `F = ceil(S / 4096)` free blocks and `J = ceil(S /
16384)` free inodes (mke2fs's default ratio). With `U` and `D` of R-SIZE-1,
`I = 10 + U + J` and `B = SIZE(D, F, I)`. The image has `B` blocks and the
geometry of `GEOMETRY(B, I)`; it therefore has at least `F` free blocks and
at least `J` free inodes (both may be higher by the geometry's rounding).

**Superblock (§8.2).** `s_creator_os` is 0 (Linux), `filetype` is always set,
and `s_uuid` is zero. The volume label is still `-L`.

**Data (§8.5, §8.6).** Every regular file is dense: its blocks are
`[0, ceil(size/bs))`, and zero ranges of tar sparse members are stored as
zero blocks. Free blocks are zero. In a regular output file, zero regions may
still be left as holes (§10), which changes no byte of the image.

**Removed features.** §11.1 (starting image) and §11.2 (block maps) no longer
apply. The acceptance tests that use removed options (AT-*) are not run for
kext2; the comparison harness runs the others without `-B`, comparing file
content only.

# Clean-room record

kext2 is an experimental ext2 image builder written in Rust. It accepts the
same command line as xgenext2fs 1.5.6 and has the same reproducibility goals
as mkext2, but it is designed from the Linux kernel's ext2 driver and the
public ext2 documentation, not from genext2fs or mkext2.

## Sources allowed

- Linux kernel: `fs/ext2/`, `include/uapi/linux/`,
  `Documentation/filesystems/ext2.rst` and `Documentation/filesystems/ext4/`
  (the shared on-disk format), from torvalds/linux at commit
  `70dd4ee72eb2d5793c62e71c14e3aba78e5a4e56` (2026-10-16).
- "The Second Extended File System — Internal Layout" by Dave Poirier
  (https://www.nongnu.org/ext2-doc/ext2.html).
- The `xgenext2fs` 1.5.6 and `mkext2` 0.1.1 programs, as black boxes only
  (inputs in, image out, help text, error behavior).
- e2fsprogs programs (`e2fsck`, `debugfs`, `dumpe2fs`) and the Linux kernel
  (loop mounts) in a container, to inspect and check images.

## Sources forbidden

- The source code, specifications, tests or data of genext2fs, xgenext2fs,
  their forks and packages, and of mkext2 (`~/Documents/workspace/personal/mkext2`).
- The source code of mke2fs / e2fsprogs.

## Roles

- **Specification author.** Reads the kernel sources and the ext2
  documentation, runs xgenext2fs as a black box to learn the command line
  behavior, and writes [`SPEC.md`](SPEC.md). The specification is written in
  its own words: it describes the on-disk format, the requirements and the
  layout policy, and contains no kernel code.
- **Implementer.** Writes the Rust implementation from `SPEC.md` and the public
  documentation (Poirier, kernel `Documentation/`), never from kernel C sources,
  and may run xgenext2fs/mkext2 as black boxes.
- **Verifier.** Writes the black-box comparison harness in `compare/`, which
  only runs the three programs and inspects their images.

## Log

- 2026-10-16: repository created; references fetched outside the repository.
- 2026-10-16, verifier: wrote `compare/` (generators, kernel-mount/e2fsck
  inspection in a container, case matrix, reproducibility checks, benchmark).
  Only the programs' `--help` output, mkext2's README (command line and goals)
  and their images were used. Baseline findings about the reference tools,
  from images only: xgenext2fs crashes on `-q`, fails to size an image built
  from a device table alone, and on macOS stores pax UTF-8 names decomposed
  (NFD); mkext2 omits `lost+found` when no directory or archive is given.
- 2026-10-16: specification author wrote `docs/SPEC.md`. Read: the Linux
  kernel at the commit above — `fs/ext2/ext2.h`, `super.c` (`ext2_fill_super`,
  `ext2_check_descriptors`, `ext2_max_size`, `ext2_setup_super`,
  `ext2_set_options`, `ext2_statfs`, `descriptor_loc`), `balloc.c`,
  `ialloc.c` (`find_group_orlov`, `find_group_other`, `ext2_new_inode`),
  `inode.c` (block mapping, `ext2_iget`, `ext2_write_inode`, fast-symlink
  test), `dir.c` (`ext2_check_folio`, `ext2_make_empty`, `ext2_readdir`),
  `namei.c`, `symlink.c`; `include/linux/ext2_fs.h` and
  `include/linux/kdev_t.h` (fetched from github.com/torvalds/linux at the same
  commit); `Documentation/filesystems/ext2.rst` and
  `Documentation/filesystems/ext4/special_inodes.rst`, `super.rst`; Poirier's
  document. Ran xgenext2fs 1.5.6 as a black box only (help text, ~300 runs
  on synthetic directory trees, tar archives made with GNU tar 1.35 and Python
  `tarfile`, device tables) and inspected its images with e2fsprogs 1.47.2
  (`dumpe2fs`, `debugfs`, `e2fsck -fn`) and Linux loop mounts in the
  `kext2-tools` container. A throwaway Python prototype written from the draft
  spec (kept outside the repository) was used to check the layout rules with
  e2fsck and the kernel. Forbidden sources were not read or searched: no
  genext2fs/xgenext2fs/fork/package sources, documentation or tests, no
  mke2fs/e2fsprogs sources, nothing under `~/Documents/workspace/personal/mkext2`
  or `~/Documents/workspace/cartesi`, no web searches for image-builder
  implementations. No kernel code was copied into the specification.
- 2026-10-16, implementer: wrote kext2 (`src/`, `tests/`, `README.md`,
  `LICENSE`) from `docs/SPEC.md` (all of it, Appendix C included) and
  `CLEANROOM.md`. No other ext2 documentation was needed; the Poirier
  document and the kernel `Documentation/` files were not opened. Ran as
  black boxes only: `xgenext2fs` 1.5.6 (help text, `-g` output format, a few
  images), `mkext2` (help text, a few timings on the benchmark data), GNU tar
  1.35 inside the `kext2-tools` container and on the host (to make sparse
  archives), and e2fsprogs (`e2fsck -fn`, `dumpe2fs`) and read-only Linux loop
  mounts in the container to check kext2's images. Ran `compare/run.py` and
  `compare/bench.py`. Added sparse tar cases to `compare/` (`gen.make_sparse_tars`
  and the `tar-sparse-*` cases in `run.py`), written from the tar format
  description in SPEC R-TAR-7; no existing harness expectation was changed.
  Crates used, all permissively licensed: flate2/zlib-rs, bzip2/libbz2-rs-sys,
  liblzma, zstd, tempfile. Forbidden sources were not read or searched: no
  kernel C sources, no genext2fs/xgenext2fs/fork/package sources, no
  mke2fs/e2fsprogs sources, nothing under `~/Documents/workspace/personal/mkext2`
  other than running `target/release/mkext2`, nothing under
  `~/Documents/workspace/cartesi`, not the specification author's prototype,
  and no web searches.
- 2026-10-16, implementer: implemented the `-r` amendment of SPEC Appendix C.1
  (`-r +N` extra free blocks, `-r N` exactly N blocks, `-r -N` a usage error,
  ignored with `-b`) from the specification text only, with tests; same
  sources and rules as the entry above.
- 2026-10-16, verifier: checked the two disagreements the implementer
  reported against xgenext2fs as a black box. `-r +N` and `-r N` differ (spec
  amended, Appendix C.1). `-q` takes a required argument (harness case changed
  to `--squash=0`). Rebuilt and reran everything independently: tests pass,
  `compare/run.py --seeds 20` shows 73/73 cases and 7/7 reproducibility checks,
  and the benchmark is in `docs/RESULTS.md`. Only binaries and images were
  inspected; no implementation source of mkext2 or genext2fs was read.

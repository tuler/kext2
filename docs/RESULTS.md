# Comparison results (2026-10-16)

Black-box comparison of kext2 0.1.0 with mkext2 0.1.1 and xgenext2fs 1.5.6 on an
Apple M1 Max (macOS, APFS), using `compare/` (see `compare/README.md`). Images were
checked with e2fsck 1.47.2 and read-only Linux 7.0 kernel mounts in Docker.

## Correctness (`python3 compare/run.py --seeds 20`)

- 73 cases: 1/2/4 KiB blocks; direct, single, double and triple indirection;
  sparse files with and without `-z`; 3000-entry directories; many block groups;
  tar in GNU, pax, ustar, gzip, bzip2, xz, zstd and the four sparse formats;
  root-owned tar members with devices; device tables; layering; and every
  size, inode, reserve, label, OS, squash, fill and timestamp option. Also
  error cases and 20 random trees.
- kext2: **73/73 pass**. Every image is e2fsck-clean, mounts, and holds the
  same content as the reference image: types, modes, owners, sizes, data
  hashes, symlink targets, devices, hard links, link counts and mtimes.
- Known reference-tool issues, excluded as xfail or with mkext2 as reference:
  - xgenext2fs crashes on `-q`/`-q0`.
  - xgenext2fs cannot auto-size a device-table-only image.
  - xgenext2fs on macOS turns pax UTF-8 names into NFD.
  - mkext2 omits `lost+found` when the image has no directory or archive input.
- kext2 departs on purpose from both tools (SPEC §13, Appendix C): a too-small
  `-b` is an error, not a silent raise.

## Reproducibility

kext2 is byte-identical in all 7 checks:
- the same build run twice;
- another path with reversed creation order;
- different host times under `-f`, `--timestamp` and `--clamp-timestamps`;
- `SOURCE_DATE_EPOCH` across wall-clock time;
- tar vs tar.gz of the same archive.

Its integration tests also check that the output does not depend on the
thread count.

## Performance (`python3 compare/bench.py 1024`)

Dataset: 1024 MiB, 9610 files (compare/gen.py gen_rootfs). Best of 3 runs.

| case | xgenext2fs | mkext2 | kext2 | kext2 vs xgenext2fs | kext2 vs mkext2 | valid | same content |
|---|---:|---:|---:|---:|---:|---|---|
| directory (4 KiB blocks) | 2.38 s | 0.29 s | 0.23 s | 10.5× | 1.3× | y/y/y | y/y/y |
| directory (1 KiB blocks) | 42.29 s | 0.44 s | 0.23 s | 184.9× | 1.9× | y/y/y | y/y/y |
| tar (4 KiB blocks) | 2.42 s | 0.24 s | 0.23 s | 10.5× | 1.1× | y/y/y | y/y/y |
| tar.gz (4 KiB blocks) | 2.52 s | 1.10 s | 0.85 s | 3.0× | 1.3× | y/y/y | y/y/y |
| empty 1024 MiB image | 0.01 s | 0.01 s | 0.01 s | 0.9× | 0.8× | y/y/y | y/n/y |

Peak RSS (MiB): directory (4 KiB blocks): xgenext2fs=3, mkext2=926, kext2=48; directory (1 KiB blocks): xgenext2fs=3, mkext2=906, kext2=48; tar (4 KiB blocks): xgenext2fs=4, mkext2=624, kext2=47; tar.gz (4 KiB blocks): xgenext2fs=4, mkext2=588, kext2=54; empty 1024 MiB image: xgenext2fs=2, mkext2=4, kext2=3

kext2 streams file data instead of buffering the tree. Its peak memory is about
50 MiB, against 600–930 MiB for mkext2; xgenext2fs needs 2–4 MiB but is 3–185×
slower.

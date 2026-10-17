# Memory use

Peak memory of xgenext2fs 1.5.6, mkext2 0.1.1 and kext2 0.1.0, measured from
the outside with `compare/memory.py` on 2026-10-16 (Apple M1 Max).

**In short:** the three tools make different trade-offs.

- **xgenext2fs** uses about 2–4 MiB, whatever the input.
- **kext2** uses about 45 MiB plus roughly 0.5 KiB per file.
- **mkext2** grows with the size of the image: about as much memory as the
  image's allocated blocks on macOS, and about a third of that on Linux.

## How it is measured

| platform | metric | how |
|---|---|---|
| macOS 27 (APFS) | peak memory footprint (maximum RSS in parentheses) | `/usr/bin/time -l` |
| Linux 7.0 arm64 (Docker, 10 CPUs) | maximum resident set size | GNU `/usr/bin/time -v` |

On macOS, RSS also counts clean file-backed pages, such as mapped input files
or cached data, which the OS can drop at any time. The footprint counts only
memory the process owns, so the tables use the footprint. Here the two
differ by less than 1 MiB, so none of the memory is reclaimable cache.

The rows separate what memory grows with:
- **data size:** root-filesystem-shaped trees of 64, 256 and 1024 MiB;
- **file count:** 10,000 and 100,000 files of 1 KiB;
- **image size:** empty images of 1, 4 and 16 GiB.

Each case runs once per tool. "kext2 (1 thread)" is the same build with
`KEXT2_THREADS=1`; its output is identical.

Run it again with `python3 compare/memory.py --out compare/MEMORY-new.md`.
On Linux, the run used the published binaries of xgenext2fs and mkext2 and
a kext2 built in `rust:1-bookworm`.

## macOS

Host: Darwin arm64; one run per cell; MiB of peak memory footprint (maximum RSS in parentheses).

| what grows | case | xgenext2fs | mkext2 | kext2 | kext2 (1 thread) |
|---|---|---:|---:|---:|---:|
| data size | 64 MiB tree (899 files), 4 KiB blocks | 2.5 (3.2) | 98.7 (99.6) | 42.5 (43.4) | 14.2 (15.1) |
| data size | 256 MiB tree (1820 files), 4 KiB blocks | 2.6 (3.2) | 304.3 (305.0) | 43.0 (43.9) | 14.8 (15.7) |
| data size | 1024 MiB tree (9610 files), 4 KiB blocks | 2.7 (3.3) | 924.2 (924.4) | 46.9 (47.8) | 18.6 (19.5) |
| data size | 1024 MiB tar, 4 KiB blocks | 3.4 (4.4) | 615.9 (616.9) | 45.8 (46.7) | 17.5 (18.4) |
| data size | 1024 MiB tree, 1 KiB blocks | 2.0 (2.7) | 808.6 (809.0) | 47.0 (48.0) | 18.8 (19.7) |
| file count | 10,000 files of 1 KiB, 4 KiB blocks | 2.1 (2.8) | 61.8 (62.7) | 42.6 (43.5) | 13.7 (14.6) |
| file count | 100,000 files of 1 KiB, 4 KiB blocks | 2.2 (2.8) | 484.1 (484.8) | 83.5 (84.4) | 53.2 (54.1) |
| image size | empty 1 GiB image | 1.5 (2.1) | 2.8 (3.7) | 1.7 (2.6) | 1.3 (2.2) |
| image size | empty 4 GiB image | 1.7 (2.3) | 3.3 (4.2) | 1.8 (2.7) | 1.3 (2.2) |
| image size | empty 16 GiB image | 1.8 (2.5) | 4.3 (5.2) | 1.8 (2.7) | 1.4 (2.3) |

## Linux

Host: Linux aarch64; one run per cell; MiB of maximum resident set size.

| what grows | case | xgenext2fs | mkext2 | kext2 | kext2 (1 thread) |
|---|---|---:|---:|---:|---:|
| data size | 64 MiB tree (899 files), 4 KiB blocks | 4.0 | 72.2 | 42.0 | 14.7 |
| data size | 256 MiB tree (1820 files), 4 KiB blocks | 4.1 | 178.2 | 42.7 | 15.0 |
| data size | 1024 MiB tree (9610 files), 4 KiB blocks | 4.3 | 337.2 | 46.2 | 18.4 |
| data size | 1024 MiB tar, 4 KiB blocks | 4.6 | 57.2 | 45.5 | 17.9 |
| data size | 1024 MiB tree, 1 KiB blocks | 3.5 | 385.9 | 46.5 | 18.6 |
| file count | 10,000 files of 1 KiB, 4 KiB blocks | 3.8 | 29.2 | 43.6 | 13.9 |
| file count | 100,000 files of 1 KiB, 4 KiB blocks | 3.8 | 148.2 | 77.4 | 51.0 |
| image size | empty 1 GiB image | 3.0 | 2.1 | 2.3 | 2.3 |
| image size | empty 4 GiB image | 3.3 | 2.6 | 4.4 | 2.3 |
| image size | empty 16 GiB image | 3.4 | 3.4 | 2.3 | 2.3 |

## What grows with what

| | data size | file count | image size | writer/read-ahead threads |
|---|---|---|---|---|
| xgenext2fs | flat (2–4 MiB) | flat | flat | — (single-threaded) |
| mkext2 | grows: macOS ≈ allocated image + up to ~300 MiB read-ahead; Linux ≈ ⅓ of that | grows (it follows the image) | flat when empty | read-ahead adds up to ~300 MiB |
| kext2 | flat (~45 MiB) | ~0.5 KiB per file | flat | ~30 MiB for 8 threads |

### xgenext2fs: no in-memory model

xgenext2fs keeps 2–4 MiB even with 100,000 files or a 16 GiB image. It must
work directly on the image file with a small cache, keeping no model of the
tree. Its time shows the price: 41 s on macOS and 30 s on Linux for the
1 GiB tree with 1 KiB blocks (about 0.3 s for the other two). The
specification author also measured quadratic time for large directories:
70 s for 20,000 files in one directory (SPEC §13).

### kext2: a tree in memory plus fixed write buffers

kext2 builds the whole file tree in memory, but no file data. It then
renders the image in 4 MiB chunks: up to 8 threads fill chunk buffers and one
thread writes them in block order. The measurements match that design:
- **Fixed part:** about 14 MiB with one writer thread and about 45 MiB with
  the default (up to 8), whatever the data size (64 MiB to 1 GiB) or image
  size (up to 16 GiB). The difference is the chunk buffers.
- **Per file:** from 10,000 to 100,000 files the footprint grows by 41 MiB,
  about 470 bytes per file: the tree node, its name, directory entries and
  the block plan.
- **Threads:** one writer thread saves about 30 MiB and costs up to 3× in
  time on Linux (0.27 → 0.81 s for the 1 GiB tree).

A tree of a million files would need about 0.5 GiB. That growth is the
in-memory tree, the price of a planned, contiguous, reproducible layout.

### mkext2: grows with the image

These are black-box observations: mkext2's source was not read (see
`docs/CLEANROOM.md`).

- On macOS, its footprint follows the allocated part of the image, not the
  input. The same 100,000 files of 1 KiB use 479 MiB when the image
  allocates 405 MiB (4 KiB blocks) and 199 MiB when it allocates 112 MiB
  (1 KiB blocks). kext2 uses 77 and 76 MiB for those two builds, and
  xgenext2fs 2 MiB.
- Its read-ahead adds up to ~300 MiB on top. Through its Node.js binding
  (`@tuler/mkext2`, 1 GiB tree, macOS), the footprint is 684 MiB with
  read-ahead off (`threads: 0`), 794 MiB with 2 threads, 941 MiB with 4 and
  986 MiB by default.
- On Linux it uses about a third as much (333 MiB for the 1 GiB tree, 57 MiB
  for the tar), steady across runs (333, 333, 333 MiB). It is the same with
  the image on disk or on tmpfs (346 MiB), so the output speed is not the
  cause. Why Linux and macOS differ is not visible from outside.

## kext2: reducing memory without losing speed

After this report, kext2's memory was profiled by phase. A counting
allocator, `cargo build --features memstats`, prints the live and peak heap
after the scan, the plan and the write. For the 1 GiB tree with 8 threads, the
heap peaks at 44 MiB: 2.5 MiB for the tree, 1.3 MiB for the plan, and 40 MiB
of write buffers.

### Per-file data (done)

Three changes cut the per-file cost from about 476 to about 309 bytes
(−35%), with byte-identical images and unchanged speed:

- **Host paths.** A file read from a host directory now keeps only its name;
  the directory path is stored once. Before, each file stored its full host
  path.
- **Block pointers.** The plan no longer stores the 15 `i_block` pointers
  (60 bytes) for every node. They are computed from the node's extents when
  its inode is written.
- **Smaller records.**
  - Extent records shrink from 32 to 24 bytes: a logical block number fits
    32 bits in ext2.
  - The rarely used `-z` zero-map and the symlink target are boxed, which
    shrinks every regular file's record and every tree node.
  - The node vector releases its growth slack before the plan.

Peak memory footprint on macOS, in MiB:

| case | 8 threads before | 8 threads after | 1 thread before | 1 thread after |
|---|---:|---:|---:|---:|
| 1 GiB tree (9,610 files), 4 KiB blocks | 46.8 | 45.0 | 18.5 | 16.7 |
| 1 GiB tar, 4 KiB blocks | 45.8 | 44.9 | 17.5 | 16.6 |
| 10,000 files of 1 KiB | 42.5 | 40.8 | 13.9 | 12.3 |
| 100,000 files of 1 KiB | 82.3 | 68.5 | 56.7 | 40.1 |
| empty 16 GiB image | 1.8 | 1.7 | 1.4 | 1.3 |

Checks run on these changes:

- **Bytes:** 36 builds, covering every block size, every indirection level,
  holes, sparse tar formats, device tables, `-x`, the 1 GiB tree and the
  100,000 files, gave the same SHA-256 before and after.
- **Harness:** `compare/run.py` reports 0 kext2 failures, and every
  reproducibility check is identical.
- **Speed:** wall time, min / median of 15 shuffled runs:

| case | before | after |
|---|---:|---:|
| 1 GiB tree, 4 KiB | 232 / 245 ms | 231 / 252 ms |
| 1 GiB tree, 1 KiB | 232 / 238 ms | 230 / 239 ms |
| 1 GiB tar | 196 / 202 ms | 191 / 198 ms |
| 1 GiB tar.gz | 820 / 865 ms | 822 / 852 ms |
| 100,000 files | 2,037 / 2,094 ms | 2,022 / 2,088 ms |

The differences are within the run-to-run noise: two copies of the same
"before" binary differ by as much.

### Write buffers (kept)

Rendering threads fill 4 MiB chunks, and one thread writes them in order;
up to *threads + 2* chunks are in flight. Profiling the 1 GiB tree shows the
write phase is bound by that writer:
- the writer spends about 157 ms in `pwrite`, 9 ms zeroing buffers and only
  about 20 ms waiting;
- the 8 renderers together wait about 880 ms.

The extra chunks absorb uneven render times: a region with many small files
renders slowly, because every file must be opened.

Every way of cutting the buffers cost speed on directory trees, measured
with shuffled, warmed-up runs:

| option | buffers | 1 GiB tree, 4 KiB |
|---|---:|---:|
| current: 4 MiB × 10 | 40 MiB | baseline |
| 2 MiB × 10 | 20 MiB | +3–4% |
| 2 MiB × 12, or 1 MiB × 24 | 24 MiB | +2–3% |
| 4, 5 or 6 threads (window threads + 2) | 24–32 MiB | +3–10% |
| each renderer writes its own chunk (no window) | 8–32 MiB | +35% (out-of-order writes are slow on APFS) |
| zeroing moved from the writer to the renderers | 40 MiB | no clear gain |

The buffers stay as they are. When memory matters more than time, set
`KEXT2_THREADS=1` (or `threads: 1` in Node.js). The buffers then drop to
12 MiB, about 17 MiB in total for the 1 GiB tree, at about 1.5× the time on
macOS and up to 3× on Linux. The image is the same.

## Wall time of the same runs

macOS — Wall time (s):

| case | xgenext2fs | mkext2 | kext2 | kext2 (1 thread) |
|---|---:|---:|---:|---:|
| 64 MiB tree (899 files), 4 KiB blocks | 0.16 | 0.03 | 0.03 | 0.04 |
| 256 MiB tree (1820 files), 4 KiB blocks | 0.52 | 0.08 | 0.06 | 0.10 |
| 1024 MiB tree (9610 files), 4 KiB blocks | 2.71 | 0.36 | 0.26 | 0.38 |
| 1024 MiB tar, 4 KiB blocks | 2.50 | 0.32 | 0.19 | 0.19 |
| 1024 MiB tree, 1 KiB blocks | 41.37 | 0.45 | 0.25 | 0.35 |
| 10,000 files of 1 KiB, 4 KiB blocks | 0.40 | 0.15 | 0.14 | 0.19 |
| 100,000 files of 1 KiB, 4 KiB blocks | 4.05 | 2.12 | 2.03 | 2.43 |
| empty 1 GiB image | 0.02 | 0.01 | 0.01 | 0.01 |
| empty 4 GiB image | 0.01 | 0.01 | 0.01 | 0.01 |
| empty 16 GiB image | 0.03 | 0.01 | 0.03 | 0.02 |


Linux — Wall time (s):

| case | xgenext2fs | mkext2 | kext2 | kext2 (1 thread) |
|---|---:|---:|---:|---:|
| 64 MiB tree (899 files), 4 KiB blocks | 0.11 | 0.05 | 0.03 | 0.03 |
| 256 MiB tree (1820 files), 4 KiB blocks | 0.45 | 0.52 | 0.08 | 0.07 |
| 1024 MiB tree (9610 files), 4 KiB blocks | 1.91 | 0.58 | 0.27 | 0.81 |
| 1024 MiB tar, 4 KiB blocks | 2.04 | 0.36 | 0.25 | 0.25 |
| 1024 MiB tree, 1 KiB blocks | 29.87 | 0.58 | 0.26 | 0.28 |
| 10,000 files of 1 KiB, 4 KiB blocks | 0.16 | 0.08 | 0.04 | 0.05 |
| 100,000 files of 1 KiB, 4 KiB blocks | 1.75 | 0.72 | 0.25 | 0.42 |
| empty 1 GiB image | 0.00 | 0.00 | 0.01 | 0.00 |
| empty 4 GiB image | 0.00 | 0.01 | 0.02 | 0.01 |
| empty 16 GiB image | 0.01 | 0.05 | 0.06 | 0.04 |


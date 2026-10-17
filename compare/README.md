# Black-box comparison harness

Compares `kext2`, `mkext2` and `xgenext2fs` only from the outside: it runs each
program on the same inputs and inspects the images they write. It never reads
any of the programs' sources.

Requirements: Python 3.9+, Docker (privileged containers, used for `e2fsck`,
`dumpe2fs` and read-only Linux kernel loop mounts), `zstd`, and the programs:

| program | default path | override |
|---|---|---|
| kext2 | `target/release/kext2` | `KEXT2` |
| mkext2 | `~/Documents/workspace/personal/mkext2/target/release/mkext2` | `MKEXT2` |
| xgenext2fs | `xgenext2fs` on the `PATH` | `XGENEXT2FS` |

Build the tools image once:

```bash
docker build -t kext2-tools compare/docker
```

## Correctness and reproducibility

```bash
python3 compare/run.py                 # all cases, all tools
python3 compare/run.py --only tar      # cases whose name contains "tar"
python3 compare/run.py --tools kext2,xgenext2fs --seeds 20
```

For every case each image must pass `e2fsck -fn`, mount read-only through the
Linux kernel, and hold the same content as the reference image (xgenext2fs,
or mkext2 where xgenext2fs is known to be wrong or crash): file types, modes,
uid/gid, sizes, SHA-256 of data, symlink targets, device numbers, hard-link
groups, link counts and mtimes. Directory sizes, inode numbers, atime/ctime and
layout are not compared.

The reproducibility checks build the same content twice under different
conditions (another path with reversed creation order, different host file
times with `-f`/`--timestamp`/`--clamp-timestamps`, wall clock with
`SOURCE_DATE_EPOCH`, tar vs tar.gz) and require byte-identical images. When
images differ, the check says whether the content or only the layout differs.

Results go to `compare/work/results.json`; generated inputs and images stay in
`compare/work/` (`KEXT2_WORK` to move it).

## Performance

```bash
python3 compare/bench.py 1024 --out compare/BENCH.md
```

Generates a root-filesystem-shaped dataset (default 1 GiB), times every tool
(best of 3, wall clock, peak RSS), and checks the images for validity and same
content as xgenext2fs.

## Memory

```bash
python3 compare/memory.py --out compare/MEMORY.md
```

Measures peak memory (macOS: peak memory footprint and maximum RSS; Linux:
maximum RSS) for data sizes, file counts, image sizes and kext2 thread counts.
See [`MEMORY.md`](MEMORY.md) for the results and their analysis.

## Image size and structure

```bash
python3 compare/layout.py --out compare/LAYOUT.md
```

Builds the same inputs with every tool and analyzes the images
(`analyze_image.py`): where the blocks go, the geometry, the inodes and how
inodes and data are laid out. See [`LAYOUT.md`](LAYOUT.md).


# kext2

## 0.2.0 — 2026-10-20

### Minor changes

- [9f564b7](https://github.com/tuler/kext2/commit/9f564b76b28bea526f1ff74d12df320cce26ccc7) Breaking: kext2 no longer takes the xgenext2fs command line. Images always use 4 KiB blocks, are sized from their content, reserve no blocks and always have a 16 KiB lost+found. The new `--free-space BYTES` option (`freeSpace` in Node.js) leaves that much free space, plus one free inode per 16 KiB of it. Removed: `-B`, `-b`, `-r`, `-N`, `-i`, `-m`, `-o`, `-e`, `-z`, `-x` and `-g`, the matching Node.js options, and the `reservedBlocks` result field. Kept: `-d`, `-a`, `-D`, `-L`, `-U`, `-P`, `-q`, `-f`, `--timestamp`, `--clamp-timestamps` and `$SOURCE_DATE_EPOCH`. — Thanks @tuler!

## 0.1.1 — 2026-10-17

### Patch changes

- [83b4e6e](https://github.com/tuler/kext2/commit/83b4e6e5de441045542ef91f3f0abcf95d908c4f) Use about a third less memory per file (309 instead of 476 bytes): host directory paths are stored once, block pointers are computed when inodes are written, and per-file records are smaller. Images are byte-identical and builds are as fast as before. — Thanks @tuler!


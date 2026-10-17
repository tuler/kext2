---
cargo/kext2: minor
---

Breaking: kext2 no longer takes the xgenext2fs command line. Images always use 4 KiB blocks, are sized from their content, reserve no blocks and always have a 16 KiB lost+found. The new `--free-space BYTES` option (`freeSpace` in Node.js) leaves that much free space, plus one free inode per 16 KiB of it. Removed: `-B`, `-b`, `-r`, `-N`, `-i`, `-m`, `-o`, `-e`, `-z`, `-x` and `-g`, the matching Node.js options, and the `reservedBlocks` result field. Kept: `-d`, `-a`, `-D`, `-L`, `-U`, `-P`, `-q`, `-f`, `--timestamp`, `--clamp-timestamps` and `$SOURCE_DATE_EPOCH`.

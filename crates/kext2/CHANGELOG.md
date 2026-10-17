# kext2

## 0.1.1 — 2026-10-17

### Patch changes

- [83b4e6e](https://github.com/tuler/kext2/commit/83b4e6e5de441045542ef91f3f0abcf95d908c4f) Use about a third less memory per file (309 instead of 476 bytes): host directory paths are stored once, block pointers are computed when inodes are written, and per-file records are smaller. Images are byte-identical and builds are as fast as before. — Thanks @tuler!


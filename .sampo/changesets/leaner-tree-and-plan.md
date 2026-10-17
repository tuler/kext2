---
cargo/kext2: patch
---

Use about a third less memory per file (309 instead of 476 bytes): host directory paths are stored once, block pointers are computed when inodes are written, and per-file records are smaller. Images are byte-identical and builds are as fast as before.

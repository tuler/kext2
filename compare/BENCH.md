Dataset: 1024 MiB, 9610 files (compare/gen.py gen_rootfs). Best of 3 runs.

| case | xgenext2fs | mkext2 | kext2 | kext2 vs xgenext2fs | kext2 vs mkext2 | valid | same content |
|---|---:|---:|---:|---:|---:|---|---|
| directory (4 KiB blocks) | 2.38 s | 0.29 s | 0.23 s | 10.5× | 1.3× | y/y/y | y/y/y |
| directory (1 KiB blocks) | 42.29 s | 0.44 s | 0.23 s | 184.9× | 1.9× | y/y/y | y/y/y |
| tar (4 KiB blocks) | 2.42 s | 0.24 s | 0.23 s | 10.5× | 1.1× | y/y/y | y/y/y |
| tar.gz (4 KiB blocks) | 2.52 s | 1.10 s | 0.85 s | 3.0× | 1.3× | y/y/y | y/y/y |
| empty 1024 MiB image | 0.01 s | 0.01 s | 0.01 s | 0.9× | 0.8× | y/y/y | y/n/y |

Peak RSS (MiB): directory (4 KiB blocks): xgenext2fs=3, mkext2=926, kext2=48; directory (1 KiB blocks): xgenext2fs=3, mkext2=906, kext2=48; tar (4 KiB blocks): xgenext2fs=4, mkext2=624, kext2=47; tar.gz (4 KiB blocks): xgenext2fs=4, mkext2=588, kext2=54; empty 1024 MiB image: xgenext2fs=2, mkext2=4, kext2=3

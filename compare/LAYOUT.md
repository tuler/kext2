# Image size and structure

How xgenext2fs 1.5.6, mkext2 0.1.1 and kext2 0.1.0 lay out the same content,
measured from the images with `compare/layout.py` on 2026-10-16. That script
builds each case with every tool (automatic size, `-f`), then
`compare/analyze_image.py` reads the superblock, group descriptors, bitmaps
and every inode's block map.

## Summary

**The sizes are almost the same.** The three images of the same content
differ by less than 0.3%. kext2's is the smallest in four of the five cases.

| case | file data | xgenext2fs | mkext2 | kext2 |
|---|---:|---:|---:|---:|
| 1 GiB root filesystem tree, 4 KiB blocks | 1,024.0 MiB | 1,107.5 MiB | 1,108.4 MiB | 1,105.6 MiB |
| 1 GiB root filesystem tree, 1 KiB blocks | 1,024.0 MiB | 1,088.5 MiB | 1,090.1 MiB | 1,087.4 MiB |
| 100,000 files of 1 KiB, 4 KiB blocks | 85.4 MiB | 425.1 MiB | 425.8 MiB | 424.8 MiB |
| mixed test tree, 1 KiB blocks | 30.7 MiB | 32.7 MiB | 32.6 MiB | 32.6 MiB |
| one directory of 3,000 entries, 1 KiB blocks | 0 | 3.6 MiB | 3.6 MiB | 3.6 MiB |

**The content takes the same blocks in all three.** File data, indirect
blocks, directory blocks and symlink blocks match block for block in every
case, apart from one 3,000-entry directory where xgenext2fs packs entries
into 2 fewer blocks. The size differences come only from the format
overhead around the content:

- **Reserved blocks (`-m 5`)**, the biggest difference. mkext2 reserves 5% of
  the final image (14,187 blocks in the 1 GiB case). kext2 reserves 5% of the
  size without the reserve (13,477). xgenext2fs reserves 13,500 and leaves a
  few hundred extra free blocks from its size estimate.
- **Superblock and descriptor copies.** xgenext2fs doesn't use `sparse_super`,
  so every group has a copy: 822 blocks in the 1 KiB case against 66 for the
  others.
- **Group sizes.** kext2 and xgenext2fs shrink the blocks-per-group to fit
  the image, and kext2 sometimes needs one group fewer (136 vs 137 at 1 KiB).
  mkext2 always uses full 8×block-size groups, with a short last group.

**Where the space goes.** In the 1 GiB tree at 4 KiB blocks, the image is
1.08× the file data. As a share of the file data, the extra is:
- 1.8%: the unused tails of each file's last block (266,948 data blocks hold
  1,024 MiB);
- 0.7%: indirect blocks;
- 5.1%: the root reserve;
- 0.3%: inode tables, bitmaps, superblocks and directories.

With many small files, the tails dominate: 100,000 files of 1 KiB take
100,000 blocks of 4 KiB, 4.98× the data. With 1 KiB blocks they would take
about 1.4× (measured: 112 MiB allocated, plus the reserve).

**The inodes have the same format and count in all three.**
- **Format:** revision 1, 128-byte inodes, the first ordinary inode is 11,
  and inodes 1–10 are reserved.
- **Count:** the same inode count and inodes per group in every case
  (10,368 for the 1 GiB tree: 10,235 used plus enough free to fill the last
  inode-table block of each group). kext2's specification sized inodes to
  match xgenext2fs's observed counts.
- **Features:** xgenext2fs sets none. mkext2 and kext2 set `filetype` (the
  type in each directory entry, so `ls` and `find` don't need to read the
  inode) and `sparse_super`.
- **Data:** all three store every file contiguously (about 1.00 runs per
  file), with indirect blocks next to the data they map.

**The placement differs:**

| 1 GiB tree, 4 KiB blocks | xgenext2fs | mkext2 | kext2 |
|---|---:|---:|---:|
| inode numbers follow a sorted depth-first walk | 48% | 99% | 99% |
| files in their directory's group | 9% | 99% | 99% |
| subdirectories in their parent's group | 10% | 87% | 89% |
| file data in the inode's group | 86% | 72% | 77% |

- **xgenext2fs** fills inode groups in creation order. A directory's
  children usually sit in other groups than the directory. Its directory
  entries keep input order, with `lost+found` first.
- **mkext2 and kext2** number inodes along a sorted depth-first walk and keep
  a directory's inodes together in its group, as the kernel's allocators do.
  That favors directory scans (`ls -l`, `find`, `du`), which read inodes
  directory by directory. kext2 also spreads top-level directories across
  groups (Orlov-style, deterministic) and keeps directory entries sorted by
  name, `lost+found` included.

At 1 KiB blocks a group holds only 8 MiB of data and 80 inodes, so no tool
can keep both together: mkext2 and kext2 keep the inodes with their
directory (84% of files), and xgenext2fs keeps more data next to its inode
(62% against 11–14%).

Directory blocks are about the same for all three: entries are packed to the
same block counts, except for that 3,000-entry directory.

Two quirks show up in the tables:
- In the 3,000-entry case, xgenext2fs reserves 176 blocks but leaves only 159
  free ("free beyond reserved" is −17), so non-root users have no space at
  all.
- The "dirs with name-sorted entries" rows are below 100% for xgenext2fs and
  mkext2 only because of where `lost+found` sits in the root directory.

## Full tables

### 1 GiB root filesystem tree, 4 KiB blocks

Content: 9,610 regular files, 1,024.0 MiB of file data.

| | xgenext2fs | mkext2 | kext2 |
|---|---:|---:|---:|
| **image size (MiB)** | **1,107.5** | **1,108.4** | **1,105.6** |
| image size / file data | 1.08× | 1.08× | 1.08× |
| blocks | 283,509 | 283,750 | 283,040 |
| *where the blocks go* | | | |
| superblock + descriptor copies | 18 | 10 | 10 |
| block + inode bitmaps | 18 | 18 | 18 |
| inode tables | 324 | 324 | 324 |
| file data | 266,948 | 266,948 | 266,948 |
| indirect blocks | 1,824 | 1,824 | 1,824 |
| directory blocks | 439 | 439 | 439 |
| free (incl. reserved) | 13,938 | 14,187 | 13,477 |
| reserved for root (`-m`) | 13,500 | 14,187 | 13,477 |
| free beyond reserved | 438 | 0 | 0 |
| *geometry* | | | |
| block groups | 9 | 9 | 9 |
| blocks per group | 31,504 | 32,768 | 32,768 |
| groups with a superblock copy | 9 | 5 | 5 |
| inode size (bytes) | 128 | 128 | 128 |
| inodes per group | 1,152 | 1,152 | 1,152 |
| inode table blocks per group | 36 | 36 | 36 |
| features | none | filetype sparse_super | filetype sparse_super |
| *inodes* | | | |
| inodes (total) | 10,368 | 10,368 | 10,368 |
| inodes used | 10,235 | 10,235 | 10,235 |
| inodes free | 133 | 133 | 133 |
| *layout* | | | |
| groups holding inodes | 9 | 9 | 9 |
| groups holding top-level dirs | 6 | 4 | 5 |
| subdirs in parent's group | 10% | 87% | 89% |
| files in parent dir's group | 9% | 99% | 99% |
| file data in inode's group | 86% | 72% | 77% |
| contiguous files | 100% | 100% | 100% |
| runs per file (avg) | 1.00 | 1.00 | 1.00 |
| inode numbers follow sorted walk | 48% | 99% | 99% |
| dirs with name-sorted entries | 100% | 100% | 100% |

### 1 GiB root filesystem tree, 1 KiB blocks

Content: 9,610 regular files, 1,024.0 MiB of file data.

| | xgenext2fs | mkext2 | kext2 |
|---|---:|---:|---:|
| **image size (MiB)** | **1,088.5** | **1,090.1** | **1,087.4** |
| image size / file data | 1.06× | 1.06× | 1.06× |
| blocks | 1,114,587 | 1,116,279 | 1,113,472 |
| *where the blocks go* | | | |
| boot block | 1 | 1 | 1 |
| superblock + descriptor copies | 822 | 66 | 66 |
| block + inode bitmaps | 274 | 274 | 272 |
| inode tables | 1,370 | 1,370 | 1,360 |
| file data | 1,053,341 | 1,053,341 | 1,053,341 |
| indirect blocks | 4,975 | 4,975 | 4,975 |
| directory blocks | 439 | 439 | 439 |
| free (incl. reserved) | 53,365 | 55,813 | 53,018 |
| reserved for root (`-m`) | 53,075 | 55,813 | 53,018 |
| free beyond reserved | 290 | 0 | 0 |
| *geometry* | | | |
| block groups | 137 | 137 | 136 |
| blocks per group | 8,136 | 8,192 | 8,192 |
| groups with a superblock copy | 137 | 11 | 11 |
| inode size (bytes) | 128 | 128 | 128 |
| inodes per group | 80 | 80 | 80 |
| inode table blocks per group | 10 | 10 | 10 |
| features | none | filetype sparse_super | filetype sparse_super |
| *inodes* | | | |
| inodes (total) | 10,960 | 10,960 | 10,880 |
| inodes used | 10,235 | 10,235 | 10,235 |
| inodes free | 725 | 725 | 645 |
| *layout* | | | |
| groups holding inodes | 137 | 128 | 133 |
| groups holding top-level dirs | 7 | 6 | 6 |
| subdirs in parent's group | 0% | 40% | 39% |
| files in parent dir's group | 1% | 84% | 84% |
| file data in inode's group | 62% | 11% | 14% |
| contiguous files | 99% | 99% | 99% |
| runs per file (avg) | 1.01 | 1.01 | 1.01 |
| inode numbers follow sorted walk | 71% | 99% | 99% |
| dirs with name-sorted entries | 100% | 100% | 100% |

### 100,000 files of 1 KiB, 4 KiB blocks

Content: 100,001 regular files, 85.4 MiB of file data.

| | xgenext2fs | mkext2 | kext2 |
|---|---:|---:|---:|
| **image size (MiB)** | **425.1** | **425.8** | **424.8** |
| image size / file data | 4.98× | 4.98× | 4.97× |
| blocks | 108,831 | 109,014 | 108,741 |
| *where the blocks go* | | | |
| superblock + descriptor copies | 8 | 6 | 6 |
| block + inode bitmaps | 8 | 8 | 8 |
| inode tables | 3,132 | 3,132 | 3,132 |
| file data | 100,000 | 100,000 | 100,000 |
| indirect blocks | 1 | 1 | 1 |
| directory blocks | 417 | 417 | 417 |
| free (incl. reserved) | 5,265 | 5,450 | 5,177 |
| reserved for root (`-m`) | 5,182 | 5,450 | 5,177 |
| free beyond reserved | 83 | 0 | 0 |
| *geometry* | | | |
| block groups | 4 | 4 | 4 |
| blocks per group | 27,208 | 32,768 | 27,192 |
| groups with a superblock copy | 4 | 3 | 3 |
| inode size (bytes) | 128 | 128 | 128 |
| inodes per group | 25,056 | 25,056 | 25,056 |
| inode table blocks per group | 783 | 783 | 783 |
| features | none | filetype sparse_super | filetype sparse_super |
| *inodes* | | | |
| inodes (total) | 100,224 | 100,224 | 100,224 |
| inodes used | 100,112 | 100,112 | 100,112 |
| inodes free | 112 | 112 | 112 |
| *layout* | | | |
| groups holding inodes | 4 | 4 | 4 |
| groups holding top-level dirs | 4 | 4 | 4 |
| subdirs in parent's group | 24% | 27% | 26% |
| files in parent dir's group | 25% | 97% | 100% |
| file data in inode's group | 100% | 59% | 100% |
| contiguous files | 100% | 100% | 100% |
| runs per file (avg) | 1.00 | 1.00 | 1.00 |
| inode numbers follow sorted walk | 50% | 100% | 100% |
| dirs with name-sorted entries | 99% | 99% | 100% |

### mixed test tree (links, sparse sizes), 1 KiB blocks

Content: 151 regular files, 30.7 MiB of file data.

| | xgenext2fs | mkext2 | kext2 |
|---|---:|---:|---:|
| **image size (MiB)** | **32.7** | **32.6** | **32.6** |
| image size / file data | 1.06× | 1.06× | 1.06× |
| blocks | 33,439 | 33,423 | 33,338 |
| *where the blocks go* | | | |
| boot block | 1 | 1 | 1 |
| superblock + descriptor copies | 10 | 6 | 6 |
| block + inode bitmaps | 10 | 10 | 10 |
| inode tables | 30 | 30 | 30 |
| file data | 31,494 | 31,494 | 31,494 |
| indirect blocks | 154 | 154 | 154 |
| directory blocks | 45 | 45 | 45 |
| symlink blocks | 12 | 12 | 12 |
| free (incl. reserved) | 1,683 | 1,671 | 1,586 |
| reserved for root (`-m`) | 1,592 | 1,671 | 1,586 |
| free beyond reserved | 91 | 0 | 0 |
| *geometry* | | | |
| block groups | 5 | 5 | 5 |
| blocks per group | 6,688 | 8,192 | 8,192 |
| groups with a superblock copy | 5 | 3 | 3 |
| inode size (bytes) | 128 | 128 | 128 |
| inodes per group | 48 | 48 | 48 |
| inode table blocks per group | 6 | 6 | 6 |
| features | none | filetype sparse_super | filetype sparse_super |
| *inodes* | | | |
| inodes (total) | 240 | 240 | 240 |
| inodes used | 213 | 213 | 213 |
| inodes free | 27 | 27 | 27 |
| *layout* | | | |
| groups holding inodes | 5 | 5 | 5 |
| groups holding top-level dirs | 4 | 2 | 2 |
| subdirs in parent's group | 17% | 76% | 59% |
| files in parent dir's group | 19% | 63% | 85% |
| file data in inode's group | 79% | 54% | 50% |
| contiguous files | 99% | 98% | 97% |
| runs per file (avg) | 1.02 | 1.02 | 1.03 |
| inode numbers follow sorted walk | 61% | 93% | 93% |
| dirs with name-sorted entries | 97% | 97% | 100% |

### one directory of 3,000 entries, 1 KiB blocks

Content: 3,000 regular files, 0.0 MiB of file data.

| | xgenext2fs | mkext2 | kext2 |
|---|---:|---:|---:|
| **image size (MiB)** | **3.6** | **3.6** | **3.6** |
| image size / file data | 347.63× | 350.36× | 349.42× |
| blocks | 3,697 | 3,726 | 3,716 |
| *where the blocks go* | | | |
| boot block | 1 | 1 | 1 |
| superblock + descriptor copies | 2 | 2 | 2 |
| block + inode bitmaps | 2 | 2 | 2 |
| inode tables | 380 | 380 | 380 |
| file data | 3,000 | 3,000 | 3,000 |
| indirect blocks | 2 | 2 | 2 |
| directory blocks | 141 | 143 | 143 |
| symlink blocks | 10 | 10 | 10 |
| free (incl. reserved) | 159 | 186 | 176 |
| reserved for root (`-m`) | 176 | 186 | 176 |
| free beyond reserved | -17 | 0 | 0 |
| *geometry* | | | |
| block groups | 1 | 1 | 1 |
| blocks per group | 3,696 | 8,192 | 3,720 |
| groups with a superblock copy | 1 | 1 | 1 |
| inode size (bytes) | 128 | 128 | 128 |
| inodes per group | 3,040 | 3,040 | 3,040 |
| inode table blocks per group | 380 | 380 | 380 |
| features | none | filetype sparse_super | filetype sparse_super |
| *inodes* | | | |
| inodes (total) | 3,040 | 3,040 | 3,040 |
| inodes used | 3,034 | 3,034 | 3,034 |
| inodes free | 6 | 6 | 6 |
| *layout* | | | |
| groups holding inodes | 1 | 1 | 1 |
| groups holding top-level dirs | 1 | 1 | 1 |
| subdirs in parent's group | 100% | 100% | 100% |
| files in parent dir's group | 100% | 100% | 100% |
| file data in inode's group | 100% | 100% | 100% |
| contiguous files | 100% | 100% | 100% |
| runs per file (avg) | 1.00 | 1.00 | 1.00 |
| inode numbers follow sorted walk | 100% | 100% | 100% |
| dirs with name-sorted entries | 80% | 80% | 100% |


# @tuler/kext2

Reproducible ext2 filesystem images from Node.js, without root privileges.
This is the native binding of [kext2](https://github.com/tuler/kext2). It takes
the same arguments as cartesi's `xgenext2fs` 1.5.6 and stores the same
content, in valid, byte-for-byte reproducible images. Its design comes from
the Linux kernel's ext2 driver.

```ts
import { build, buildBuffer, buildSync, kext2, version } from '@tuler/kext2'

// the command line, in process (the same arguments as xgenext2fs)
const run = await kext2(['--block-size', '4096', '--faketime', '--root', 'rootfs', 'out.ext2'])
console.log(run.stderr) // diagnostics, as the tool prints them

// typed options; builds run on the libuv thread pool
const info = await build('out.ext2', {
  blockSize: 4096,
  faketime: true,
  readjustment: '+1000', // 1000 free blocks over the computed size ('N': exactly N blocks)
  layers: [
    { type: 'tar', path: 'rootfs.tar.gz' },
    { type: 'dir', path: 'extra', target: '/opt' },
    { type: 'devtable', data: Buffer.from('/dev/console c 600 0 0 5 1 - - -\n') },
  ],
})
console.log(info.blocks, info.freeBlocks, info.messages)

// in memory
const image: Buffer = await buildBuffer({ blockSize: 4096, sizeInBlocks: 2560, faketime: true })
```

| option | command line | |
|---|---|---|
| `blockSize` | `-B` | 1024 (default), 2048 or 4096 |
| `sizeInBlocks` | `-b` | exact size; computed from the inputs otherwise |
| `readjustment` | `-r` | `'+N'`: N extra free blocks; `'N'`: exactly N blocks |
| `bytesPerInode`, `numberOfInodes` | `-i`, `-N` | inode count |
| `reservedPercentage` | `-m` | default 5 |
| `volumeLabel`, `creatorOs` | `-L`, `-o` | |
| `fillValue`, `allowHoles` | `-e`, `-z` | |
| `faketime`, `timestamp`, `clampTimestamps` | `-f`, `--timestamp`, `--clamp-timestamps` | mutually exclusive |
| `sourceDateEpoch` | `$SOURCE_DATE_EPOCH` | filesystem time otherwise |
| `squashUids`, `squashPerms` | `-U`, `-P` | |
| `startingImage`, `blockMaps` | `-x`, `-g` | |
| `layers` | `-d`, `-a`, `-D` | `{ type: 'dir' \| 'tar' \| 'devtable', path \| data, target? }`, in order |
| `threads`, `verbose` | `-v` | the image does not depend on `threads` |

Errors reject the promise (or throw, for `buildSync`) with kext2's message.
Writing to standard output (`'-'`) is not supported from Node.js.

Prebuilt binaries: macOS (x64, arm64), Linux glibc ≥ 2.17 (x64, arm64),
Windows x64.

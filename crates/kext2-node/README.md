# @tuler/kext2

Reproducible ext2 filesystem images from Node.js, without root privileges.
This is the native binding of [kext2](https://github.com/tuler/kext2). It
stores the same file content as cartesi's `xgenext2fs` 1.5.6, in valid,
byte-for-byte reproducible images with 4 KiB blocks. Its design comes from
the Linux kernel's ext2 driver.

```ts
import { build, buildBuffer, buildSync, kext2, version } from '@tuler/kext2'

// the command line, in process
const run = await kext2(['--faketime', '--free-space', '64M', '--root', 'rootfs', 'out.ext2'])
console.log(run.stderr) // diagnostics, as the tool prints them

// typed options; builds run on the libuv thread pool
const info = await build('out.ext2', {
  faketime: true,
  freeSpace: 64 << 20, // bytes free on top of the content, and one free inode per 16 KiB
  layers: [
    { type: 'tar', path: 'rootfs.tar.gz' },
    { type: 'dir', path: 'extra', target: '/opt' },
    { type: 'devtable', data: Buffer.from('/dev/console c 600 0 0 5 1 - - -\n') },
  ],
})
console.log(info.blocks, info.freeBlocks, info.freeInodes, info.messages)

// in memory
const image: Buffer = await buildBuffer({ freeSpace: 10 << 20, faketime: true })
```

The image size is computed from the content plus `freeSpace`. No blocks are
reserved for root, and lost+found always exists.

| option | command line | |
|---|---|---|
| `freeSpace` | `--free-space` | free bytes (default 0), plus one free inode per 16 KiB |
| `volumeLabel` | `-L` | at most 16 bytes |
| `faketime`, `timestamp`, `clampTimestamps` | `-f`, `--timestamp`, `--clamp-timestamps` | mutually exclusive |
| `sourceDateEpoch` | `$SOURCE_DATE_EPOCH` | filesystem time otherwise |
| `squashUids`, `squashPerms` | `-U`, `-P` | |
| `layers` | `-d`, `-a`, `-D` | `{ type: 'dir' \| 'tar' \| 'devtable', path \| data, target? }`, in order |
| `threads`, `verbose` | `-v` | the image does not depend on `threads` |

Errors reject the promise (or throw, for `buildSync`) with kext2's message.
Writing to standard output (`'-'`) is not supported from Node.js.

Prebuilt binaries: macOS (x64, arm64), Linux glibc ≥ 2.17 (x64, arm64),
Windows x64.

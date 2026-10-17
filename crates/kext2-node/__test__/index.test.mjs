import { test } from 'node:test'
import assert from 'node:assert/strict'
import { execFileSync } from 'node:child_process'
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, symlinkSync, writeFileSync } from 'node:fs'
import { createRequire } from 'node:module'
import { tmpdir } from 'node:os'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

const require = createRequire(import.meta.url)
const ext2 = require('../index.js')

const here = dirname(fileURLToPath(import.meta.url))
const exe = process.platform === 'win32' ? 'kext2.exe' : 'kext2'
const cli = process.env.KEXT2_BIN ?? join(here, '..', '..', '..', 'target', 'release', exe)

function makeTree() {
  const dir = mkdtempSync(join(tmpdir(), 'kext2-node-'))
  const root = join(dir, 'root')
  mkdirSync(join(root, 'etc', 'deep', 'er'), { recursive: true })
  writeFileSync(join(root, 'etc', 'hostname'), 'kext2\n')
  writeFileSync(join(root, 'etc', 'deep', 'er', 'big.bin'), Buffer.alloc(300_000, 7))
  writeFileSync(join(root, 'empty'), '')
  if (process.platform !== 'win32') symlinkSync('etc/hostname', join(root, 'link'))
  return { dir, root }
}

// The kext2 command line tool, for byte-for-byte comparisons.
function reference(args, cwd) {
  execFileSync(cli, args, { cwd, stdio: ['ignore', 'ignore', 'pipe'] })
}

test('version', () => {
  assert.match(ext2.version(), /^\d+\.\d+\.\d+/)
})

test('build, buildSync and buildBuffer match the command line tool', { skip: !existsSync(cli) && 'kext2 CLI not built' }, async () => {
  const { dir, root } = makeTree()
  try {
    const args = ['-B', '4096', '-f', '-r', '+100', '-L', 'node', '-d', root]
    reference([...args, join(dir, 'ref.ext2')], dir)
    const ref = readFileSync(join(dir, 'ref.ext2'))
    const opts = { blockSize: 4096, faketime: true, readjustment: '+100', volumeLabel: 'node', layers: [{ type: 'dir', path: root }] }

    const info = await ext2.build(join(dir, 'a.ext2'), opts)
    assert.ok(ref.equals(readFileSync(join(dir, 'a.ext2'))))
    assert.equal(info.blockSize, 4096)
    assert.equal(info.blocks * 4096, ref.length)
    assert.ok(info.freeBlocks >= 100)
    assert.ok(info.inodes > info.freeInodes)
    assert.ok(info.messages.some((m) => m.level === 'info' && m.text.includes(root)))

    ext2.buildSync(join(dir, 'b.ext2'), opts)
    assert.ok(ref.equals(readFileSync(join(dir, 'b.ext2'))))

    assert.ok(ref.equals(await ext2.buildBuffer(opts)))

    const run = await ext2.kext2([...args, join(dir, 'c.ext2')])
    assert.ok(ref.equals(readFileSync(join(dir, 'c.ext2'))))
    assert.match(run.stderr, /copying from directory/)
    assert.equal(run.result.blocks, info.blocks)
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})

test('tar archives from files and buffers', { skip: process.platform === 'win32' && 'needs tar' }, async () => {
  const { dir, root } = makeTree()
  try {
    const tarPath = join(dir, 'root.tar')
    execFileSync('tar', ['-cf', tarPath, '-C', root, '.'], { env: { ...process.env, COPYFILE_DISABLE: '1' } })
    const opts = (layer) => ({ blockSize: 4096, faketime: true, readjustment: '+500', layers: [layer] })
    const fromFile = await ext2.buildBuffer(opts({ type: 'tar', path: tarPath }))
    const fromBuffer = await ext2.buildBuffer(opts({ type: 'tar', data: readFileSync(tarPath) }))
    assert.ok(fromFile.equals(fromBuffer))
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})

test('layer targets, device tables and fixed timestamps', async () => {
  const { dir, root } = makeTree()
  try {
    const opts = {
      blockSize: 1024,
      timestamp: 1700000000,
      squashUids: 0,
      readjustment: '+300',
      layers: [
        { type: 'devtable', data: Buffer.from('/opt d 755 0 0 - - - - -\n/dev d 755 0 0 - - - - -\n/dev/null c 666 0 0 1 3 - - -\n') },
        { type: 'dir', path: root, target: '/opt/app' },
      ],
    }
    const img = await ext2.buildBuffer(opts)
    // superblock s_wtime at offset 1024 + 48
    assert.equal(img.readUInt32LE(1024 + 48), 1700000000)
    assert.equal(img.readUInt16LE(1024 + 56), 0xef53)
    // the same build twice is identical, whatever the thread count
    assert.ok(img.equals(await ext2.buildBuffer({ ...opts, threads: 1 })))
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})

test('source date epoch sets the filesystem time', async () => {
  const img = await ext2.buildBuffer({ sizeInBlocks: 2048, sourceDateEpoch: 1600000000 })
  assert.equal(img.readUInt32LE(1024 + 48), 1600000000)
})

test('verbose output and warnings go to messages', async () => {
  const info = await ext2.build(join(tmpdir(), `kext2-node-${process.pid}.ext2`), {
    sizeInBlocks: 4096,
    readjustment: '+10',
    verbose: true,
    faketime: true,
  })
  rmSync(join(tmpdir(), `kext2-node-${process.pid}.ext2`), { force: true })
  assert.ok(info.messages.some((m) => m.level === 'warning' && /-r/.test(m.text)))
  assert.ok(info.messages.some((m) => m.level === 'info' && /block size 1024/.test(m.text)))
})

test('errors reject with a message', async () => {
  const out = join(tmpdir(), `kext2-node-err-${process.pid}.ext2`)
  await assert.rejects(ext2.buildBuffer({ blockSize: 1000 }), /block size/i)
  await assert.rejects(ext2.build(out, { layers: [{ type: 'dir', path: '/does/not/exist' }] }), /does\/not\/exist/)
  await assert.rejects(ext2.build(out, { sizeInBlocks: 5, layers: [{ type: 'devtable', data: Buffer.from('/a d 755 0 0 - - - - -\n') }] }), /too small/)
  await assert.rejects(ext2.kext2(['--bogus', 'x.img']), /bogus/)
  assert.throws(() => ext2.buildSync(out, { readjustment: '+5x' }), /5x/)
  assert.throws(() => ext2.buildSync(out, { layers: [{ type: 'zip', path: 'a' }] }), /unknown layer type/)
  assert.throws(() => ext2.buildSync(out, { layers: [{ type: 'dir', data: Buffer.from('x') }] }), /need a path/)
  assert.throws(() => ext2.buildSync('-', {}), /standard output/)
  assert.ok(!existsSync(out))
})

test('help and version through the command line entry point', async () => {
  assert.match((await ext2.kext2(['--help'])).stderr, /Usage: kext2/)
  assert.equal((await ext2.kext2(['--version'])).stdout, `kext2 ${ext2.version()}\n`)
})

test('builds run off the main thread', async () => {
  const { dir, root } = makeTree()
  try {
    let ticks = 0
    const timer = setInterval(() => ticks++, 1)
    await ext2.build(join(dir, 'a.ext2'), {
      blockSize: 1024,
      faketime: true,
      sizeInBlocks: 2_000_000,
      fillValue: 7,
      layers: [{ type: 'dir', path: root }],
    })
    clearInterval(timer)
    assert.ok(ticks > 0, 'event loop should keep running during the build')
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})

"""Deterministic input generators: random trees, tar archives, device tables,
and a root-filesystem-shaped benchmark dataset."""

import io
import os
import random
import subprocess
import tarfile

from common import rmtree

NAME_CHARS = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789._-+ =~,@"


def rand_name(rng, maxlen=40):
    n = rng.choice([1, 2, 3, 5, 8, 12, 20, maxlen])
    s = "".join(rng.choice(NAME_CHARS) for _ in range(n))
    if rng.random() < 0.05:
        s += "é中"  # some UTF-8
    if s in (".", ".."):
        s += "x"
    return s


def block_thresholds(bs):
    """File sizes around the ext2 indirection boundaries for block size bs."""
    p = bs // 4
    direct = 12 * bs
    single = direct + p * bs
    return [direct - 1, direct, direct + 1, single - 1, single, single + 1]


def write_data(path, size, rng, sparse=False):
    with open(path, "wb") as f:
        if size == 0:
            return
        if sparse:
            f.truncate(size)
            # a few islands of data in a sea of holes
            for _ in range(rng.randint(1, 4)):
                off = rng.randrange(0, size)
                n = min(rng.randint(1, 9000), size - off)
                f.seek(off)
                f.write(rng.randbytes(n))
            return
        if size > (1 << 20):
            # big files: repeat a random 64 KiB chunk (fast to write, still distinct)
            chunk = rng.randbytes(65536)
            left = size
            while left:
                n = min(left, len(chunk))
                f.write(chunk[:n])
                left -= n
        else:
            f.write(rng.randbytes(size))


def gen_tree(root, seed, nfiles=150, ndirs=25, bs=1024, big=False, sparse=False, fifos=True,
             long_symlinks=False, long_names=True, symlink_max=500, max_name=40, mtime_base=1_600_000_000):
    """Create a random tree at root. Everything is derived from seed."""
    rng = random.Random(seed)
    rmtree(root)
    os.makedirs(root)
    dirs = [root]
    for _ in range(ndirs):
        parent = rng.choice(dirs)
        d = os.path.join(parent, "d_" + rand_name(rng, max_name))
        if not os.path.exists(d):
            os.mkdir(d)
            dirs.append(d)
    sizes = [0, 1, 7, 100, 511, 512, 1023, 1024, 1025, 4095, 4096, 4097] + block_thresholds(bs)
    files = []
    for i in range(nfiles):
        parent = rng.choice(dirs)
        p = os.path.join(parent, f"f{i}_" + rand_name(rng, max_name))
        if os.path.exists(p):
            continue
        r = rng.random()
        if r < 0.6:
            size = rng.choice(sizes)
        elif r < 0.95:
            size = int(rng.lognormvariate(8, 2)) % (4 << 20)
        else:
            size = rng.randint(1 << 20, 3 << 20)
        write_data(p, size, rng)
        files.append(p)
    if long_names:
        n = os.path.join(rng.choice(dirs), "L" * 255)
        write_data(n, 10, rng)
        files.append(n)
    # big file crossing into double (and for 1 KiB blocks, triple) indirection
    if big:
        p = bs // 4
        triple = 12 * bs + p * bs + p * p * bs
        size = triple + 3 * bs + 17 if bs == 1024 else 12 * bs + p * bs + 5 * bs + 3
        write_data(os.path.join(root, "big.bin"), size, rng, sparse=sparse)
    if sparse:
        for i in range(5):
            write_data(os.path.join(rng.choice(dirs), f"sparse{i}"), rng.randint(1, 3) << 20, rng, sparse=True)
        z = os.path.join(root, "zeros")
        with open(z, "wb") as f:
            f.write(b"\0" * (64 * 1024) + b"x" + b"\0" * (64 * 1024))
    # hard links
    for i in range(min(8, len(files))):
        src = rng.choice(files)
        dst = os.path.join(rng.choice(dirs), f"hl{i}_" + rand_name(rng, 10))
        if not os.path.exists(dst):
            os.link(src, dst)
    # symlinks: fast (< 60 bytes) and slow (>= 60 bytes, < block size)
    for i in range(20):
        n = min(symlink_max, rng.choice([1, 5, 30, 59, 60, 61, 200, 500]))
        if long_symlinks:
            n = rng.choice([bs - 1, bs, bs + 1])
        target = ("../" * 3 + "t" * n)[:n]
        dst = os.path.join(rng.choice(dirs), f"sl{i}_" + rand_name(rng, 10))
        if not os.path.lexists(dst):
            os.symlink(target, dst)
    if fifos:
        for i in range(3):
            dst = os.path.join(rng.choice(dirs), f"fifo{i}")
            if not os.path.lexists(dst):
                os.mkfifo(dst)
    # empty directories, permissions and times
    for i in range(3):
        os.makedirs(os.path.join(rng.choice(dirs), f"empty{i}"), exist_ok=True)
    allpaths = []
    for dp, dn, fn in os.walk(root):
        for n in dn + fn:
            allpaths.append(os.path.join(dp, n))
    allpaths.sort()
    for path in allpaths:
        if os.path.islink(path):
            continue
        st = os.lstat(path)
        if os.path.isdir(path):
            mode = rng.choice([0o755, 0o750, 0o700, 0o775, 0o1777, 0o2755])
        else:
            mode = rng.choice([0o644, 0o600, 0o755, 0o444, 0o4755, 0o2755, 0o640])
        os.chmod(path, mode)
    for path in sorted(allpaths, key=lambda p: -p.count("/")):  # children first
        t = mtime_base + rng.randrange(0, 100_000_000)
        os.utime(path, (t, t), follow_symlinks=False)
    os.utime(root, (mtime_base, mtime_base))
    return root


def make_tar(src, out, fmt="gnu", compress=None):
    """Tar the tree at src deterministically (sorted, as the current user)."""
    fmts = {"gnu": tarfile.GNU_FORMAT, "pax": tarfile.PAX_FORMAT, "ustar": tarfile.USTAR_FORMAT}
    mode = {None: "w", "gz": "w:gz", "bz2": "w:bz2", "xz": "w:xz", "zst": "w"}[compress]
    tmp = out + ".tmp" if compress == "zst" else out
    with tarfile.open(tmp, mode, format=fmts[fmt]) as tf:
        def add(path, arc):
            tf.add(path, arcname=arc, recursive=False)
            if os.path.isdir(path) and not os.path.islink(path):
                for n in sorted(os.listdir(path)):
                    add(os.path.join(path, n), arc + "/" + n)
        for n in sorted(os.listdir(src)):
            add(os.path.join(src, n), n)
    if compress == "zst":
        subprocess.run(["zstd", "-q", "-f", "--rm", tmp, "-o", out], check=True)
    return out


def make_tar_root_owned(src, out, uid=0, gid=0, big_ids=False):
    """A tar whose members are owned by uid/gid (or by large ids), plus device
    nodes, which a non-root user cannot create on disk."""
    with tarfile.open(out, "w", format=tarfile.GNU_FORMAT) as tf:
        def filt(ti):
            ti.uid, ti.gid = (uid, gid) if not big_ids else (70000 + len(ti.name), 80000)
            ti.uname = ti.gname = ""
            return ti
        for n in sorted(os.listdir(src)):
            tf.add(os.path.join(src, n), arcname=n, filter=filt)
        for i, (t, maj, mnr) in enumerate([(tarfile.CHRTYPE, 1, 3), (tarfile.BLKTYPE, 8, 1), (tarfile.CHRTYPE, 4, 64),
                                           (tarfile.CHRTYPE, 136, 255)]):
            ti = tarfile.TarInfo(f"devnode{i}")
            ti.type, ti.devmajor, ti.devminor, ti.mode, ti.mtime = t, maj, mnr, 0o620, 1_500_000_000
            tf.addfile(filt(ti))
    return out


DEVTABLE = """\
# name type mode uid gid major minor start inc count
/dev d 755 0 0 - - - - -
/dev/console c 600 0 0 5 1 - - -
/dev/null c 666 0 0 1 3 - - -
/dev/zero c 666 0 0 1 5 - - -
/dev/tty c 666 0 5 5 0 - - -
/dev/ttyS c 660 0 5 4 64 0 1 4
/dev/sda b 660 0 6 8 0 - - -
/dev/sda b 660 0 6 8 1 1 1 3
/dev/initctl p 600 0 0 - - - - -
/etc d 755 0 0 - - - - -
/etc/shadow-dir d 700 0 0 - - - - -
/var d 755 0 0 - - - - -
/var/run d 755 0 0 - - - - -
"""


def make_devtable(out, text=DEVTABLE):
    with open(out, "w") as f:
        f.write(text)
    return out


def gen_rootfs(root, mib, seed=1):
    """A dataset shaped like a root filesystem: many small files, some large
    ones, deep-ish directories, symlinks. About `mib` MiB of data."""
    rng = random.Random(seed)
    rmtree(root)
    os.makedirs(root)
    target = mib << 20
    total = 0
    tops = ["bin", "etc", "lib", "usr/bin", "usr/lib", "usr/share/doc", "usr/share/locale", "var/lib", "opt/app"]
    dirs = []
    for t in tops:
        os.makedirs(os.path.join(root, t), exist_ok=True)
        dirs.append(os.path.join(root, t))
    nd = max(40, mib * 2 // 5)
    for i in range(nd):
        d = os.path.join(rng.choice(dirs), f"pkg{i}")
        os.makedirs(d, exist_ok=True)
        dirs.append(d)
    chunk = os.urandom(1 << 20)
    i = 0
    while total < target:
        r = rng.random()
        if r < 0.80:
            size = rng.randint(0, 8 << 10)
        elif r < 0.99:
            size = rng.randint(8 << 10, 256 << 10)
        else:
            size = rng.randint(1 << 20, 16 << 20)
        size = min(size, target - total) if total + size > target else size
        p = os.path.join(rng.choice(dirs), f"file{i}.dat")
        with open(p, "wb") as f:
            left, off = size, (i * 4099) % len(chunk)
            while left:
                n = min(left, len(chunk) - off)
                f.write(chunk[off:off + n])
                left -= n
                off = 0
        total += size
        i += 1
        if i % 50 == 0:
            os.symlink(f"../file{i - 1}.dat", os.path.join(rng.choice(dirs), f"link{i}"))
    return root, i


def make_sparse_tars(outdir, seed=21):
    """Sparse archives written directly (no sparse host files needed): old GNU
    'S' members with extension headers, and pax sparse 0.0, 0.1 and 1.0.
    Returns {format: path}."""
    rng = random.Random(seed)
    real = 9 * 1024 * 1024 + 123
    regions = []
    off = 0
    for i in range(9):
        off += rng.randint(1, 900_000)
        n = rng.randint(1, 30_000)
        if off + n > real:
            break
        regions.append((off, n))
        off += n
    regions.append((real, 0))
    data = b"".join(rng.randbytes(n) for _, n in regions)

    def octal(v, w):
        return ("%0*o" % (w - 1, v)).encode() + b"\0"

    def gnu_header(name, size, real, regs):
        h = bytearray(512)
        h[0:len(name)] = name
        h[100:108] = octal(0o644, 8)
        h[108:116] = octal(0, 8)
        h[116:124] = octal(0, 8)
        h[124:136] = octal(size, 12)
        h[136:148] = octal(1_600_000_000, 12)
        h[156] = ord("S")
        h[257:265] = b"ustar  \0"
        for i, (o, n) in enumerate(regs[:4]):
            h[386 + 24 * i:398 + 24 * i] = octal(o, 12)
            h[398 + 24 * i:410 + 24 * i] = octal(n, 12)
        h[482] = 1 if len(regs) > 4 else 0
        h[483:495] = octal(real, 12)
        h[148:156] = b" " * 8
        h[148:156] = ("%06o\0 " % sum(h)).encode()
        out = bytes(h)
        rest = regs[4:]
        while rest:
            x = bytearray(512)
            for i, (o, n) in enumerate(rest[:21]):
                x[24 * i:24 * i + 12] = octal(o, 12)
                x[24 * i + 12:24 * i + 24] = octal(n, 12)
            rest = rest[21:]
            x[504] = 1 if rest else 0
            out += bytes(x)
        return out

    def pad(b):
        return b + b"\0" * ((512 - len(b) % 512) % 512)

    paths = {}
    p = os.path.join(outdir, "sparse.oldgnu.tar")
    with open(p, "wb") as f:
        f.write(gnu_header(b"sparse-file", len(data), real, regions) + pad(data) + b"\0" * 1024)
    paths["oldgnu"] = p
    for ver in ("0.0", "0.1", "1.0"):
        p = os.path.join(outdir, f"sparse.pax{ver}.tar")
        with tarfile.open(p, "w", format=tarfile.PAX_FORMAT) as tf:
            ti = tarfile.TarInfo("sparse-file")
            ti.mtime = 1_600_000_000
            payload = data
            if ver == "0.0":
                # tarfile keeps one value per key; write the repeated keys by hand below
                ti.pax_headers = {"GNU.sparse.size": str(real)}
            elif ver == "0.1":
                ti.name = "GNUSparseFile.0/sparse-file"
                ti.pax_headers = {"GNU.sparse.size": str(real), "GNU.sparse.name": "sparse-file",
                                  "GNU.sparse.map": ",".join(f"{o},{n}" for o, n in regions)}
            else:
                ti.name = "GNUSparseFile.0/sparse-file"
                ti.pax_headers = {"GNU.sparse.major": "1", "GNU.sparse.minor": "0", "GNU.sparse.name": "sparse-file",
                                  "GNU.sparse.realsize": str(real)}
                m = ("%d\n" % len(regions) + "".join(f"{o}\n{n}\n" for o, n in regions)).encode()
                payload = pad(m) + data
            ti.size = len(payload)
            if ver == "0.0":
                recs = [("GNU.sparse.size", str(real)), ("GNU.sparse.numblocks", str(len(regions)))]
                for o, n in regions:
                    recs += [("GNU.sparse.offset", str(o)), ("GNU.sparse.numbytes", str(n))]
                body = b""
                for k, v in recs:
                    r = f" {k}={v}\n"
                    n = len(r) + 1
                    while len(str(n)) + len(r) != n:
                        n = len(str(n)) + len(r)
                    body += (str(n) + r).encode()
                xh = tarfile.TarInfo("PaxHeaders/sparse-file")
                xh.type = tarfile.XHDTYPE
                xh.size = len(body)
                tf.fileobj.write(xh.tobuf(tarfile.USTAR_FORMAT))
                tf.fileobj.write(pad(body))
                tf.offset += 512 + len(pad(body))
                ti.pax_headers = {}
                tf.fileobj.write(ti.tobuf(tarfile.USTAR_FORMAT))
                tf.fileobj.write(pad(payload))
                tf.offset += 512 + len(pad(payload))
            else:
                tf.addfile(ti, io.BytesIO(payload))
        paths["pax" + ver] = p
    return paths

"""Runs inside the kext2-tools container (privileged). For each image: e2fsck
-fn, dumpe2fs -h, then a read-only kernel loop mount and a walk of the tree.
Prints one JSON object {image: report}.
"""

import hashlib
import json
import os
import stat
import subprocess
import sys

TYPES = [
    (stat.S_ISREG, "f"),
    (stat.S_ISDIR, "d"),
    (stat.S_ISLNK, "l"),
    (stat.S_ISCHR, "c"),
    (stat.S_ISBLK, "b"),
    (stat.S_ISFIFO, "p"),
    (stat.S_ISSOCK, "s"),
]


def ftype(mode):
    for pred, t in TYPES:
        if pred(mode):
            return t
    return "?"


def digest(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def superblock(img):
    p = subprocess.run(["dumpe2fs", "-h", img], capture_output=True, text=True)
    sb = {}
    for line in p.stdout.splitlines():
        if ":" in line:
            k, v = line.split(":", 1)
            sb[k.strip()] = v.strip()
    return sb


def walk(root, hash_data):
    entries, inodes = {}, {}
    for dirpath, dirnames, filenames in os.walk(root):
        dirnames.sort()
        names = sorted(dirnames + filenames)
        if dirpath == root:
            names = [None] + names
        for name in names:
            full = dirpath if name is None else os.path.join(dirpath, name)
            rel = "/" if name is None else "/" + os.path.relpath(full, root)
            st = os.lstat(full)
            t = ftype(st.st_mode)
            e = {
                "type": t,
                "mode": stat.S_IMODE(st.st_mode),
                "uid": st.st_uid,
                "gid": st.st_gid,
                "mtime": int(st.st_mtime),
                "atime": int(st.st_atime),
                "ctime": int(st.st_ctime),
                "nlink": st.st_nlink,
                "ino": st.st_ino,
                "blocks": st.st_blocks,
                "size": st.st_size,
            }
            if t == "f" and hash_data:
                e["sha256"] = digest(full)
            if t == "l":
                e["target"] = os.readlink(full)
            if t in "cb":
                e["rdev"] = [os.major(st.st_rdev), os.minor(st.st_rdev)]
            if t != "d" and st.st_nlink > 1:
                inodes.setdefault(st.st_ino, []).append(rel)
            entries[rel] = e
    # Hard links: name each group by its smallest path.
    for paths in inodes.values():
        first = min(paths)
        for p in paths:
            entries[p]["link"] = first
    for e in entries.values():
        del e["ino"]
    return entries


def inspect(img, hash_data):
    rep = {}
    p = subprocess.run(["e2fsck", "-fn", img], capture_output=True, text=True)
    rep["fsck_rc"] = p.returncode
    rep["fsck_out"] = (p.stdout + p.stderr)[-3000:]
    rep["sb"] = superblock(img)
    mnt = "/mnt/x"
    os.makedirs(mnt, exist_ok=True)
    m = subprocess.run(["mount", "-t", "ext2", "-o", "loop,ro", img, mnt], capture_output=True, text=True)
    rep["mount_ok"] = m.returncode == 0
    rep["mount_err"] = m.stderr[-2000:]
    if rep["mount_ok"]:
        try:
            rep["entries"] = walk(mnt, hash_data)
        except Exception as ex:  # report, don't crash the batch
            rep["walk_error"] = repr(ex)
        finally:
            subprocess.run(["umount", mnt])
    k = subprocess.run(["dmesg"], capture_output=True, text=True)
    rep["kernel_errors"] = [
        l for l in k.stdout.splitlines()[-50:] if "EXT" in l and ("error" in l.lower() or "warn" in l.lower())
    ]
    return rep


def main():
    args = sys.argv[1:]
    hash_data = True
    if args and args[0] == "--no-hash":
        hash_data = False
        args = args[1:]
    print(json.dumps({img: inspect(img, hash_data) for img in args}))


if __name__ == "__main__":
    main()

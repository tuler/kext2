"""Shared helpers for the black-box comparison harness.

The harness only runs the image builders and inspects the images they write
(through the Linux kernel, e2fsck and dumpe2fs in a container). It never looks
inside the builders.
"""

import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.dirname(HERE)
WORK = os.environ.get("KEXT2_WORK", os.path.join(HERE, "work"))
DOCKER_IMAGE = os.environ.get("KEXT2_TOOLS_IMAGE", "kext2-tools")

TOOLS = {
    "xgenext2fs": os.environ.get("XGENEXT2FS", shutil.which("xgenext2fs") or "/opt/homebrew/bin/xgenext2fs"),
    "mkext2": os.environ.get(
        "MKEXT2", os.path.expanduser("~/Documents/workspace/personal/mkext2/target/release/mkext2")
    ),
    "kext2": os.environ.get("KEXT2", os.path.join(REPO, "target", "release", "kext2")),
}

# Options only the newer tools understand.
EXTENSIONS = ("--timestamp", "--clamp-timestamps")


def available_tools(names=None):
    names = names or list(TOOLS)
    return [n for n in names if os.access(TOOLS[n], os.X_OK)]


def supports(tool, args):
    if tool == "xgenext2fs":
        return not any(a.split("=")[0] in EXTENSIONS for a in args)
    return True


def sha256_file(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def run_tool(tool, args, image, env=None, cwd=None, measure=False):
    """Run a builder. Returns dict(rc, seconds, rss, footprint, stderr): peak
    resident set size and, on macOS, peak memory footprint (bytes)."""
    cmd = [TOOLS[tool]] + list(args) + [image]
    if measure and sys.platform == "darwin":
        cmd = ["/usr/bin/time", "-l"] + cmd
    full_env = dict(os.environ)
    full_env.pop("SOURCE_DATE_EPOCH", None)
    if env:
        full_env.update(env)
    t0 = time.perf_counter()
    p = subprocess.run(cmd, capture_output=True, env=full_env, cwd=cwd)
    secs = time.perf_counter() - t0
    err = p.stderr.decode(errors="replace")
    rss = foot = None
    m = re.search(r"(\d+)\s+maximum resident set size", err)
    if m:
        rss = int(m.group(1))
    m = re.search(r"(\d+)\s+peak memory footprint", err)
    if m:
        foot = int(m.group(1))
    return {"rc": p.returncode, "seconds": secs, "rss": rss, "footprint": foot, "stderr": err, "cmd": cmd}


def inspect_images(images, hash_data=True):
    """Inspect images in the tools container. images: list of host paths.

    Returns {host_path: report}. Mounts read-only through the kernel, walks the
    tree, runs e2fsck -fn and dumpe2fs -h.
    """
    if not images:
        return {}
    images = [os.path.abspath(p) for p in images]
    mounts, args = [], []
    for i, img in enumerate(images):
        d = os.path.dirname(img)
        mounts += ["-v", f"{d}:/img{i}:ro"]
        args.append(f"/img{i}/{os.path.basename(img)}")
    cmd = (
        ["docker", "run", "--rm", "--privileged", "-v", f"{HERE}:/harness:ro"]
        + mounts
        + [DOCKER_IMAGE, "python3", "/harness/inspect_images.py"]
        + ([] if hash_data else ["--no-hash"])
        + args
    )
    p = subprocess.run(cmd, capture_output=True)
    if p.returncode != 0:
        raise RuntimeError("inspect failed: " + p.stderr.decode(errors="replace")[-4000:])
    out = json.loads(p.stdout)
    return {images[i]: out[a] for i, a in enumerate(args)}


# Fields of a manifest entry compared by default. Directory sizes, inode
# numbers, atime and ctime are layout/policy and reported separately.
CONTENT_FIELDS = ("type", "mode", "uid", "gid", "size", "sha256", "target", "rdev", "mtime", "nlink", "link")


def compare_manifests(a, b, fields=CONTENT_FIELDS, ignore=()):
    """Return a list of human-readable differences between two manifests."""
    diffs = []
    for path in sorted(set(a) | set(b)):
        if any(path == p or path.startswith(p + "/") for p in ignore):
            continue
        if path not in a:
            diffs.append(f"{ascii(path)}: only in second")
            continue
        if path not in b:
            diffs.append(f"{ascii(path)}: only in first")
            continue
        ea, eb = a[path], b[path]
        for f in fields:
            if f == "size" and ea.get("type") == "d":
                continue
            if ea.get(f) != eb.get(f):
                diffs.append(f"{ascii(path)}: {f} {ea.get(f)!r} != {eb.get(f)!r}")
    return diffs


def rmtree(path):
    if os.path.lexists(path):
        subprocess.run(["chmod", "-R", "u+rwx", path], capture_output=True)
        shutil.rmtree(path, ignore_errors=True)

#!/usr/bin/env python3
"""Memory comparison of kext2, mkext2 and xgenext2fs.

    python3 compare/memory.py [--sizes 64,256,1024] [--files 10000,100000]
                              [--empty 1,4,16] [--out compare/MEMORY.md]

For each case it runs every tool once and records wall time and peak memory:
on macOS the maximum resident set size and the peak memory footprint
(`/usr/bin/time -l`; the footprint leaves out clean file-backed pages such as
mapped or cached input files), on Linux the maximum resident set size
(`/usr/bin/time -v`). The cases separate what memory grows with:

- data size: root-filesystem-shaped trees of a few sizes (compare/gen.py);
- file count: many 1 KiB files, where per-file metadata dominates;
- image size: empty images of several sizes;
- kext2 writer threads: the same build with KEXT2_THREADS=1 and the default.
"""

import argparse
import os
import platform
import re
import subprocess
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from common import TOOLS, WORK, available_tools, rmtree, supports, tool_args
import gen


def measured(cmd, env=None):
    """Run cmd; return (rc, seconds, max_rss_bytes, footprint_bytes or None)."""
    if sys.platform == "darwin":
        wrapped = ["/usr/bin/time", "-l"] + cmd
    else:
        wrapped = ["/usr/bin/time", "-v"] + cmd
    full_env = dict(os.environ)
    full_env.pop("SOURCE_DATE_EPOCH", None)
    full_env.update(env or {})
    t0 = time.perf_counter()
    p = subprocess.run(wrapped, capture_output=True, env=full_env)
    secs = time.perf_counter() - t0
    err = p.stderr.decode(errors="replace")
    rss = foot = None
    if sys.platform == "darwin":
        m = re.search(r"(\d+)\s+maximum resident set size", err)
        rss = int(m.group(1)) if m else None
        m = re.search(r"(\d+)\s+peak memory footprint", err)
        foot = int(m.group(1)) if m else None
    else:
        m = re.search(r"Maximum resident set size \(kbytes\): (\d+)", err)
        rss = int(m.group(1)) * 1024 if m else None
    return p.returncode, secs, rss, foot, err


def many_files(root, n):
    rmtree(root)
    per_dir = 1000
    for i in range(n):
        d = os.path.join(root, f"d{i // per_dir:04d}")
        if i % per_dir == 0:
            os.makedirs(d)
        with open(os.path.join(d, f"f{i:07d}"), "wb") as f:
            f.write(b"%07d" % i * 128)  # 1 KiB


def mib(b):
    return "—" if b is None else f"{b / 2**20:,.1f}"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--sizes", default="64,256,1024", help="dataset sizes in MiB")
    ap.add_argument("--files", default="10000,100000", help="file counts for the many-small-files case")
    ap.add_argument("--empty", default="1,4,16", help="empty image sizes in GiB")
    ap.add_argument("--tools", default="xgenext2fs,mkext2,kext2")
    ap.add_argument("--out", default=None)
    args = ap.parse_args()
    tools = available_tools(args.tools.split(","))
    base = os.path.join(WORK, "memory")
    os.makedirs(base, exist_ok=True)
    img = os.path.join(base, "out.img")

    cases = []  # (group, label, args, env per tool)
    for s in [int(x) for x in args.sizes.split(",") if x]:
        d = os.path.join(WORK, f"bench-{s}", "rootfs")
        stamp = os.path.join(WORK, f"bench-{s}", ".ready")
        if not os.path.exists(stamp):
            print(f"generating {s} MiB dataset ...", flush=True)
            os.makedirs(os.path.dirname(d), exist_ok=True)
            _, n = gen.gen_rootfs(d, s)
            open(stamp, "w").write(str(n))
        n = open(stamp).read().strip()
        cases.append(("data size", f"{s} MiB tree ({n} files), 4 KiB blocks", ["-B", "4096", "-f", "-d", d]))
        tar = os.path.join(WORK, f"bench-{s}", "rootfs.tar")
        if s == max(int(x) for x in args.sizes.split(",")):
            if not os.path.exists(tar):
                gen.make_tar(d, tar, "gnu")
            cases.append(("data size", f"{s} MiB tar, 4 KiB blocks", ["-B", "4096", "-f", "-a", tar]))
            cases.append(("data size", f"{s} MiB tree, 1 KiB blocks", ["-B", "1024", "-f", "-d", d]))
    for n in [int(x) for x in args.files.split(",") if x]:
        d = os.path.join(WORK, f"files-{n}")
        if not os.path.exists(os.path.join(d, ".ready")):
            print(f"generating {n} files ...", flush=True)
            many_files(d, n)
            open(os.path.join(d, ".ready"), "w").close()
        cases.append(("file count", f"{n:,} files of 1 KiB, 4 KiB blocks", ["-B", "4096", "-f", "-d", d]))
    for g in [int(x) for x in args.empty.split(",") if x]:
        cases.append(("image size", f"empty {g} GiB image", ["-B", "4096", "-f", "-b", str(g << 18)]))
    # kext2 sizes images from the content: an empty image is free space only
    alt = {f"empty {g} GiB image": {"kext2": ["-f", "--free-space", f"{g}G"]}
           for g in [int(x) for x in args.empty.split(",") if x]}

    rows = []
    for group, label, cargs in cases:
        row = {"group": group, "label": label, "tools": {}}
        variants = [(t, {}) for t in tools]
        if "kext2" in tools:
            variants.append(("kext2 (1 thread)", {"KEXT2_THREADS": "1"}))
        for name, env in variants:
            tool = name.split(" ")[0]
            targs = alt.get(label, {}).get(tool, cargs)
            if not supports(tool, targs):
                continue
            if os.path.exists(img):
                os.remove(img)
            rc, secs, rss, foot, err = measured([TOOLS[tool]] + tool_args(tool, targs) + [img], env)
            row["tools"][name] = (rc, secs, rss, foot)
            print(f"  {label:42} {name:18} rc={rc} {secs:7.2f}s  rss={mib(rss):>9} MiB  footprint={mib(foot):>9} MiB",
                  flush=True)
            if rc:
                print("    " + err.strip().splitlines()[-1][:200])
        rows.append(row)
    if os.path.exists(img):
        os.remove(img)

    names = list(rows[0]["tools"]) if rows else []
    darwin = sys.platform == "darwin"
    metric = "peak memory footprint" if darwin else "maximum resident set size"
    lines = [
        f"Host: {platform.system()} {platform.machine()}; one run per cell; MiB of {metric}"
        + (" (maximum RSS in parentheses)" if darwin else "") + ".",
        "",
        "| what grows | case | " + " | ".join(names) + " |",
        "|---|---|" + "---:|" * len(names),
    ]
    for row in rows:
        cells = []
        for n in names:
            rc, secs, rss, foot = row["tools"][n]
            if rc:
                cells.append("fails")
            elif darwin:
                cells.append(f"{mib(foot)} ({mib(rss)})")
            else:
                cells.append(mib(rss))
        lines.append(f"| {row['group']} | {row['label']} | " + " | ".join(cells) + " |")
    lines += ["", "Wall time (s):", "", "| case | " + " | ".join(names) + " |", "|---|" + "---:|" * len(names)]
    for row in rows:
        lines.append(f"| {row['label']} | " + " | ".join(
            "fails" if row["tools"][n][0] else f"{row['tools'][n][1]:.2f}" for n in names) + " |")
    text = "\n".join(lines)
    print("\n" + text)
    if args.out:
        with open(args.out, "w") as f:
            f.write(text + "\n")


if __name__ == "__main__":
    main()

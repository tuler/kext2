#!/usr/bin/env python3
"""Image size and structure comparison of kext2, mkext2 and xgenext2fs.

    python3 compare/layout.py [--out compare/LAYOUT.md]

Builds the same inputs with every tool (automatic size, `-f`) and analyzes
the images (compare/analyze_image.py in the kext2-tools container): where the
blocks go, the geometry, how many inodes there are and how they are used, and
how inodes and data are laid out.
"""

import argparse
import json
import os
import subprocess
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from common import DOCKER_IMAGE, HERE, WORK, available_tools, run_tool
import gen
from memory import many_files

FEATURES = [("compat", 0x08, "ext_attr"), ("compat", 0x10, "resize_inode"), ("compat", 0x20, "dir_index"),
            ("incompat", 0x02, "filetype"), ("ro_compat", 0x01, "sparse_super"), ("ro_compat", 0x02, "large_file")]


def features(f):
    names = [n for k, bit, n in FEATURES if f[k] & bit]
    return " ".join(names) or "none"


def analyze(images):
    d = os.path.dirname(images[0])
    cmd = ["docker", "run", "--rm", "-v", f"{HERE}:/harness:ro", "-v", f"{d}:/img:ro", DOCKER_IMAGE,
           "python3", "/harness/analyze_image.py"] + [f"/img/{os.path.basename(p)}" for p in images]
    p = subprocess.run(cmd, capture_output=True, check=True)
    out = json.loads(p.stdout)
    return {p: out[f"/img/{os.path.basename(p)}"] for p in images}


def mib(blocks, bs):
    return f"{blocks * bs / 2**20:,.1f}"


def pct(x):
    return f"{100 * x:.0f}%"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", default=None)
    ap.add_argument("--tools", default="xgenext2fs,mkext2,kext2")
    args = ap.parse_args()
    tools = available_tools(args.tools.split(","))
    base = os.path.join(WORK, "layout")
    os.makedirs(base, exist_ok=True)

    rootfs = os.path.join(WORK, "bench-1024", "rootfs")
    if not os.path.exists(os.path.join(WORK, "bench-1024", ".ready")):
        os.makedirs(os.path.dirname(rootfs), exist_ok=True)
        _, n = gen.gen_rootfs(rootfs, 1024)
        open(os.path.join(WORK, "bench-1024", ".ready"), "w").write(str(n))
    small = os.path.join(WORK, "files-100000")
    if not os.path.exists(os.path.join(small, ".ready")):
        many_files(small, 100_000)
        open(os.path.join(small, ".ready"), "w").close()
    mixed = os.path.join(WORK, "inputs", "tree1024")
    wide = os.path.join(WORK, "inputs", "wide")
    if not (os.path.exists(mixed) and os.path.exists(wide)):
        sys.exit("run compare/run.py once first (it generates compare/work/inputs)")

    cases = [
        ("1 GiB root filesystem tree, 4 KiB blocks", ["-B", "4096", "-f", "-d", rootfs]),
        ("1 GiB root filesystem tree, 1 KiB blocks", ["-B", "1024", "-f", "-d", rootfs]),
        ("100,000 files of 1 KiB, 4 KiB blocks", ["-B", "4096", "-f", "-d", small]),
        ("mixed test tree (links, sparse sizes), 1 KiB blocks", ["-B", "1024", "-f", "-d", mixed]),
        ("one directory of 3,000 entries, 1 KiB blocks", ["-B", "1024", "-f", "-d", wide]),
    ]
    reports = []
    for label, cargs in cases:
        imgs = {}
        for t in tools:
            img = os.path.join(base, f"{len(reports)}.{t}.img")
            if os.path.exists(img):
                os.remove(img)
            r = run_tool(t, cargs, img)
            if r["rc"] == 0:
                imgs[t] = img
            else:
                print(f"{label} [{t}] failed: {r['stderr'][-200:]}")
        print(f"analyzing {label} ...", flush=True)
        res = analyze(list(imgs.values()))
        reports.append((label, {t: res[p] for t, p in imgs.items()}))
        for p in imgs.values():
            os.remove(p)

    L = []
    for label, rep in reports:
        names = list(rep)
        L += [f"## {label}", ""]
        first = rep[names[0]]
        fb = first["content"]["file_bytes"]
        L += [f"Content: {first['content']['files']:,} regular files, {fb / 2**20:,.1f} MiB of file data.", ""]
        L += ["| | " + " | ".join(names) + " |", "|---|" + "---:|" * len(names)]

        def row(title, f):
            L.append(f"| {title} | " + " | ".join(f(rep[n]) for n in names) + " |")

        bsz = lambda r: r["geometry"]["block_size"]
        row("**image size (MiB)**", lambda r: f"**{mib(r['geometry']['blocks'], bsz(r))}**")
        row("image size / file data", lambda r: f"{r['geometry']['blocks'] * bsz(r) / max(1, fb):.2f}×")
        row("blocks", lambda r: f"{r['geometry']['blocks']:,}")
        L.append("| *where the blocks go* |" + " |" * len(names))
        for key, title in [("boot_block", "boot block"), ("superblocks_and_descriptors", "superblock + descriptor copies"),
                           ("bitmaps", "block + inode bitmaps"), ("inode_tables", "inode tables"),
                           ("file_data", "file data"), ("indirect", "indirect blocks"),
                           ("directories", "directory blocks"), ("symlinks", "symlink blocks"),
                           ("free", "free (incl. reserved)"), ("unaccounted", "other")]:
            if all(rep[n]["blocks"][key] == 0 for n in names):
                continue
            row(title, lambda r, k=key: f"{r['blocks'][k]:,}")
        row("reserved for root (`-m`)", lambda r: f"{r['geometry']['reserved_blocks']:,}")
        row("free beyond reserved", lambda r: f"{r['blocks']['free'] - r['geometry']['reserved_blocks']:,}")
        L.append("| *geometry* |" + " |" * len(names))
        row("block groups", lambda r: f"{r['geometry']['groups']}")
        row("blocks per group", lambda r: f"{r['geometry']['blocks_per_group']:,}")
        row("groups with a superblock copy", lambda r: f"{r['geometry']['groups_with_superblock']}")
        row("inode size (bytes)", lambda r: f"{r['geometry']['inode_size']}")
        row("inodes per group", lambda r: f"{r['geometry']['inodes_per_group']:,}")
        row("inode table blocks per group", lambda r: f"{r['geometry']['inode_table_blocks_per_group']:,}")
        row("features", lambda r: features(r["geometry"]["features"]))
        L.append("| *inodes* |" + " |" * len(names))
        row("inodes (total)", lambda r: f"{r['inodes']['total']:,}")
        row("inodes used", lambda r: f"{r['inodes']['used']:,}")
        row("inodes free", lambda r: f"{r['inodes']['total'] - r['inodes']['used']:,}")
        L.append("| *layout* |" + " |" * len(names))
        row("groups holding inodes", lambda r: f"{r['layout']['groups_with_used_inodes']}")
        row("groups holding top-level dirs", lambda r: f"{r['layout']['groups_holding_top_level_dirs']}")
        row("subdirs in parent's group", lambda r: pct(r["layout"]["dirs_in_parent_group"]))
        row("files in parent dir's group", lambda r: pct(r["layout"]["files_in_parent_group"]))
        row("file data in inode's group", lambda r: pct(r["layout"]["files_data_in_inode_group"]))
        row("contiguous files", lambda r: pct(r["layout"]["files_contiguous"]))
        row("runs per file (avg)", lambda r: f"{r['layout']['runs_per_file']:.2f}")
        row("inode numbers follow sorted walk", lambda r: pct(r["layout"]["numbering_follows_sorted_walk"]))
        row("dirs with name-sorted entries", lambda r: pct(r["layout"]["dirs_with_sorted_entries"]))
        L.append("")
    text = "\n".join(L)
    print("\n" + text)
    with open(os.path.join(base, "layout.json"), "w") as f:
        json.dump(reports, f, indent=1)
    if args.out:
        with open(args.out, "w") as f:
            f.write(text + "\n")


if __name__ == "__main__":
    main()

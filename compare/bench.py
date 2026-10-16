#!/usr/bin/env python3
"""Performance comparison of kext2, mkext2 and xgenext2fs.

    python3 compare/bench.py [MiB] [--runs N] [--tools ...]

Generates a root-filesystem-shaped dataset of about MiB MiB, times each tool
(best of N wall-clock runs, peak RSS), and checks that every image is valid
(e2fsck -fn, kernel mount) and holds the same content as xgenext2fs' image.
"""

import argparse
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from common import WORK, available_tools, compare_manifests, inspect_images, run_tool, supports
import gen


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("mib", nargs="?", type=int, default=1024)
    ap.add_argument("--runs", type=int, default=3)
    ap.add_argument("--tools", default="xgenext2fs,mkext2,kext2")
    ap.add_argument("--skip", default="", help="comma-separated case names to skip")
    ap.add_argument("--out", default=None, help="write a markdown table here")
    args = ap.parse_args()
    tools = available_tools(args.tools.split(","))
    base = os.path.join(WORK, f"bench-{args.mib}")
    data = os.path.join(base, "rootfs")
    stamp = os.path.join(base, ".ready")
    if not os.path.exists(stamp):
        print(f"generating {args.mib} MiB dataset ...", flush=True)
        os.makedirs(base, exist_ok=True)
        _, nfiles = gen.gen_rootfs(data, args.mib)
        gen.make_tar(data, os.path.join(base, "rootfs.tar"), "gnu")
        gen.make_tar(data, os.path.join(base, "rootfs.tar.gz"), "gnu", compress="gz")
        open(stamp, "w").write(str(nfiles))
    nfiles = open(stamp).read()
    blocks_empty = str((args.mib << 20) // 4096)
    cases = [
        ("directory (4 KiB blocks)", ["-B", "4096", "-f", "-d", data]),
        ("directory (1 KiB blocks)", ["-B", "1024", "-f", "-d", data]),
        ("tar (4 KiB blocks)", ["-B", "4096", "-f", "-a", os.path.join(base, "rootfs.tar")]),
        ("tar.gz (4 KiB blocks)", ["-B", "4096", "-f", "-a", os.path.join(base, "rootfs.tar.gz")]),
        (f"empty {args.mib} MiB image", ["-B", "4096", "-f", "-b", blocks_empty]),
    ]
    skip = set(s for s in args.skip.split(",") if s)
    results = []
    for name, cargs in cases:
        if name in skip:
            continue
        row = {"case": name, "tools": {}}
        for tool in tools:
            if not supports(tool, cargs):
                continue
            img = os.path.join(base, f"{tool}.{len(results)}.img")
            best = None
            for _ in range(args.runs):
                if os.path.exists(img):
                    os.remove(img)
                r = run_tool(tool, cargs, img, measure=True)
                if r["rc"] != 0:
                    best = r
                    break
                if best is None or r["seconds"] < best["seconds"]:
                    best = r
            row["tools"][tool] = {"seconds": best["seconds"], "rss": best["rss"], "rc": best["rc"],
                                  "img": img if best["rc"] == 0 else None, "stderr": best["stderr"][-300:]}
            print(f"  {name:28} {tool:12} {best['seconds']:8.2f}s  rss={(best['rss'] or 0) / 2**20:7.1f} MiB  rc={best['rc']}",
                  flush=True)
        imgs = [t["img"] for t in row["tools"].values() if t["img"]]
        reps = inspect_images(imgs)
        ref = row["tools"].get("xgenext2fs")
        ref_entries = reps[os.path.abspath(ref["img"])].get("entries") if ref and ref["img"] else None
        for tool, t in row["tools"].items():
            if not t["img"]:
                t["valid"], t["same"] = False, None
                continue
            rep = reps[os.path.abspath(t["img"])]
            t["valid"] = rep["fsck_rc"] == 0 and rep["mount_ok"]
            t["blocks"] = int(rep["sb"].get("Block count", 0))
            t["same"] = None if ref_entries is None else not compare_manifests(ref_entries, rep.get("entries") or {})
            os.remove(t["img"])
        results.append(row)

    lines = [f"Dataset: {args.mib} MiB, {nfiles} files (compare/gen.py gen_rootfs). Best of {args.runs} runs.", "",
             "| case | " + " | ".join(tools) + " | " + " | ".join(f"kext2 vs {t}" for t in tools if t != "kext2") +
             " | valid | same content |",
             "|---|" + "---:|" * len(tools) + "---:|" * (len(tools) - 1) + "---|---|"]
    for row in results:
        ts = row["tools"]
        cells = []
        for t in tools:
            v = ts.get(t)
            cells.append("—" if v is None or v["rc"] else f"{v['seconds']:.2f} s")
        sp = []
        k = ts.get("kext2")
        for t in tools:
            if t == "kext2":
                continue
            v = ts.get(t)
            sp.append(f"{v['seconds'] / k['seconds']:.1f}×" if k and v and not k["rc"] and not v["rc"] else "—")
        valid = "/".join("y" if ts[t].get("valid") else "n" for t in tools if t in ts)
        same = "/".join({True: "y", False: "n", None: "-"}[ts[t].get("same")] for t in tools if t in ts)
        lines.append(f"| {row['case']} | " + " | ".join(cells) + " | " + " | ".join(sp) + f" | {valid} | {same} |")
    lines += ["", "Peak RSS (MiB): " + "; ".join(
        f"{row['case']}: " + ", ".join(f"{t}={(v['rss'] or 0) / 2**20:.0f}" for t, v in row["tools"].items())
        for row in results)]
    text = "\n".join(lines)
    print("\n" + text)
    if args.out:
        with open(args.out, "w") as f:
            f.write(text + "\n")


if __name__ == "__main__":
    main()

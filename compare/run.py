#!/usr/bin/env python3
"""Black-box correctness and reproducibility comparison of kext2, mkext2 and
xgenext2fs.

    python3 compare/run.py [--only SUBSTR] [--tools kext2,mkext2,xgenext2fs] [--seeds N]

Every image is checked with `e2fsck -fn` and a read-only Linux kernel mount;
content (types, modes, owners, sizes, data, symlink targets, device numbers,
hard links, mtimes) is compared with the reference: xgenext2fs when it accepts
the command line, mkext2 otherwise.
"""

import argparse
import json
import os
import shutil
import stat
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from common import (WORK, available_tools, compare_manifests, inspect_images, rmtree, run_tool, sha256_file,
                    supports)
import gen

INPUTS = os.path.join(WORK, "inputs")


def inp(name):
    return os.path.join(INPUTS, name)


def prepare_inputs(seeds):
    os.makedirs(INPUTS, exist_ok=True)
    stamp = os.path.join(INPUTS, f".ready-{seeds}")
    if os.path.exists(stamp):
        return
    for bs in (1024, 2048, 4096):
        gen.gen_tree(inp(f"tree{bs}"), seed=bs, bs=bs)
    gen.gen_tree(inp("big1024"), seed=7, nfiles=20, ndirs=3, bs=1024, big=True)
    gen.gen_tree(inp("big4096"), seed=8, nfiles=20, ndirs=3, bs=4096, big=True)
    gen.gen_tree(inp("sparse"), seed=9, nfiles=20, ndirs=3, bs=1024, big=True, sparse=True)
    gen.gen_tree(inp("wide"), seed=10, nfiles=0, ndirs=0, long_names=False, fifos=False)
    for i in range(3000):  # one huge directory
        with open(os.path.join(inp("wide"), f"entry-{i:05d}-" + "w" * (i % 40)), "wb") as f:
            f.write(b"%d" % i)
    gen.gen_tree(inp("many"), seed=11, nfiles=2500, ndirs=400, bs=1024)  # many groups with 1 KiB blocks
    gen.gen_tree(inp("layer2"), seed=12, nfiles=30, ndirs=5)
    gen.gen_tree(inp("ustar-src"), seed=13, nfiles=60, ndirs=8, long_names=False, max_name=8, symlink_max=90)
    for s in range(seeds):
        gen.gen_tree(inp(f"rand{s}"), seed=1000 + s, nfiles=40 + 20 * s, ndirs=4 + s, bs=1024)
    t = inp("tree1024")
    gen.make_tar(t, inp("tree.gnu.tar"), "gnu")
    gen.make_tar(t, inp("tree.pax.tar"), "pax")
    gen.make_tar(inp("ustar-src"), inp("tree.ustar.tar"), "ustar")
    for c in ("gz", "bz2", "xz", "zst"):
        gen.make_tar(t, inp(f"tree.tar.{c}"), "pax", compress=c)
    gen.make_tar_root_owned(inp("layer2"), inp("rootowned.tar"))
    gen.make_tar_root_owned(inp("layer2"), inp("bigids.tar"), big_ids=True)
    gen.make_devtable(inp("devtable"))
    gen.make_sparse_tars(INPUTS)
    open(stamp, "w").close()


# xgenext2fs on macOS (libarchive) stores pax UTF-8 names decomposed (NFD);
# the archive holds NFC bytes, so mkext2 is the reference for those cases.
PAX_UTF8 = {"ref": "mkext2"}


def cases(seeds):
    """(name, args, expect_ok, ignore_paths, fields_override)"""
    c = []
    for bs in (1024, 2048, 4096):
        c.append((f"dir-B{bs}", ["-B", str(bs), "-f", "-d", inp(f"tree{bs}")], True))
        c.append((f"dir-B{bs}-times", ["-B", str(bs), "-d", inp(f"tree{bs}")], True,
                  {"env": {"SOURCE_DATE_EPOCH": "1700000000"}}))
    c += [
        ("big-1k-triple-indirect", ["-B", "1024", "-f", "-d", inp("big1024")], True),
        ("big-4k-double-indirect", ["-B", "4096", "-f", "-d", inp("big4096")], True),
        ("sparse-holes-z", ["-B", "1024", "-f", "-z", "-d", inp("sparse")], True),
        ("sparse-noholes", ["-B", "4096", "-f", "-d", inp("sparse")], True),
        ("wide-dir-1k", ["-B", "1024", "-f", "-d", inp("wide")], True),
        ("wide-dir-4k", ["-B", "4096", "-f", "-d", inp("wide")], True),
        ("many-groups-1k", ["-B", "1024", "-f", "-d", inp("many")], True),
        ("tar-gnu", ["-B", "4096", "-f", "-a", inp("tree.gnu.tar")], True),
        ("tar-pax", ["-B", "4096", "-f", "-a", inp("tree.pax.tar")], True, PAX_UTF8),
        ("tar-ustar", ["-B", "1024", "-f", "-a", inp("tree.ustar.tar")], True),
        ("tar-gz", ["-B", "4096", "-f", "-a", inp("tree.tar.gz")], True, PAX_UTF8),
        ("tar-bz2", ["-B", "4096", "-f", "-a", inp("tree.tar.bz2")], True, PAX_UTF8),
        ("tar-xz", ["-B", "4096", "-f", "-a", inp("tree.tar.xz")], True, PAX_UTF8),
        ("tar-zst", ["-B", "4096", "-f", "-a", inp("tree.tar.zst")], True, PAX_UTF8),
        ("tar-devices-root", ["-B", "1024", "-f", "-a", inp("rootowned.tar")], True),
        ("devtable", ["-B", "1024", "-f", "-b", "4096", "-D", inp("devtable")], True),
        ("devtable-autosize", ["-B", "1024", "-f", "-D", inp("devtable")], True,
         {"ref": "mkext2", "xfail": ["xgenext2fs"]}),  # xgenext2fs: "no free space"
        ("dir+devtable", ["-B", "4096", "-f", "-d", inp("layer2"), "-D", inp("devtable")], True),
        ("layers-dir-tar", ["-B", "4096", "-f", "-d", inp("layer2"), "-a", inp("tree.gnu.tar")], True),
        ("empty-25600", ["-B", "4096", "-f", "-b", "25600"], True),
        ("empty-1k-default", ["-f", "-b", "8192"], True),
        ("size-explicit", ["-B", "1024", "-f", "-b", "60000", "-d", inp("tree1024")], True),
        ("readjust+1000", ["-B", "4096", "-f", "-r", "+1000", "-d", inp("tree4096")], True),
        ("inodes-N5000", ["-B", "1024", "-f", "-N", "5000", "-d", inp("tree1024")], True),
        ("bytes-per-inode", ["-B", "1024", "-f", "-i", "2048", "-d", inp("tree1024")], True),
        ("reserved-0", ["-B", "1024", "-f", "-m", "0", "-d", inp("tree1024")], True),
        ("reserved-20", ["-B", "1024", "-f", "-m", "20", "-d", inp("tree1024")], True),
        ("label", ["-B", "1024", "-f", "-L", "my-volume", "-d", inp("tree1024")], True),
        ("creator-hurd", ["-B", "1024", "-f", "-o", "hurd", "-d", inp("layer2")], True),
        ("creator-freebsd", ["-B", "1024", "-f", "-o", "freebsd", "-d", inp("layer2")], True),
        ("squash-U-P", ["-B", "1024", "-f", "-U", "1000", "-P", "-d", inp("tree1024")], True),
        ("squash-q", ["-B", "1024", "-f", "--squash=0", "-d", inp("tree1024")], True),
        ("readjust-absolute", ["-B", "1024", "-f", "-r", "40000", "-d", inp("tree1024")], True),
        ("fill-value", ["-B", "1024", "-f", "-e", "170", "-b", "20000", "-d", inp("layer2")], True),
        ("timestamp", ["-B", "4096", "--timestamp", "1700000000", "-d", inp("tree4096")], True),
        ("clamp", ["-B", "4096", "--clamp-timestamps", "1650000000", "-d", inp("tree4096")], True),
        # xgenext2fs and mkext2 silently raise a too-small -b; kext2 fails (SPEC R-SIZE-5)
        ("too-small-b", ["-B", "1024", "-f", "-b", "100", "-d", inp("tree1024")], True, {"expect": {"kext2": False}}),
        ("missing-input", ["-B", "1024", "-f", "-d", inp("does-not-exist")], False),
        ("bad-block-size", ["-B", "3000", "-f", "-b", "1000"], False),
    ]
    for s in range(seeds):
        bs = (1024, 2048, 4096)[s % 3]
        c.append((f"random-{s}", ["-B", str(bs), "-f", "-d", inp(f"rand{s}")], True))
    # sparse tar members (added by the kext2 implementer: GNU 'S', pax 0.0/0.1/1.0)
    for fmt in ("oldgnu", "pax0.0", "pax0.1", "pax1.0"):
        c.append((f"tar-sparse-{fmt}", ["-B", "1024", "-f", "-a", inp(f"sparse.{fmt}.tar")], True))
        c.append((f"tar-sparse-{fmt}-z", ["-B", "4096", "-f", "-z", "-a", inp(f"sparse.{fmt}.tar")], True))
    return c


def copy_reversed(src, dst):
    """Copy a tree creating entries in reverse order (different inode numbers,
    directory order, creation order), preserving modes, times and hard links."""
    rmtree(dst)
    os.makedirs(dst)
    seen = {}
    entries = []
    for dp, dn, fn in os.walk(src):
        rel = os.path.relpath(dp, src)
        for n in dn:
            if os.path.islink(os.path.join(dp, n)):
                entries.append(os.path.join(rel, n))
            else:
                os.makedirs(os.path.join(dst, rel, n), exist_ok=True)
        for n in fn:
            entries.append(os.path.join(rel, n))
    for rel in sorted(entries, reverse=True):
        s, d = os.path.join(src, rel), os.path.join(dst, rel)
        st = os.lstat(s)
        key = (st.st_dev, st.st_ino)
        if stat.S_ISLNK(st.st_mode):
            os.symlink(os.readlink(s), d)
        elif stat.S_ISFIFO(st.st_mode):
            os.mkfifo(d)
        elif key in seen and st.st_nlink > 1:
            os.link(seen[key], d)
        else:
            shutil.copyfile(s, d)
            seen[key] = d
    allp = []
    for dp, dn, fn in os.walk(src):
        for n in dn + fn:
            allp.append(os.path.relpath(os.path.join(dp, n), src))
    for rel in sorted(allp, key=lambda p: -p.count("/")) + ["."]:
        s, d = os.path.join(src, rel), os.path.join(dst, rel)
        st = os.lstat(s)
        if not stat.S_ISLNK(st.st_mode):
            os.chmod(d, stat.S_IMODE(st.st_mode))
        os.utime(d, ns=(st.st_atime_ns, st.st_mtime_ns), follow_symlinks=False)


def touch_all(root, t):
    for dp, dn, fn in os.walk(root):
        for n in dn + fn:
            os.utime(os.path.join(dp, n), (t, t), follow_symlinks=False)
    os.utime(root, (t, t))


def repro_checks(tools):
    """Each check builds twice under different conditions; images must be identical."""
    base = os.path.join(WORK, "repro")
    rmtree(base)
    os.makedirs(base)
    a = inp("tree1024")
    b = os.path.join(base, "copy-elsewhere", "x", "tree")
    copy_reversed(a, b)
    c = os.path.join(base, "copy-touched")
    copy_reversed(a, c)
    touch_all(c, 1_234_567_890)
    checks = [
        ("same-run-twice", ["-B", "1024", "-f", "-d", a], ["-B", "1024", "-f", "-d", a], {}, {}),
        ("path-and-creation-order", ["-B", "1024", "-f", "-d", a], ["-B", "1024", "-f", "-d", b], {}, {}),
        ("host-times-ignored(--timestamp)", ["-B", "1024", "--timestamp", "1700000000", "-d", a],
         ["-B", "1024", "--timestamp", "1700000000", "-d", c], {}, {}),
        ("host-times-ignored(-f)", ["-B", "1024", "-f", "-d", a], ["-B", "1024", "-f", "-d", c], {}, {}),
        ("SOURCE_DATE_EPOCH(wall clock)", ["-B", "1024", "-d", a], ["-B", "1024", "-d", a],
         {"SOURCE_DATE_EPOCH": "1600000000"}, {"SOURCE_DATE_EPOCH": "1600000000", "_sleep": "1.5"}),
        ("tar-vs-tar.gz", ["-B", "4096", "-f", "-a", inp("tree.pax.tar")], ["-B", "4096", "-f", "-a", inp("tree.tar.gz")],
         {}, {}),
        ("clamp-vs-host-times", ["-B", "1024", "--clamp-timestamps", "1000000000", "-d", a],
         ["-B", "1024", "--clamp-timestamps", "1000000000", "-d", c], {}, {}),
    ]
    out = []
    for name, a1, a2, e1, e2 in checks:
        for tool in tools:
            if not (supports(tool, a1) and supports(tool, a2)):
                continue
            i1 = os.path.join(base, f"{tool}-{name[:20]}-1.img".replace("/", "_"))
            i2 = os.path.join(base, f"{tool}-{name[:20]}-2.img".replace("/", "_"))
            r1 = run_tool(tool, a1, i1, env=e1)
            if "_sleep" in e2:
                time.sleep(float(e2["_sleep"]))
            r2 = run_tool(tool, a2, i2, env={k: v for k, v in e2.items() if not k.startswith("_")})
            if r1["rc"] or r2["rc"]:
                same = f"error rc={r1['rc']},{r2['rc']}"
            else:
                same = sha256_file(i1) == sha256_file(i2)
            out.append({"check": name, "tool": tool, "identical": same, "imgs": (i1, i2)})
    pending = [r for r in out if r["identical"] is False]
    reps = inspect_images([p for r in pending for p in r["imgs"]]) if pending else {}
    for r in pending:
        e1, e2 = (reps[os.path.abspath(p)].get("entries") for p in r["imgs"])
        if e1 is not None and e2 is not None:
            d = compare_manifests(e1, e2)
            r["content_diffs"] = d[:10]
            r["identical"] = "no (content differs)" if d else "no (layout only)"
    return out


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--only", default="")
    ap.add_argument("--tools", default="kext2,mkext2,xgenext2fs")
    ap.add_argument("--seeds", type=int, default=6)
    ap.add_argument("--no-repro", action="store_true")
    ap.add_argument("--json", default=os.path.join(WORK, "results.json"))
    args = ap.parse_args()
    tools = available_tools(args.tools.split(","))
    print("tools:", ", ".join(tools))
    prepare_inputs(args.seeds)
    imgdir = os.path.join(WORK, "images")
    os.makedirs(imgdir, exist_ok=True)

    results = []
    images = []
    for name, cargs, expect_ok, *extra in cases(args.seeds):
        extra = extra[0] if extra else {}
        if args.only and args.only not in name:
            continue
        row = {"case": name, "args": cargs, "expect_ok": expect_ok, "tools": {}, "ref": extra.get("ref"),
               "xfail": extra.get("xfail", []), "expect": extra.get("expect", {})}
        for tool in tools:
            if not supports(tool, cargs):
                continue
            img = os.path.join(imgdir, f"{name}.{tool}.img")
            if os.path.exists(img):
                os.remove(img)
            r = run_tool(tool, cargs, img, env=extra.get("env"))
            row["tools"][tool] = {"rc": r["rc"], "seconds": round(r["seconds"], 3), "stderr": r["stderr"][-600:],
                                  "img": img if r["rc"] == 0 and os.path.exists(img) else None}
            if row["tools"][tool]["img"]:
                images.append(img)
        results.append(row)

    print(f"inspecting {len(images)} images ...", flush=True)
    reports = {}
    for i in range(0, len(images), 40):
        reports.update(inspect_images(images[i:i + 40]))

    failures = 0
    for row in results:
        ts = row["tools"]
        ref = row["ref"] if row["ref"] in ts else (
            "xgenext2fs" if "xgenext2fs" in ts else ("mkext2" if "mkext2" in ts else None))
        row["ref"] = ref
        for tool, t in ts.items():
            # tools listed in xfail have a known bug for this case
            ok_rc = ((t["rc"] == 0) == row["expect"].get(tool, row["expect_ok"])) or tool in row["xfail"]
            t["xfail"] = tool in row["xfail"]
            t["ok_rc"] = ok_rc
            if not t["img"]:
                t["valid"] = None
                continue
            rep = reports[os.path.abspath(t["img"])]
            sb = rep["sb"]
            t["fsck_rc"] = rep["fsck_rc"]
            t["mount_ok"] = rep["mount_ok"]
            t["valid"] = rep["fsck_rc"] == 0 and rep["mount_ok"] and "walk_error" not in rep
            if not t["valid"]:
                t["fsck_out"] = rep["fsck_out"][-1500:]
                t["mount_err"] = rep.get("mount_err") or rep.get("walk_error")
            t["blocks"] = int(sb.get("Block count", 0))
            t["free_blocks"] = int(sb.get("Free blocks", 0))
            t["inodes"] = int(sb.get("Inode count", 0))
            t["free_inodes"] = int(sb.get("Free inodes", 0))
            t["groups_bpg"] = sb.get("Blocks per group")
            t["features"] = sb.get("Filesystem features")
            t["label"] = sb.get("Filesystem volume name")
            t["os"] = sb.get("Filesystem OS type")
            t["reserved"] = int(sb.get("Reserved block count", 0))
            t["entries"] = rep.get("entries")
        if ref and ts[ref].get("entries") is not None:
            for tool, t in ts.items():
                if tool == ref or t.get("entries") is None:
                    continue
                d = compare_manifests(ts[ref]["entries"], t["entries"])
                t["vs_ref"] = ref
                t["diffs"] = d
            if ts.get("kext2", {}).get("entries") is not None:
                for other in ts:
                    if other != "kext2" and ts[other].get("entries") is not None:
                        ts["kext2"]["vs_" + other] = len(compare_manifests(ts[other]["entries"], ts["kext2"]["entries"]))
        for tool, t in ts.items():
            bad = (not t["ok_rc"]) or (t["valid"] is False) or bool(t.get("diffs"))
            if tool in row["xfail"]:
                bad = False
            t["pass"] = not bad
            if tool == "kext2" and bad:
                failures += 1

    repro = [] if args.no_repro else repro_checks(tools)

    # --- report ---
    def cell(t):
        if t is None:
            return "—"
        if t["rc"] and t["ok_rc"] and not t["img"] and t.get("xfail"):
            return "xfail"
        if not t["ok_rc"]:
            return f"rc={t['rc']}!"
        if t["img"] is None:
            return "fails(ok)"
        s = "ok" if t["valid"] else ("FSCK" if t["fsck_rc"] else "MOUNT")
        if t.get("diffs"):
            s += f" Δ{len(t['diffs'])}"
        return s

    print()
    hdr = f"{'case':28}" + "".join(f"{x:>14}" for x in tools) + "   blocks(" + "/".join(tools) + ")"
    print(hdr)
    for row in results:
        ts = row["tools"]
        line = f"{row['case']:28}" + "".join(f"{cell(ts.get(x)):>14}" for x in tools)
        line += "   " + "/".join(str(ts[x].get("blocks", "-")) if x in ts else "-" for x in tools)
        print(line)
    print()
    for row in results:
        for tool, t in row["tools"].items():
            if not t["pass"]:
                print(f"--- {row['case']} [{tool}] rc={t['rc']} valid={t['valid']}")
                if t["stderr"].strip():
                    print("    stderr:", t["stderr"].strip()[-300:].replace("\n", "\n    "))
                if t.get("fsck_out") and t.get("fsck_rc"):
                    print("    fsck:", t["fsck_out"][-600:].replace("\n", "\n    "))
                if t.get("mount_err"):
                    print("    mount:", t["mount_err"])
                for d in (t.get("diffs") or [])[:12]:
                    print(f"    vs {t['vs_ref']}: {d}")
                if len(t.get("diffs") or []) > 12:
                    print(f"    ... {len(t['diffs']) - 12} more")
    if repro:
        print("\nreproducibility (identical images expected):")
        for r in repro:
            print(f"  {r['check']:36} {r['tool']:12} {r['identical']}")
            for d in r.get("content_diffs", [])[:3]:
                print(f"      {d}")
            if r["tool"] == "kext2" and r["identical"] is not True:
                failures += 1
    for row in results:
        for t in row["tools"].values():
            t.pop("entries", None)
    with open(args.json, "w") as f:
        json.dump({"cases": results, "repro": repro}, f, indent=1)
    print(f"\nkext2 failures: {failures}   (details: {args.json})")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())

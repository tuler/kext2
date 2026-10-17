"""Structural analysis of ext2 images (runs anywhere with Python 3; used in
the kext2-tools container). Reads the on-disk structures directly and prints
one JSON object {image: report}:

- geometry: block size, blocks, groups, blocks/inodes per group, inode size,
  features, reserved blocks;
- where the blocks go: superblock and descriptor copies, bitmaps, inode
  tables, file data, indirect blocks, directory blocks, symlink blocks, free;
- inodes: total, used, by type, numbering order relative to a sorted walk;
- layout: group placement of directories and files, data locality, file
  contiguity, directory entry order.

    python3 analyze_image.py IMAGE...
"""

import json
import struct
import sys

S_IFMT = 0o170000
TYPES = {0o100000: "file", 0o040000: "dir", 0o120000: "symlink", 0o020000: "chr", 0o060000: "blk",
         0o010000: "fifo", 0o140000: "sock"}


class Image:
    def __init__(self, path):
        self.f = open(path, "rb")
        sb = self.read_at(1024, 1024)
        u32 = lambda o: struct.unpack_from("<I", sb, o)[0]
        u16 = lambda o: struct.unpack_from("<H", sb, o)[0]
        assert u16(56) == 0xEF53, "not an ext2 image"
        self.inodes_count, self.blocks_count, self.r_blocks = u32(0), u32(4), u32(8)
        self.free_blocks, self.free_inodes, self.first_data_block = u32(12), u32(16), u32(20)
        self.bs = 1024 << u32(24)
        self.bpg, self.ipg = u32(32), u32(40)
        self.rev = u32(76)
        self.first_ino = u32(84) if self.rev else 11
        self.isize = u16(88) if self.rev else 128
        self.compat, self.incompat, self.ro_compat = u32(92), u32(96), u32(100)
        self.res_gdt = u16(206) if self.compat & 0x10 else 0
        self.groups = (self.blocks_count - self.first_data_block + self.bpg - 1) // self.bpg
        self.gdt_blocks = (self.groups * 32 + self.bs - 1) // self.bs
        gdt = self.read_at((self.first_data_block + 1) * self.bs, self.groups * 32)
        self.gd = [struct.unpack_from("<IIIHHH", gdt, g * 32) for g in range(self.groups)]
        self.itb = (self.ipg * self.isize + self.bs - 1) // self.bs

    def read_at(self, off, n):
        self.f.seek(off)
        return self.f.read(n)

    def block(self, b):
        return self.read_at(b * self.bs, self.bs)

    def group_of_block(self, b):
        return (b - self.first_data_block) // self.bpg

    def has_super(self, g):
        if g <= 1 or not (self.ro_compat & 1):
            return True
        for p in (3, 5, 7):
            n = p
            while n < g:
                n *= p
            if n == g:
                return True
        return False

    def inode(self, n):
        g, i = divmod(n - 1, self.ipg)
        raw = self.read_at(self.gd[g][2] * self.bs + i * self.isize, 128)
        mode, uid, size = struct.unpack_from("<HHI", raw, 0)
        links, blocks512 = struct.unpack_from("<HI", raw, 26)
        file_acl = struct.unpack_from("<I", raw, 104)[0]
        size_high = struct.unpack_from("<I", raw, 108)[0]
        iblock = list(struct.unpack_from("<15I", raw, 40))
        if mode & S_IFMT == 0o100000:
            size |= size_high << 32
        return {"mode": mode, "size": size, "links": links, "blocks512": blocks512, "block": iblock,
                "file_acl": file_acl}

    def used_inodes(self):
        out = []
        for g in range(self.groups):
            bm = self.block(self.gd[g][1])
            for i in range(self.ipg):
                if bm[i >> 3] >> (i & 7) & 1:
                    out.append(g * self.ipg + i + 1)
        return out

    def block_map(self, ino):
        """(data blocks in file order with holes skipped, indirect blocks)."""
        ptrs = self.bs // 4
        data, ind = [], []

        def walk(b, depth):
            if b == 0:
                return
            if depth == 0:
                data.append(b)
                return
            ind.append(b)
            for p in struct.unpack(f"<{ptrs}I", self.block(b)):
                if p:
                    walk(p, depth - 1)

        for b in ino["block"][:12]:
            if b:
                data.append(b)
        for k, depth in ((12, 1), (13, 2), (14, 3)):
            walk(ino["block"][k], depth)
        return data, ind


def runs(blocks):
    """Number of contiguous runs in a block list (in file order)."""
    if not blocks:
        return 0
    return 1 + sum(1 for a, b in zip(blocks, blocks[1:]) if b != a + 1)


def analyze(path):
    im = Image(path)
    bs = im.bs
    used = im.used_inodes()
    by_type = {}
    data_blocks = ind_blocks = dir_blocks = sym_blocks = 0
    file_bytes = 0
    files = files_contig = files_local = 0
    frag_runs = 0
    inode_of = {}
    info = {}
    for n in used:
        ino = im.inode(n)
        if n < im.first_ino and n != 2:
            continue
        if ino["links"] == 0:
            continue
        t = TYPES.get(ino["mode"] & S_IFMT, "?")
        by_type[t] = by_type.get(t, 0) + 1
        fast_symlink = t == "symlink" and ino["blocks512"] - (bs // 512 if ino["file_acl"] else 0) == 0
        if t in ("file", "dir") or (t == "symlink" and not fast_symlink):
            data, ind = im.block_map(ino)
        else:
            data, ind = [], []
        ind_blocks += len(ind)
        if t == "file":
            data_blocks += len(data)
            file_bytes += ino["size"]
            if data:
                files += 1
                # contiguous: data and its indirect blocks form one run
                allb = sorted(data + ind)
                r = runs(allb)
                frag_runs += r
                files_contig += r == 1
                files_local += im.group_of_block(data[0]) == (n - 1) // im.ipg
        elif t == "dir":
            dir_blocks += len(data)
        elif t == "symlink":
            sym_blocks += len(data)
        info[n] = (t, data)

    # walk the tree from the root: names, order, parents
    entries_sorted = dirs_seen = 0
    order = []  # inode numbers in a sorted depth-first walk
    seen = set()
    placement = {"dirs_in_parent_group": 0, "dirs": 0, "files_in_parent_group": 0, "files_placed": 0}
    groups_with_topdirs = set()
    stack = [(2, 0)]
    while stack:
        d, depth = stack.pop()
        if d in seen:
            continue
        seen.add(d)
        names = []
        for b in info.get(d, ("dir", []))[1]:
            blk = im.block(b)
            off = 0
            while off < bs:
                ino_n, rec_len, name_len = struct.unpack_from("<IHB", blk, off)
                if rec_len == 0:
                    break
                if ino_n:
                    name = blk[off + 8:off + 8 + name_len]
                    if name not in (b".", b".."):
                        names.append((name, ino_n))
                off += rec_len
        dirs_seen += 1
        entries_sorted += [n for n, _ in names] == sorted(n for n, _ in names)
        pg = (d - 1) // im.ipg
        subdirs = []
        for name, child in sorted(names):
            if child not in inode_of:
                inode_of[child] = name
                order.append(child)
            t = info.get(child, ("?", []))[0]
            cg = (child - 1) // im.ipg
            if t == "dir":
                placement["dirs"] += 1
                placement["dirs_in_parent_group"] += cg == pg
                if d == 2:
                    groups_with_topdirs.add(cg)
                subdirs.append(child)
            else:
                placement["files_placed"] += 1
                placement["files_in_parent_group"] += cg == pg
        for s in reversed(subdirs):
            stack.append((s, depth + 1))

    # numbering: how often consecutive entries of a sorted walk have increasing inode numbers
    increasing = sum(1 for a, b in zip(order, order[1:]) if b > a)

    groups_with_sb = sum(1 for g in range(im.groups) if im.has_super(g))
    sb_gdt = groups_with_sb * (1 + im.gdt_blocks + im.res_gdt)
    bitmaps = 2 * im.groups
    itables = im.itb * im.groups
    content = data_blocks + ind_blocks + dir_blocks + sym_blocks
    accounted = sb_gdt + bitmaps + itables + content + im.free_blocks + (im.first_data_block)
    return {
        "geometry": {
            "block_size": bs, "blocks": im.blocks_count, "groups": im.groups, "blocks_per_group": im.bpg,
            "inodes_per_group": im.ipg, "inode_size": im.isize, "inode_table_blocks_per_group": im.itb,
            "inodes": im.inodes_count, "reserved_blocks": im.r_blocks, "rev": im.rev,
            "features": {"compat": im.compat, "incompat": im.incompat, "ro_compat": im.ro_compat},
            "reserved_gdt_blocks": im.res_gdt, "groups_with_superblock": groups_with_sb,
        },
        "blocks": {
            "superblocks_and_descriptors": sb_gdt, "bitmaps": bitmaps, "inode_tables": itables,
            "file_data": data_blocks, "indirect": ind_blocks, "directories": dir_blocks, "symlinks": sym_blocks,
            "free": im.free_blocks, "boot_block": im.first_data_block,
            "unaccounted": im.blocks_count - accounted,
        },
        "content": {"file_bytes": file_bytes, "files": by_type.get("file", 0)},
        "inodes": {"total": im.inodes_count, "used": im.inodes_count - im.free_inodes, "by_type": by_type},
        "layout": {
            "numbering_follows_sorted_walk": increasing / max(1, len(order) - 1),
            "dirs_with_sorted_entries": entries_sorted / max(1, dirs_seen),
            "dirs_in_parent_group": placement["dirs_in_parent_group"] / max(1, placement["dirs"]),
            "files_in_parent_group": placement["files_in_parent_group"] / max(1, placement["files_placed"]),
            "files_data_in_inode_group": files_local / max(1, files),
            "files_contiguous": files_contig / max(1, files),
            "runs_per_file": frag_runs / max(1, files),
            "groups_holding_top_level_dirs": len(groups_with_topdirs),
            "groups_with_used_inodes": len({(n - 1) // im.ipg for n in used}),
        },
    }


if __name__ == "__main__":
    print(json.dumps({p: analyze(p) for p in sys.argv[1:]}))

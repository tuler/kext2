//! Block-group geometry and image sizing (SPEC §7.3, §7.4).

use crate::error::{Error, Result};

pub const MAX_BLOCKS: u64 = u32::MAX as u64;

/// True if group `g` holds a superblock backup (sparse_super, R-GEO-1).
pub fn has_super(g: u64) -> bool {
    if g <= 1 {
        return true;
    }
    for base in [3u64, 5, 7] {
        let mut p = base;
        while p < g {
            p *= base;
        }
        if p == g {
            return true;
        }
    }
    false
}

/// Number of groups `h < n` with `has_super(h)`.
pub fn count_super(n: u64) -> u64 {
    if n == 0 {
        return 0;
    }
    let mut c = n.min(2); // groups 0 and 1
    for base in [3u64, 5, 7] {
        let mut p = base;
        while p < n {
            c += 1;
            match p.checked_mul(base) {
                Some(q) => p = q,
                None => break,
            }
        }
    }
    c
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Geometry {
    pub bs: u32,
    /// `s_first_data_block`.
    pub fdb: u64,
    /// Total blocks `B`.
    pub blocks: u64,
    /// Number of groups `G`.
    pub groups: u64,
    pub bpg: u64,
    pub ipg: u64,
    /// Inode-table blocks per group.
    pub itb: u64,
    /// Group-descriptor blocks.
    pub gdb: u64,
    /// Data capacity `C` (data blocks over all groups).
    pub capacity: u64,
}

/// Result of [`geometry`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Geo {
    Valid(Geometry),
    Invalid { capacity: i128 },
}

fn ceil(a: u64, b: u64) -> u64 {
    a.div_ceil(b)
}
fn roundup(a: u64, k: u64) -> u64 {
    ceil(a, k) * k
}

/// GEOMETRY(B, I) of R-GEO-2.
pub fn geometry(bs: u32, blocks: u64, inodes: u64) -> Geo {
    let bsz = bs as u64;
    let fdb = if bs == 1024 { 1 } else { 0 };
    let ipb = bsz / 128;
    if blocks <= fdb {
        return Geo::Invalid { capacity: 0 };
    }
    let n = blocks - fdb;
    let gmin = ceil(inodes, 8 * bsz).max(1);
    let mut bpg = (8 * bsz).min(roundup(ceil(n, gmin), 8));
    let ipg_of = |bpg: u64| roundup(ceil(inodes, ceil(n, bpg)), ipb).max(16);
    // Iterations with ipg > 8·bs only lower bpg: jump to the largest bpg of
    // the sequence bpg, bpg−8, … with ipg ≤ 8·bs (ipg grows with bpg).
    if bpg >= 8 && ipg_of(bpg) > 8 * bsz {
        let (mut lo, mut hi) = (0u64, (bpg - 8) / 8 + 1); // k in [lo, hi): bpg − 8k
        if ipg_of(bpg - 8 * (hi - 1)) > 8 * bsz {
            bpg -= 8 * (hi - 1);
        } else {
            // invariant: ipg(bpg − 8·lo) > 8bs, ipg(bpg − 8·(hi−1)) ≤ 8bs
            hi -= 1;
            while hi - lo > 1 {
                let mid = (lo + hi) / 2;
                if ipg_of(bpg - 8 * mid) > 8 * bsz {
                    lo = mid;
                } else {
                    hi = mid;
                }
            }
            bpg -= 8 * hi;
        }
    }
    let mut cap: i128 = 0;
    while bpg >= 8 {
        let g = ceil(n, bpg);
        let ipg = roundup(ceil(inodes, g), ipb).max(16);
        let itb = ipg / ipb;
        let gdb = ceil(g * 32, bsz);
        let nsuper = count_super(g);
        let overhead = g as i128 * (2 + itb) as i128 + nsuper as i128 * (1 + gdb) as i128;
        cap = n as i128 - overhead;
        if ipg <= 8 * bsz {
            if bpg <= itb + 3 || cap <= 0 {
                return Geo::Invalid { capacity: cap };
            }
            let ov_super = 1 + gdb + 2 + itb;
            let ov_plain = 2 + itb;
            let last = n - (g - 1) * bpg;
            let ov_last = if has_super(g - 1) { ov_super } else { ov_plain };
            // every group but the last has bpg blocks; group 0 has a superblock
            let others_ok = g == 1 || bpg > ov_super;
            if others_ok && last > ov_last {
                return Geo::Valid(Geometry { bs, fdb, blocks, groups: g, bpg, ipg, itb, gdb, capacity: cap as u64 });
            }
        }
        bpg -= 8;
    }
    Geo::Invalid { capacity: cap }
}

/// Is there any geometry that can hold `inodes` inodes? Group 0 must hold the
/// superblock, a group-descriptor table for all groups, two bitmaps and its
/// inode table in at most `8·bs` blocks; more groups mean a larger
/// descriptor table, fewer mean larger inode tables.
pub fn inodes_feasible(bs: u32, inodes: u64) -> bool {
    let bsz = bs as u64;
    let ipb = bsz / 128;
    if inodes > u32::MAX as u64 {
        return false;
    }
    let gmin = ceil(inodes, 8 * bsz).max(1);
    let ov = |g: u64| 3 + ceil(g * 32, bsz) + roundup(ceil(inodes, g), ipb).max(16) / ipb;
    if ov(gmin) < 8 * bsz {
        return true;
    }
    // beyond bs²/4 groups the descriptor table alone fills a group
    let gmax = bsz * bsz / 4;
    (gmin..=gmax).any(|g| roundup(ceil(inodes, g), ipb) <= 8 * bsz && ov(g) < 8 * bsz)
}

/// SIZE(D, F, I) of R-SIZE-3.
pub fn size(bs: u32, d: u64, f: u64, inodes: u64) -> Result<u64> {
    if !inodes_feasible(bs, inodes) {
        return Err(Error::fatal(format!("too many inodes ({inodes}) for {bs}-byte blocks")));
    }
    let fdb = if bs == 1024 { 1 } else { 0 };
    let ipb = bs as u64 / 128;
    let need = d as u128 + f as u128;
    let mut b: u128 = fdb as u128 + need + 4 + ceil(inodes, ipb) as u128;
    let mut rounds = 0u64;
    loop {
        if b > MAX_BLOCKS as u128 {
            return Err(Error::fatal("filesystem too large"));
        }
        rounds += 1;
        // The search adds one block at a time while the geometry is invalid
        // but has capacity; with hundreds of millions of inodes it would
        // need billions of rounds. Give up then (use -b or larger blocks).
        if rounds > 1_000_000 {
            return Err(Error::fatal(format!(
                "no geometry found for {inodes} inodes (too many inodes; give -b, or use larger blocks)"
            )));
        }
        let (ok, cap) = match geometry(bs, b as u64, inodes) {
            Geo::Valid(g) => (true, g.capacity as i128),
            Geo::Invalid { capacity } => (false, capacity),
        };
        if ok && cap >= need as i128 {
            return Ok(b as u64);
        }
        let deficit = need as i128 - cap;
        b += deficit.max(1) as u128;
    }
}

impl Geometry {
    pub fn has_super(&self, g: u64) -> bool {
        has_super(g)
    }
    /// First block of group `g`.
    pub fn start(&self, g: u64) -> u64 {
        self.fdb + g * self.bpg
    }
    /// Blocks in group `g`.
    pub fn size(&self, g: u64) -> u64 {
        if g + 1 < self.groups {
            self.bpg
        } else {
            self.blocks - self.fdb - (self.groups - 1) * self.bpg
        }
    }
    /// Metadata blocks at the start of group `g`.
    pub fn overhead(&self, g: u64) -> u64 {
        (if has_super(g) { 1 + self.gdb } else { 0 }) + 2 + self.itb
    }
    /// Data blocks of group `g`.
    pub fn data_blocks(&self, g: u64) -> u64 {
        self.size(g) - self.overhead(g)
    }
    /// Data index of the first data block of group `g` (R-GEO-4).
    pub fn first_data_index(&self, g: u64) -> u64 {
        g * (self.bpg - 2 - self.itb) - count_super(g) * (1 + self.gdb)
    }
    /// Block bitmap block of group `g`.
    pub fn block_bitmap(&self, g: u64) -> u64 {
        self.start(g) + if has_super(g) { 1 + self.gdb } else { 0 }
    }
    pub fn inode_bitmap(&self, g: u64) -> u64 {
        self.block_bitmap(g) + 1
    }
    pub fn inode_table(&self, g: u64) -> u64 {
        self.block_bitmap(g) + 2
    }
    pub fn inodes_count(&self) -> u64 {
        self.groups * self.ipg
    }
    pub fn ipb(&self) -> u64 {
        self.bs as u64 / 128
    }
}

/// Maps data indices to block numbers.
#[derive(Debug, Clone)]
pub struct DataMap {
    /// First data index of each group, plus the capacity at the end.
    fdi: Vec<u64>,
    /// First data block of each group.
    fdb: Vec<u64>,
}

impl DataMap {
    pub fn new(g: &Geometry) -> DataMap {
        let mut fdi = Vec::with_capacity(g.groups as usize + 1);
        let mut fdb = Vec::with_capacity(g.groups as usize);
        for i in 0..g.groups {
            fdi.push(g.first_data_index(i));
            fdb.push(g.start(i) + g.overhead(i));
        }
        fdi.push(g.capacity);
        DataMap { fdi, fdb }
    }
    /// Group holding data index `d` (`G−1` for `d = C`).
    pub fn group_of(&self, d: u64) -> u64 {
        let groups = self.fdb.len();
        let i = self.fdi[..groups].partition_point(|&x| x <= d);
        (i.max(1) - 1) as u64
    }
    pub fn first_index(&self, g: u64) -> u64 {
        self.fdi[g as usize]
    }
    /// Block number of data index `d`.
    pub fn block(&self, d: u64) -> u64 {
        let g = self.group_of(d) as usize;
        self.fdb[g] + (d - self.fdi[g])
    }
    /// Call `f(block, count)` for the contiguous block runs of data indices
    /// `[d, d+n)`.
    pub fn for_runs(&self, mut d: u64, mut n: u64, mut f: impl FnMut(u64, u64)) {
        while n > 0 {
            let g = self.group_of(d) as usize;
            let end = self.fdi[g + 1];
            let take = n.min(end - d);
            f(self.fdb[g] + (d - self.fdi[g]), take);
            d += take;
            n -= take;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sparse_groups() {
        let s: Vec<u64> = (0..60).filter(|&g| has_super(g)).collect();
        assert_eq!(s, vec![0, 1, 3, 5, 7, 9, 25, 27, 49]);
        for n in 0..3000 {
            assert_eq!(count_super(n), (0..n).filter(|&g| has_super(g)).count() as u64, "n={n}");
        }
    }

    #[test]
    fn worked_example() {
        // SPEC §7.4: SIZE(3, 0, 14) with 1 KiB blocks = 10
        assert_eq!(size(1024, 3, 0, 14).unwrap(), 10);
        match geometry(1024, 10, 14) {
            Geo::Valid(g) => {
                assert_eq!((g.groups, g.bpg, g.ipg, g.itb, g.gdb, g.capacity), (1, 16, 16, 2, 1, 3));
                assert_eq!(g.first_data_index(0), 0);
                let m = DataMap::new(&g);
                assert_eq!(m.block(0), 7);
            }
            _ => panic!(),
        }
    }

    #[test]
    fn invariants() {
        for bs in [1024u32, 2048, 4096] {
            for b in (1..20000u64).step_by(37).chain([8193, 8194, 8195, 16385, 16386, 24577, 3 * 8192 + 1]) {
                for inodes in [11u64, 100, 2000, 30000] {
                    if let Geo::Valid(g) = geometry(bs, b, inodes) {
                        let bsz = bs as u64;
                        assert!(g.bpg % 8 == 0 && g.bpg <= 8 * bsz);
                        assert!(g.ipg % 8 == 0 && g.ipg >= 16 && g.ipg <= 8 * bsz && g.ipg % (bsz / 128) == 0);
                        assert!(g.bpg > g.itb + 3);
                        assert!(g.groups * g.ipg >= inodes);
                        assert_eq!(g.groups, (b - g.fdb).div_ceil(g.bpg));
                        let mut cap = 0;
                        for gr in 0..g.groups {
                            assert!(g.size(gr) > g.overhead(gr));
                            assert_eq!(g.first_data_index(gr), cap);
                            cap += g.data_blocks(gr);
                        }
                        assert_eq!(cap, g.capacity);
                    }
                }
            }
        }
    }

    /// GEOMETRY exactly as written in R-GEO-2 (no shortcuts).
    fn geometry_ref(bs: u32, blocks: u64, inodes: u64) -> Geo {
        let bsz = bs as u64;
        let fdb = if bs == 1024 { 1 } else { 0 };
        let ipb = bsz / 128;
        if blocks <= fdb {
            return Geo::Invalid { capacity: 0 };
        }
        let n = blocks - fdb;
        let gmin = ceil(inodes, 8 * bsz).max(1);
        let mut bpg = (8 * bsz).min(roundup(ceil(n, gmin), 8));
        let mut cap: i128 = 0;
        while bpg >= 8 {
            let g = ceil(n, bpg);
            let ipg = roundup(ceil(inodes, g), ipb).max(16);
            let itb = ipg / ipb;
            let gdb = ceil(g * 32, bsz);
            let ov = |x: u64| (if has_super(x) { 1 + gdb } else { 0 }) + 2 + itb;
            let size = |x: u64| if x + 1 < g { bpg } else { n - (g - 1) * bpg };
            cap = n as i128 - (0..g).map(|x| ov(x) as i128).sum::<i128>();
            if ipg <= 8 * bsz {
                if bpg <= itb + 3 || cap <= 0 {
                    return Geo::Invalid { capacity: cap };
                }
                if (0..g).all(|x| size(x) > ov(x)) {
                    return Geo::Valid(Geometry {
                        bs,
                        fdb,
                        blocks,
                        groups: g,
                        bpg,
                        ipg,
                        itb,
                        gdb,
                        capacity: cap as u64,
                    });
                }
            }
            bpg -= 8;
        }
        Geo::Invalid { capacity: cap }
    }

    #[test]
    fn matches_reference() {
        for bs in [1024u32, 2048, 4096] {
            for b in (2..40000u64).step_by(97).chain([8193, 8200, 16386, 24577, 65537, 200_001]) {
                for i in [11u64, 64, 1000, 9000, 20012, 40000, 70000, 150_000, 400_000] {
                    assert_eq!(geometry(bs, b, i), geometry_ref(bs, b, i), "bs={bs} b={b} i={i}");
                }
            }
        }
    }

    #[test]
    fn infeasible_inodes() {
        assert!(inodes_feasible(1024, 100_000));
        assert!(!inodes_feasible(1024, 4_000_000_000));
        assert!(size(1024, 1, 0, 4_000_000_000).is_err());
    }

    #[test]
    fn size_is_valid() {
        for bs in [1024u32, 2048, 4096] {
            for d in [1u64, 5, 100, 8000, 9000, 70000] {
                for i in [11u64, 500, 20012] {
                    let b = size(bs, d, 0, i).unwrap();
                    match geometry(bs, b, i) {
                        Geo::Valid(g) => assert!(g.capacity >= d),
                        _ => panic!(),
                    }
                }
            }
        }
    }

    #[test]
    fn data_map_runs() {
        let g = match geometry(1024, 30000, 100) {
            Geo::Valid(g) => g,
            _ => panic!(),
        };
        assert!(g.groups >= 3);
        let m = DataMap::new(&g);
        let d1 = g.first_data_index(1);
        let mut v = vec![];
        m.for_runs(d1 - 2, 4, |b, n| v.push((b, n)));
        assert_eq!(v.len(), 2);
        assert_eq!(v[0], (g.start(1) - 2, 2));
        assert_eq!(v[1], (g.start(1) + g.overhead(1), 2));
        assert_eq!(m.group_of(d1), 1);
        assert_eq!(m.group_of(d1 - 1), 0);
        assert_eq!(m.group_of(g.capacity), g.groups - 1);
    }
}

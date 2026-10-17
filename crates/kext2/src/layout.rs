//! Block map layout (SPEC §8.5): the layout sequence of an inode's blocks,
//! indirect blocks included (R-FMT-12, R-FMT-13), and file size limits.
//! Every file is dense: its logical blocks are `[0, n)`.

use crate::geometry::BS;

/// One element of a layout sequence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Seg {
    /// Data blocks `[lstart, lstart+n)` (all within one indirect range).
    Data { lstart: u64, n: u64 },
    /// An indirect block of `level` (1 single, 2 double, 3 triple) mapping
    /// logical blocks from `lstart`.
    Ind { level: u8, lstart: u64 },
}

/// Logical-block boundaries for `p` block numbers per indirect block.
#[derive(Debug, Clone, Copy)]
pub struct Tiers {
    pub p: u64,
    /// First logical block mapped through `i_block[12]`, `[13]`, `[14]`, and
    /// the end of the triple-indirect range.
    pub single: u64,
    pub double: u64,
    pub triple: u64,
    pub end: u64,
}

impl Tiers {
    pub fn new() -> Tiers {
        let p = BS as u64 / 4;
        let single = 12;
        let double = single + p;
        let triple = double + p * p;
        let end = triple + p * p * p;
        Tiers { p, single, double, triple, end }
    }
}

impl Default for Tiers {
    fn default() -> Self {
        Tiers::new()
    }
}

/// Maximum regular file size in bytes (R-FMT-15).
pub const MAX_FILE_SIZE: u64 = 2_196_873_666_560;

/// Walk the layout sequence of a file with logical blocks `[0, n)`.
pub fn walk(n: u64, t: &Tiers, mut emit: impl FnMut(Seg)) {
    let p = t.p;
    let mut last = [u64::MAX; 4]; // last emitted lstart per level
    let mut l = 0;
    while l < n {
        // range [a, b) of the level-1 node containing l
        let b;
        if l < t.single {
            b = t.single;
        } else if l < t.double {
            if last[1] != t.single {
                emit(Seg::Ind { level: 1, lstart: t.single });
                last[1] = t.single;
            }
            b = t.double;
        } else if l < t.triple {
            if last[2] != t.double {
                emit(Seg::Ind { level: 2, lstart: t.double });
                last[2] = t.double;
            }
            let i = t.double + (l - t.double) / p * p;
            if last[1] != i {
                emit(Seg::Ind { level: 1, lstart: i });
                last[1] = i;
            }
            b = i + p;
        } else {
            if last[3] != t.triple {
                emit(Seg::Ind { level: 3, lstart: t.triple });
                last[3] = t.triple;
            }
            let m = l - t.triple;
            let d = t.triple + m / (p * p) * (p * p);
            if last[2] != d {
                emit(Seg::Ind { level: 2, lstart: d });
                last[2] = d;
            }
            let i = t.triple + m / p * p;
            if last[1] != i {
                emit(Seg::Ind { level: 1, lstart: i });
                last[1] = i;
            }
            b = i + p;
        }
        let e = n.min(b);
        emit(Seg::Data { lstart: l, n: e - l });
        l = e;
    }
}

/// Number of indirect blocks for a dense file of `n` blocks (closed form).
pub fn dense_indirect(n: u64, t: &Tiers) -> u64 {
    let p = t.p;
    let mut c = 0;
    if n > t.single {
        c += 1;
    }
    if n > t.double {
        let m = (n - t.double).min(p * p);
        c += 1 + m.div_ceil(p);
    }
    if n > t.triple {
        let m = n - t.triple;
        c += 1 + m.div_ceil(p * p) + m.div_ceil(p);
    }
    c
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dense_sequence() {
        let t = Tiers::new(); // p = 1024
        let mut v = vec![];
        walk(12 + 1024 + 1500, &t, |s| v.push(s));
        assert_eq!(
            v,
            vec![
                Seg::Data { lstart: 0, n: 12 },
                Seg::Ind { level: 1, lstart: 12 },
                Seg::Data { lstart: 12, n: 1024 },
                Seg::Ind { level: 2, lstart: 1036 },
                Seg::Ind { level: 1, lstart: 1036 },
                Seg::Data { lstart: 1036, n: 1024 },
                Seg::Ind { level: 1, lstart: 2060 },
                Seg::Data { lstart: 2060, n: 476 },
            ]
        );
    }

    #[test]
    fn dense_counts_match() {
        let t = Tiers::new();
        let p = t.p;
        let ns = [0, 1, 11, 12, 13, 12 + p - 1, 12 + p, 12 + p + 1, 12 + p + p * p, t.triple + 1, t.triple + p * p + 5];
        for n in ns {
            let mut ind = 0;
            walk(n, &t, |s| ind += matches!(s, Seg::Ind { .. }) as u64);
            assert_eq!(ind, dense_indirect(n, &t), "n={n}");
        }
    }

    #[test]
    fn max_size() {
        // R-FMT-15: max logical blocks
        assert_eq!(MAX_FILE_SIZE.div_ceil(4096), 536_346_110);
    }
}

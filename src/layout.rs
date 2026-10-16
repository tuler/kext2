//! Block map layout (SPEC §8.5): the layout sequence of an inode's blocks,
//! indirect blocks included (R-FMT-12, R-FMT-13), and file size limits.

use crate::runs::Runs;

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
    pub fn new(bs: u32) -> Tiers {
        let p = bs as u64 / 4;
        let single = 12;
        let double = single + p;
        let triple = double + p * p;
        let end = triple + p * p * p;
        Tiers { p, single, double, triple, end }
    }
}

/// Maximum regular file size in bytes (R-FMT-15).
pub fn max_file_size(bs: u32) -> u64 {
    match bs {
        1024 => 17_247_252_480,
        2048 => 275_415_851_008,
        _ => 2_196_873_666_560,
    }
}

/// Walk the layout sequence of the allocated logical blocks `s`.
pub fn walk(s: &Runs, t: &Tiers, mut emit: impl FnMut(Seg)) {
    let p = t.p;
    let mut last = [u64::MAX; 4]; // last emitted lstart per level
    for &(start, len) in &s.0 {
        let end = start + len;
        let mut l = start;
        while l < end {
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
            let e = end.min(b);
            emit(Seg::Data { lstart: l, n: e - l });
            l = e;
        }
    }
}

/// Number of indirect blocks needed for `s`.
pub fn count_indirect(s: &Runs, t: &Tiers) -> u64 {
    // Closed form per run would be faster, but runs are few and data
    // segments are emitted per indirect range, so the walk is O(nb/p).
    let mut n = 0;
    walk(s, t, |seg| {
        if let Seg::Ind { .. } = seg {
            n += 1
        }
    });
    n
}

/// Number of blocks `nb` (data + indirect) of `s`.
pub fn nb(s: &Runs, t: &Tiers) -> u64 {
    s.count() + count_indirect(s, t)
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
        let t = Tiers::new(1024); // p = 256
        let mut v = vec![];
        walk(&Runs::dense(12 + 256 + 300), &t, |s| v.push(s));
        assert_eq!(
            v,
            vec![
                Seg::Data { lstart: 0, n: 12 },
                Seg::Ind { level: 1, lstart: 12 },
                Seg::Data { lstart: 12, n: 256 },
                Seg::Ind { level: 2, lstart: 268 },
                Seg::Ind { level: 1, lstart: 268 },
                Seg::Data { lstart: 268, n: 256 },
                Seg::Ind { level: 1, lstart: 524 },
                Seg::Data { lstart: 524, n: 44 },
            ]
        );
    }

    #[test]
    fn dense_counts_match() {
        for bs in [1024u32, 4096] {
            let t = Tiers::new(bs);
            let p = t.p;
            for n in
                [0, 1, 11, 12, 13, 12 + p - 1, 12 + p, 12 + p + 1, 12 + p + p * p, t.triple + 1, t.triple + p * p + 5]
            {
                assert_eq!(count_indirect(&Runs::dense(n), &t), dense_indirect(n, &t), "bs={bs} n={n}");
            }
        }
    }

    #[test]
    fn sparse_sequence() {
        let t = Tiers::new(1024);
        let mut s = Runs::new();
        s.push(0, 1);
        s.push(t.triple, 1);
        s.push(t.triple + 256 * 256 + 3, 2);
        let mut v = vec![];
        walk(&s, &t, |x| v.push(x));
        assert_eq!(
            v,
            vec![
                Seg::Data { lstart: 0, n: 1 },
                Seg::Ind { level: 3, lstart: t.triple },
                Seg::Ind { level: 2, lstart: t.triple },
                Seg::Ind { level: 1, lstart: t.triple },
                Seg::Data { lstart: t.triple, n: 1 },
                Seg::Ind { level: 2, lstart: t.triple + 65536 },
                Seg::Ind { level: 1, lstart: t.triple + 65536 },
                Seg::Data { lstart: t.triple + 65536 + 3, n: 2 },
            ]
        );
        assert_eq!(nb(&s, &t), 4 + 5);
    }

    #[test]
    fn max_sizes() {
        // R-FMT-15: max logical blocks
        assert_eq!(max_file_size(1024).div_ceil(1024), 16_843_020);
        assert_eq!(max_file_size(2048).div_ceil(2048), 134_480_396);
        assert_eq!(max_file_size(4096).div_ceil(4096), 536_346_110);
    }
}

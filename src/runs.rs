//! Run-length sets of block numbers and zero-block detection (R-PERF-2).

/// A set of block numbers stored as sorted, non-overlapping, non-adjacent
/// runs `(start, len)`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Runs(pub Vec<(u64, u64)>);

impl Runs {
    pub fn new() -> Runs {
        Runs(Vec::new())
    }
    /// The run `[0, n)` (empty if n = 0).
    pub fn dense(n: u64) -> Runs {
        if n == 0 {
            Runs(Vec::new())
        } else {
            Runs(vec![(0, n)])
        }
    }
    /// Append blocks `[start, start+len)`; `start` must not precede the end of
    /// the last run.
    pub fn push(&mut self, start: u64, len: u64) {
        if len == 0 {
            return;
        }
        if let Some(last) = self.0.last_mut() {
            debug_assert!(start >= last.0 + last.1);
            if last.0 + last.1 == start {
                last.1 += len;
                return;
            }
        }
        self.0.push((start, len));
    }
    /// Number of blocks in the set.
    pub fn count(&self) -> u64 {
        self.0.iter().map(|r| r.1).sum()
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    /// Is block `b` in the set?
    pub fn contains(&self, b: u64) -> bool {
        let i = self.0.partition_point(|r| r.0 + r.1 <= b);
        i < self.0.len() && self.0[i].0 <= b
    }
}

/// True if every byte of `b` is zero.
#[inline]
pub fn is_zero(b: &[u8]) -> bool {
    is_zero_block(b)
}

/// Zero test without allocation, used on hot paths.
#[inline]
pub fn is_zero_block(b: &[u8]) -> bool {
    let mut chunks = b.chunks_exact(32);
    for c in &mut chunks {
        let mut acc = 0u64;
        for w in c.chunks_exact(8) {
            acc |= u64::from_ne_bytes(w.try_into().unwrap());
        }
        if acc != 0 {
            return false;
        }
    }
    chunks.remainder().iter().all(|&x| x == 0)
}

/// Builds the set of logical blocks of a file that hold a non-zero byte, from
/// its data fed in increasing logical order (bytes not fed are zero).
#[derive(Debug)]
pub struct ZeroMapBuilder {
    bs: u64,
    runs: Runs,
    /// Last block marked non-zero, to avoid re-testing it.
    last_marked: Option<u64>,
}

impl ZeroMapBuilder {
    pub fn new(bs: u32) -> Self {
        ZeroMapBuilder { bs: bs as u64, runs: Runs::new(), last_marked: None }
    }

    /// Feed bytes at logical offset `off`.
    pub fn feed(&mut self, off: u64, data: &[u8]) {
        let bs = self.bs;
        let mut pos = 0usize;
        while pos < data.len() {
            let o = off + pos as u64;
            let blk = o / bs;
            let in_blk = (o % bs) as usize;
            let take = ((bs as usize) - in_blk).min(data.len() - pos);
            if self.last_marked != Some(blk) && !is_zero_block(&data[pos..pos + take]) {
                self.runs.push(blk, 1);
                self.last_marked = Some(blk);
            }
            pos += take;
        }
    }

    pub fn finish(self) -> Runs {
        self.runs
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runs_merge() {
        let mut r = Runs::new();
        r.push(0, 2);
        r.push(2, 3);
        r.push(10, 1);
        assert_eq!(r.0, vec![(0, 5), (10, 1)]);
        assert_eq!(r.count(), 6);
        assert!(r.contains(4) && !r.contains(5) && r.contains(10) && !r.contains(11));
    }

    #[test]
    fn zero_map() {
        let mut z = ZeroMapBuilder::new(1024);
        let mut d = vec![0u8; 5000];
        d[1500] = 1; // block 1
        d[4999] = 2; // block 4
        z.feed(0, &d);
        z.feed(10 * 1024 + 3, b"x"); // block 10
        z.feed(10 * 1024 + 900, b"y");
        assert_eq!(z.finish().0, vec![(1, 1), (4, 1), (10, 1)]);
        assert!(is_zero(&[0u8; 100]));
        assert!(!is_zero(&[0, 0, 0, 0, 0, 0, 0, 0, 0, 1]));
        assert!(is_zero_block(&[0u8; 4096]));
        let mut b = [0u8; 4096];
        b[4095] = 1;
        assert!(!is_zero_block(&b));
    }
}

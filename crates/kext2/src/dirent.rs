//! Directory entries and packing (SPEC §8.7).

use crate::geometry::BS;

/// `EXT2_DIR_REC_LEN`.
#[inline]
pub fn rec_len(name_len: usize) -> usize {
    (8 + name_len + 3) & !3
}

/// Pack entries with the given name lengths (`.` and `..` included) into
/// blocks (R-DIRENT-4). Returns the index of the first entry of each block.
pub fn pack(name_lens: impl Iterator<Item = usize>) -> Vec<u32> {
    let bs = BS as usize;
    let mut starts = Vec::new();
    let mut used = bs; // forces a first block
    for (i, n) in name_lens.enumerate() {
        let r = rec_len(n);
        if used + r > bs {
            starts.push(i as u32);
            used = 0;
        }
        used += r;
    }
    starts
}

/// One directory entry to write.
pub struct Entry<'a> {
    pub ino: u32,
    pub name: &'a [u8],
    pub file_type: u8,
}

/// Write `entries` into one directory block `out` (`out.len()` = block size).
/// The last entry's `rec_len` extends to the end of the block. An empty
/// `entries` writes an empty block (inode 0, rec_len = bs).
pub fn write_block<'a>(out: &mut [u8], entries: impl Iterator<Item = Entry<'a>>) {
    let bs = out.len();
    let mut pos = 0usize;
    let mut last: Option<usize> = None;
    for e in entries {
        let r = rec_len(e.name.len());
        debug_assert!(pos + r <= bs);
        out[pos..pos + 4].copy_from_slice(&e.ino.to_le_bytes());
        out[pos + 4..pos + 6].copy_from_slice(&(r as u16).to_le_bytes());
        out[pos + 6] = e.name.len() as u8;
        out[pos + 7] = e.file_type;
        out[pos + 8..pos + 8 + e.name.len()].copy_from_slice(e.name);
        last = Some(pos);
        pos += r;
    }
    match last {
        Some(l) => {
            let r = (bs - l) as u16;
            out[l + 4..l + 6].copy_from_slice(&r.to_le_bytes());
        }
        None => {
            out[0..4].copy_from_slice(&0u32.to_le_bytes());
            out[4..6].copy_from_slice(&(bs as u16).to_le_bytes());
            out[6] = 0;
            out[7] = 0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rec_lens() {
        assert_eq!(rec_len(1), 12);
        assert_eq!(rec_len(2), 12);
        assert_eq!(rec_len(4), 12);
        assert_eq!(rec_len(5), 16);
        assert_eq!(rec_len(255), 264);
    }

    #[test]
    fn packing() {
        // . and .. take 24 bytes; 4096-byte blocks hold 24 + 339*12 = 4092
        let lens = [1usize, 2].into_iter().chain(std::iter::repeat_n(3, 339));
        assert_eq!(pack(lens), vec![0]);
        let lens = [1usize, 2].into_iter().chain(std::iter::repeat_n(3, 340));
        assert_eq!(pack(lens), vec![0, 341]);
        let lens = [1usize, 2].into_iter().chain(std::iter::repeat_n(255, 20));
        // 24 + 15*264 = 3984; a 16th does not fit
        assert_eq!(pack(lens), vec![0, 17]);
    }

    #[test]
    fn block_bytes() {
        let mut b = vec![0u8; 4096];
        write_block(
            &mut b,
            [Entry { ino: 2, name: b".", file_type: 2 }, Entry { ino: 2, name: b"..", file_type: 2 }].into_iter(),
        );
        assert_eq!(&b[0..12], &[2, 0, 0, 0, 12, 0, 1, 2, b'.', 0, 0, 0]);
        assert_eq!(&b[12..24], &[2, 0, 0, 0, 0xF4, 0x0F, 2, 2, b'.', b'.', 0, 0]);
        let mut e = vec![0u8; 4096];
        write_block(&mut e, std::iter::empty());
        assert_eq!(&e[0..8], &[0, 0, 0, 0, 0, 0x10, 0, 0]);
    }
}

//! On-disk structures (SPEC §8.1–§8.4, §8.8, §8.9): superblock, group
//! descriptors and inodes, serialised little-endian.

pub const EXT2_MAGIC: u16 = 0xEF53;
pub const INODE_SIZE: usize = 128;
pub const FIRST_INO: u32 = 11;
pub const ROOT_INO: u32 = 2;

pub const FEATURE_INCOMPAT_FILETYPE: u32 = 0x0002;
pub const FEATURE_RO_COMPAT_SPARSE_SUPER: u32 = 0x0001;
pub const FEATURE_RO_COMPAT_LARGE_FILE: u32 = 0x0002;

#[inline]
pub fn put16(b: &mut [u8], off: usize, v: u16) {
    b[off..off + 2].copy_from_slice(&v.to_le_bytes());
}
#[inline]
pub fn put32(b: &mut [u8], off: usize, v: u32) {
    b[off..off + 4].copy_from_slice(&v.to_le_bytes());
}
#[inline]
pub fn get16(b: &[u8], off: usize) -> u16 {
    u16::from_le_bytes(b[off..off + 2].try_into().unwrap())
}
#[inline]
pub fn get32(b: &[u8], off: usize) -> u32 {
    u32::from_le_bytes(b[off..off + 4].try_into().unwrap())
}

/// Superblock field values (R-FMT-3).
#[derive(Debug, Clone, Default)]
pub struct Super {
    pub inodes_count: u32,
    pub blocks_count: u32,
    pub free_blocks_count: u32,
    pub free_inodes_count: u32,
    pub blocks_per_group: u32,
    pub inodes_per_group: u32,
    pub wtime: u32,
    pub feature_ro_compat: u32,
    pub volume_name: [u8; 16],
}

/// `s_log_block_size` for 4 KiB blocks.
const LOG_BLOCK_SIZE: u32 = 2;

impl Super {
    /// Serialise as the 1024-byte superblock of group `group_nr`.
    pub fn to_bytes(&self, group_nr: u16) -> [u8; 1024] {
        let mut b = [0u8; 1024];
        put32(&mut b, 0x00, self.inodes_count);
        put32(&mut b, 0x04, self.blocks_count);
        put32(&mut b, 0x08, 0); // s_r_blocks_count: no reserved blocks
        put32(&mut b, 0x0C, self.free_blocks_count);
        put32(&mut b, 0x10, self.free_inodes_count);
        put32(&mut b, 0x14, 0); // s_first_data_block
        put32(&mut b, 0x18, LOG_BLOCK_SIZE);
        put32(&mut b, 0x1C, LOG_BLOCK_SIZE);
        put32(&mut b, 0x20, self.blocks_per_group);
        put32(&mut b, 0x24, self.blocks_per_group);
        put32(&mut b, 0x28, self.inodes_per_group);
        put32(&mut b, 0x2C, 0); // s_mtime
        put32(&mut b, 0x30, self.wtime);
        put16(&mut b, 0x34, 0); // s_mnt_count
        put16(&mut b, 0x36, 0xFFFF); // s_max_mnt_count = -1
        put16(&mut b, 0x38, EXT2_MAGIC);
        put16(&mut b, 0x3A, 1); // EXT2_VALID_FS
        put16(&mut b, 0x3C, 1); // EXT2_ERRORS_CONTINUE
        put16(&mut b, 0x3E, 0); // minor rev
        put32(&mut b, 0x40, self.wtime); // s_lastcheck
        put32(&mut b, 0x44, 0); // s_checkinterval
        put32(&mut b, 0x48, 0); // s_creator_os = Linux
        put32(&mut b, 0x4C, 1); // EXT2_DYNAMIC_REV
        put16(&mut b, 0x50, 0);
        put16(&mut b, 0x52, 0);
        put32(&mut b, 0x54, FIRST_INO);
        put16(&mut b, 0x58, INODE_SIZE as u16);
        put16(&mut b, 0x5A, group_nr);
        put32(&mut b, 0x5C, 0);
        put32(&mut b, 0x60, FEATURE_INCOMPAT_FILETYPE);
        put32(&mut b, 0x64, self.feature_ro_compat);
        // s_uuid (0x68..0x78) stays zero: images are reproducible
        b[0x78..0x88].copy_from_slice(&self.volume_name);
        b
    }
}

/// Group descriptor values (R-FMT-6).
#[derive(Debug, Clone, Copy, Default)]
pub struct GroupDesc {
    pub block_bitmap: u32,
    pub inode_bitmap: u32,
    pub inode_table: u32,
    pub free_blocks: u16,
    pub free_inodes: u16,
    pub used_dirs: u16,
}

impl GroupDesc {
    pub fn write(&self, b: &mut [u8]) {
        put32(b, 0, self.block_bitmap);
        put32(b, 4, self.inode_bitmap);
        put32(b, 8, self.inode_table);
        put16(b, 12, self.free_blocks);
        put16(b, 14, self.free_inodes);
        put16(b, 16, self.used_dirs);
    }
}

/// Inode field values (R-FMT-10).
#[derive(Debug, Clone, Default)]
pub struct Inode {
    pub mode: u16,
    pub uid: u32,
    pub gid: u32,
    pub size: u64,
    pub time: u32,
    pub links: u16,
    pub blocks512: u32,
    pub block: [u32; 15],
    /// Fast symlink target (stored in `i_block` instead of `block`).
    pub fast_link: Option<Vec<u8>>,
    /// Regular file (size high word in `i_size_high`).
    pub regular: bool,
}

impl Inode {
    pub fn write(&self, b: &mut [u8]) {
        put16(b, 0, self.mode);
        put16(b, 2, (self.uid & 0xFFFF) as u16);
        put32(b, 4, (self.size & 0xFFFF_FFFF) as u32);
        put32(b, 8, self.time);
        put32(b, 12, self.time);
        put32(b, 16, self.time);
        put32(b, 20, 0);
        put16(b, 24, (self.gid & 0xFFFF) as u16);
        put16(b, 26, self.links);
        put32(b, 28, self.blocks512);
        put32(b, 32, 0);
        put32(b, 36, 0);
        match &self.fast_link {
            Some(t) => {
                b[40..100].fill(0);
                b[40..40 + t.len()].copy_from_slice(t);
            }
            None => {
                for (i, v) in self.block.iter().enumerate() {
                    put32(b, 40 + 4 * i, *v);
                }
            }
        }
        put32(b, 100, 0); // generation
        put32(b, 104, 0); // file_acl
        put32(b, 108, if self.regular { (self.size >> 32) as u32 } else { 0 });
        put32(b, 112, 0);
        b[116..120].fill(0);
        put16(b, 120, (self.uid >> 16) as u16);
        put16(b, 122, (self.gid >> 16) as u16);
        put32(b, 124, 0);
    }
}

/// Device number encoding in `i_block` (R-FMT-18).
pub fn encode_dev(major: u32, minor: u32) -> [u32; 2] {
    if major < 256 && minor < 256 {
        [major * 256 + minor, 0]
    } else {
        [0, (minor & 0xFF) | (major << 8) | ((minor & !0xFF) << 12)]
    }
}

/// Decode `i_block[0..2]` into (major, minor).
pub fn decode_dev(b0: u32, b1: u32) -> (u32, u32) {
    if b0 != 0 {
        ((b0 >> 8) & 0xFF, b0 & 0xFF)
    } else {
        ((b1 & 0xFFF00) >> 8, (b1 & 0xFF) | ((b1 >> 12) & 0xFFF00))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn devices() {
        assert_eq!(encode_dev(1, 3), [259, 0]);
        assert_eq!(encode_dev(0, 0), [0, 0]);
        for (ma, mi) in [(1, 3), (255, 255), (256, 0), (0, 256), (4095, 1048575), (300, 70000), (8, 1)] {
            let e = encode_dev(ma, mi);
            assert_eq!(decode_dev(e[0], e[1]), (ma, mi), "{ma}:{mi}");
        }
    }

    #[test]
    fn superblock_layout() {
        let s = Super { inodes_count: 16, blocks_count: 10, volume_name: *b"0123456789abcdef", ..Default::default() };
        let b = s.to_bytes(3);
        assert_eq!(get32(&b, 0), 16);
        assert_eq!(get16(&b, 0x38), 0xEF53);
        assert_eq!(get16(&b, 0x36), 0xFFFF);
        assert_eq!(get16(&b, 0x5A), 3);
        assert_eq!(get32(&b, 0x4C), 1);
        assert_eq!(&b[0x78..0x88], b"0123456789abcdef");
        assert!(b[0x68..0x78].iter().all(|&x| x == 0));
        assert!(b[0x108..].iter().all(|&x| x == 0));
    }

    #[test]
    fn inode_layout() {
        let i = Inode {
            mode: 0x81A4,
            uid: 100000,
            gid: 70000,
            size: (5u64 << 32) + 7,
            time: 9,
            links: 1,
            blocks512: 2,
            regular: true,
            ..Default::default()
        };
        let mut b = [0u8; 128];
        i.write(&mut b);
        assert_eq!(get16(&b, 2), (100000 & 0xFFFF) as u16);
        assert_eq!(get16(&b, 120), (100000 >> 16) as u16);
        assert_eq!(get16(&b, 122), (70000 >> 16) as u16);
        assert_eq!(get32(&b, 4), 7);
        assert_eq!(get32(&b, 108), 5);
        let l = Inode { mode: 0xA1FF, size: 3, fast_link: Some(b"abc".to_vec()), ..Default::default() };
        l.write(&mut b);
        assert_eq!(&b[40..44], b"abc\0");
        assert_eq!(get32(&b, 108), 0);
    }
}

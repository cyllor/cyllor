// Minimal ext4 read-only driver
// Supports: superblock, block groups, inodes, directory entries, extents

use alloc::string::String;
use alloc::vec::Vec;
use crate::drivers::virtio::block;

const EXT4_SUPER_MAGIC: u16 = 0xEF53;
const EXT4_ROOT_INO: u32 = 2;

#[repr(C)]
#[derive(Clone, Copy)]
struct Superblock {
    s_inodes_count: u32,
    s_blocks_count_lo: u32,
    s_r_blocks_count_lo: u32,
    s_free_blocks_count_lo: u32,
    s_free_inodes_count: u32,
    s_first_data_block: u32,
    s_log_block_size: u32,
    s_log_cluster_size: u32,
    s_blocks_per_group: u32,
    s_clusters_per_group: u32,
    s_inodes_per_group: u32,
    s_mtime: u32,
    s_wtime: u32,
    s_mnt_count: u16,
    s_max_mnt_count: u16,
    s_magic: u16,
    s_state: u16,
    s_errors: u16,
    s_minor_rev_level: u16,
    s_lastcheck: u32,
    s_checkinterval: u32,
    s_creator_os: u32,
    s_rev_level: u32,
    s_def_resuid: u16,
    s_def_resgid: u16,
    // ext4 specific
    s_first_ino: u32,
    s_inode_size: u16,
    s_block_group_nr: u16,
    s_feature_compat: u32,
    s_feature_incompat: u32,
    s_feature_ro_compat: u32,
    _pad: [u8; 140], // rest of superblock up to 256 bytes
}

#[repr(C)]
#[derive(Clone, Copy)]
struct BlockGroupDesc {
    bg_block_bitmap_lo: u32,
    bg_inode_bitmap_lo: u32,
    bg_inode_table_lo: u32,
    bg_free_blocks_count_lo: u16,
    bg_free_inodes_count_lo: u16,
    bg_used_dirs_count_lo: u16,
    bg_flags: u16,
    _pad: [u8; 48], // 64-byte descriptor
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Inode {
    i_mode: u16,
    i_uid: u16,
    i_size_lo: u32,
    i_atime: u32,
    i_ctime: u32,
    i_mtime: u32,
    i_dtime: u32,
    i_gid: u16,
    i_links_count: u16,
    i_blocks_lo: u32,
    i_flags: u32,
    i_osd1: u32,
    i_block: [u32; 15], // 60 bytes: extent tree or block pointers
    i_generation: u32,
    i_file_acl_lo: u32,
    i_size_high: u32,
    _pad: [u8; 28],
}

// Extent header
#[repr(C)]
#[derive(Clone, Copy)]
struct ExtentHeader {
    eh_magic: u16,   // 0xF30A
    eh_entries: u16,
    eh_max: u16,
    eh_depth: u16,
    eh_generation: u32,
}

// Extent leaf
#[repr(C)]
#[derive(Clone, Copy)]
struct Extent {
    ee_block: u32,    // logical block
    ee_len: u16,      // length
    ee_start_hi: u16, // physical block high
    ee_start_lo: u32, // physical block low
}

// Extent index (internal node)
#[repr(C)]
#[derive(Clone, Copy)]
struct ExtentIdx {
    ei_block: u32,
    ei_leaf_lo: u32,
    ei_leaf_hi: u16,
    _pad: u16,
}

// Directory entry
#[repr(C)]
struct DirEntry {
    inode: u32,
    rec_len: u16,
    name_len: u8,
    file_type: u8,
    // name follows (name_len bytes)
}

const S_IFMT: u16 = 0o170000;
const S_IFDIR: u16 = 0o040000;
const S_IFREG: u16 = 0o100000;
const S_IFLNK: u16 = 0o120000;

pub struct Ext4Fs {
    block_size: usize,
    sb: Superblock,
    desc_size: usize,
}

impl Ext4Fs {
    pub fn mount() -> Result<Self, &'static str> {
        // Read superblock at byte offset 1024 (sector 2)
        let mut buf = [0u8; 1024];
        block::read_sectors(2, 2, &mut buf)?;

        let sb = unsafe { *(buf.as_ptr() as *const Superblock) };
        if sb.s_magic != EXT4_SUPER_MAGIC {
            return Err("Not ext4 (bad magic)");
        }

        let block_size = 1024 << sb.s_log_block_size;
        let desc_size = if sb.s_feature_incompat & 0x80 != 0 { 64 } else { 32 }; // 64-bit feature

        log::info!("ext4: block_size={} inodes={} blocks={}",
            block_size, sb.s_inodes_count, sb.s_blocks_count_lo);

        Ok(Self { block_size, sb, desc_size })
    }

    /// Read a block from disk
    fn read_block(&self, block_num: u64, buf: &mut [u8]) -> Result<(), &'static str> {
        let sectors_per_block = self.block_size / 512;
        let start_sector = block_num * sectors_per_block as u64;
        block::read_sectors(start_sector, sectors_per_block, buf)
    }

    /// Read a specific inode
    fn read_inode(&self, ino: u32) -> Result<Inode, &'static str> {
        let group = ((ino - 1) / self.sb.s_inodes_per_group) as usize;
        let index = ((ino - 1) % self.sb.s_inodes_per_group) as usize;

        // Read block group descriptor
        let desc_block = if self.block_size == 1024 { 2 } else { 1 };
        let desc_offset = group * self.desc_size;
        let desc_block_num = desc_block + (desc_offset / self.block_size) as u64;

        let mut block_buf = alloc::vec![0u8; self.block_size];
        self.read_block(desc_block_num, &mut block_buf)?;

        let bgd = unsafe {
            *(block_buf[desc_offset % self.block_size..].as_ptr() as *const BlockGroupDesc)
        };

        // Read inode from inode table
        let inode_size = self.sb.s_inode_size as usize;
        let inode_offset = index * inode_size;
        let inode_block = bgd.bg_inode_table_lo as u64 + (inode_offset / self.block_size) as u64;

        self.read_block(inode_block, &mut block_buf)?;
        let inode = unsafe {
            *(block_buf[inode_offset % self.block_size..].as_ptr() as *const Inode)
        };

        Ok(inode)
    }

    /// Get file size from inode
    fn inode_size(&self, inode: &Inode) -> u64 {
        (inode.i_size_lo as u64) | ((inode.i_size_high as u64) << 32)
    }

    /// Read file data blocks using extent tree
    fn read_extents(&self, inode: &Inode, offset: usize, buf: &mut [u8]) -> Result<usize, &'static str> {
        let i_block = &inode.i_block;
        let header = unsafe { *(i_block.as_ptr() as *const ExtentHeader) };

        if header.eh_magic != 0xF30A {
            // Not extent-based — try direct block pointers (ext2 style)
            return self.read_direct_blocks(inode, offset, buf);
        }

        if header.eh_depth == 0 {
            // Leaf extents
            let extents = unsafe {
                core::slice::from_raw_parts(
                    (i_block.as_ptr() as *const ExtentHeader).add(1) as *const Extent,
                    header.eh_entries as usize,
                )
            };
            return self.read_from_extents(extents, offset, buf);
        }

        // Depth > 0: traverse index nodes
        let indices = unsafe {
            core::slice::from_raw_parts(
                (i_block.as_ptr() as *const ExtentHeader).add(1) as *const ExtentIdx,
                header.eh_entries as usize,
            )
        };

        // Collect all leaf extents from all index nodes
        let mut all_extents: Vec<Extent> = Vec::new();
        for idx in indices {
            let leaf_block = (idx.ei_leaf_lo as u64) | ((idx.ei_leaf_hi as u64) << 32);
            let mut leaf_buf = alloc::vec![0u8; self.block_size];
            self.read_block(leaf_block, &mut leaf_buf)?;

            let leaf_header = unsafe { *(leaf_buf.as_ptr() as *const ExtentHeader) };
            if leaf_header.eh_magic != 0xF30A || leaf_header.eh_depth != 0 {
                continue;
            }

            let extents = unsafe {
                core::slice::from_raw_parts(
                    (leaf_buf.as_ptr() as *const ExtentHeader).add(1) as *const Extent,
                    leaf_header.eh_entries as usize,
                )
            };
            all_extents.extend_from_slice(extents);
        }

        self.read_from_extents(&all_extents, offset, buf)
    }

    fn read_from_extents(&self, extents: &[Extent], offset: usize, buf: &mut [u8]) -> Result<usize, &'static str> {
        let mut total_read = 0;
        let mut remaining = buf.len();
        let mut file_offset = offset;

        for ext in extents {
            let ext_start = ext.ee_block as usize * self.block_size;
            let ext_len = ext.ee_len as usize * self.block_size;
            let ext_end = ext_start + ext_len;
            let phys_block = (ext.ee_start_lo as u64) | ((ext.ee_start_hi as u64) << 32);

            if file_offset >= ext_end {
                continue;
            }
            if file_offset < ext_start {
                break;
            }

            let offset_in_ext = file_offset - ext_start;
            let available = ext_len - offset_in_ext;
            let to_read = remaining.min(available);

            let phys_offset = offset_in_ext;
            let start_block = phys_block + (phys_offset / self.block_size) as u64;
            let block_offset = phys_offset % self.block_size;

            let mut block_buf = alloc::vec![0u8; self.block_size];
            let mut read_so_far = 0;
            let mut cur_block = start_block;
            let mut cur_offset = block_offset;

            while read_so_far < to_read {
                self.read_block(cur_block, &mut block_buf)?;
                let chunk = (self.block_size - cur_offset).min(to_read - read_so_far);
                buf[total_read + read_so_far..total_read + read_so_far + chunk]
                    .copy_from_slice(&block_buf[cur_offset..cur_offset + chunk]);
                read_so_far += chunk;
                cur_block += 1;
                cur_offset = 0;
            }

            total_read += to_read;
            remaining -= to_read;
            file_offset += to_read;
        }

        Ok(total_read)
    }

    fn read_direct_blocks(&self, inode: &Inode, offset: usize, buf: &mut [u8]) -> Result<usize, &'static str> {
        let mut total = 0;
        let mut block_buf = alloc::vec![0u8; self.block_size];
        let mut file_off = offset;
        let mut remaining = buf.len();

        for i in 0..12 {
            let block_num = inode.i_block[i];
            if block_num == 0 { break; }

            let block_start = i * self.block_size;
            let block_end = block_start + self.block_size;

            if file_off >= block_end { continue; }
            if file_off < block_start { break; }

            self.read_block(block_num as u64, &mut block_buf)?;
            let off_in_block = file_off - block_start;
            let chunk = (self.block_size - off_in_block).min(remaining);
            buf[total..total + chunk].copy_from_slice(&block_buf[off_in_block..off_in_block + chunk]);
            total += chunk;
            remaining -= chunk;
            file_off += chunk;

            if remaining == 0 { break; }
        }

        Ok(total)
    }

    /// List directory entries
    pub fn read_dir(&self, ino: u32) -> Result<Vec<(String, u32, u8)>, &'static str> {
        let inode = self.read_inode(ino)?;
        let size = self.inode_size(&inode) as usize;
        let mut data = alloc::vec![0u8; size];
        let read = self.read_extents(&inode, 0, &mut data)?;
        if ino != EXT4_ROOT_INO {
            log::debug!("ext4 read_dir ino={}: size={} read={}", ino, size, read);
        }

        let mut entries = Vec::new();
        let mut pos = 0;
        while pos + 8 <= data.len() {
            let de = unsafe { &*(data[pos..].as_ptr() as *const DirEntry) };
            if de.rec_len == 0 {
                break;
            }
            if de.inode == 0 {
                // htree internal node or deleted entry — skip to next block boundary
                let next_block = ((pos / self.block_size) + 1) * self.block_size;
                if next_block >= size { break; }
                pos = next_block;
                continue;
            }
            if de.name_len > 0 && (pos + 8 + de.name_len as usize) <= data.len() {
                let name = core::str::from_utf8(&data[pos + 8..pos + 8 + de.name_len as usize])
                    .unwrap_or("?");
                entries.push((String::from(name), de.inode, de.file_type));
            }
            pos += de.rec_len as usize;
            if pos >= size { break; }
        }

        Ok(entries)
    }

    /// Read entire file by inode number
    pub fn read_file(&self, ino: u32) -> Result<Vec<u8>, &'static str> {
        let inode = self.read_inode(ino)?;
        let size = self.inode_size(&inode) as usize;
        let mut data = alloc::vec![0u8; size];
        let read = self.read_extents(&inode, 0, &mut data)?;
        data.truncate(read.max(size));
        Ok(data)
    }

    /// Lookup a path and return inode number, following symlinks
    pub fn lookup(&self, path: &str) -> Result<u32, &'static str> {
        self.lookup_with_depth(path, 0)
    }

    fn lookup_with_depth(&self, path: &str, depth: usize) -> Result<u32, &'static str> {
        self.lookup_from(EXT4_ROOT_INO, path, depth)
    }

    fn lookup_from(&self, start_ino: u32, path: &str, depth: usize) -> Result<u32, &'static str> {
        if depth > 8 {
            return Err("Too many symlinks");
        }

        let path = if path.starts_with('/') {
            // Absolute path: start from root
            return self.lookup_from(EXT4_ROOT_INO, path.trim_start_matches('/'), depth);
        } else {
            path
        };

        if path.is_empty() {
            return Ok(start_ino);
        }

        let mut current_ino = start_ino;
        let mut parent_ino = start_ino;
        let components: Vec<&str> = path.split('/').filter(|s| !s.is_empty() && *s != ".").collect();

        for (idx, component) in components.iter().enumerate() {
            if *component == ".." {
                // For simplicity, go to root
                current_ino = EXT4_ROOT_INO;
                parent_ino = EXT4_ROOT_INO;
                continue;
            }
            let entries = self.read_dir(current_ino)?;
            let found = entries.iter().find(|(name, _, _)| name == *component);
            match found {
                Some((_, ino, ftype)) => {
                    parent_ino = current_ino;
                    current_ino = *ino;
                    // Check if this is a symlink (ftype 7)
                    if *ftype == 7 {
                        let target = self.read_symlink(current_ino)?;
                        log::debug!("ext4 symlink: {} -> {}", component, target);
                        if target.starts_with('/') {
                            current_ino = self.lookup_from(EXT4_ROOT_INO, target.trim_start_matches('/'), depth + 1)?;
                        } else {
                            // Relative symlink: resolve from parent directory
                            current_ino = self.lookup_from(parent_ino, &target, depth + 1)?;
                        }
                        // After following symlink, remaining components continue from resolved dir
                        if idx + 1 < components.len() {
                            let remaining: String = components[idx+1..].join("/");
                            return self.lookup_from(current_ino, &remaining, depth);
                        }
                    }
                }
                None => return Err("File not found"),
            }
        }

        Ok(current_ino)
    }

    /// Read symlink target from inode
    fn read_symlink(&self, ino: u32) -> Result<String, &'static str> {
        let inode = self.read_inode(ino)?;
        let size = self.inode_size(&inode) as usize;

        if size <= 60 {
            // Fast symlink: target stored inline in i_block
            let bytes = unsafe {
                core::slice::from_raw_parts(
                    inode.i_block.as_ptr() as *const u8,
                    size,
                )
            };
            let s = core::str::from_utf8(bytes).map_err(|_| "Invalid symlink")?;
            Ok(String::from(s))
        } else {
            // Slow symlink: target stored in data blocks
            let mut data = alloc::vec![0u8; size];
            self.read_extents(&inode, 0, &mut data)?;
            let s = core::str::from_utf8(&data).map_err(|_| "Invalid symlink")?;
            Ok(String::from(s))
        }
    }

    /// Read a file by path
    pub fn read_file_by_path(&self, path: &str) -> Result<Vec<u8>, &'static str> {
        let ino = self.lookup(path).map_err(|e| {
            log::debug!("ext4 lookup '{}': {}", path, e);
            e
        })?;
        self.read_file(ino)
    }

    /// Check if inode is a directory
    pub fn is_dir(&self, ino: u32) -> Result<bool, &'static str> {
        let inode = self.read_inode(ino)?;
        Ok(inode.i_mode & S_IFMT == S_IFDIR)
    }

    /// Get inode mode
    pub fn inode_mode(&self, ino: u32) -> Result<u16, &'static str> {
        let inode = self.read_inode(ino)?;
        Ok(inode.i_mode)
    }

    /// Get file size by inode
    pub fn file_size(&self, ino: u32) -> Result<u64, &'static str> {
        let inode = self.read_inode(ino)?;
        Ok(self.inode_size(&inode))
    }
}

static EXT4: spin::Mutex<Option<Ext4Fs>> = spin::Mutex::new(None);

pub fn mount() -> Result<(), &'static str> {
    let fs = Ext4Fs::mount()?;
    *EXT4.lock() = Some(fs);
    Ok(())
}

pub fn read_file(path: &str) -> Result<Vec<u8>, &'static str> {
    let fs = EXT4.lock();
    let fs = fs.as_ref().ok_or("ext4 not mounted")?;
    fs.read_file_by_path(path)
}

pub fn list_dir(path: &str) -> Result<Vec<(String, u32, u8)>, &'static str> {
    let fs = EXT4.lock();
    let fs = fs.as_ref().ok_or("ext4 not mounted")?;
    let ino = fs.lookup(path)?;
    fs.read_dir(ino)
}

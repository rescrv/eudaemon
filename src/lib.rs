//! A Log-structured File System (LFS) implementation.
//!
//! This module provides a simple LFS that operates on an in-memory byte buffer.
//! The filesystem supports files up to 1/4 the size of the total filesystem.
//!
//! # Design
//!
//! The filesystem is organized as follows:
//! - Superblock: Fixed at offset 0, contains metadata about the filesystem
//! - Log: Sequential writes starting after the superblock, wraps around
//! - Inode Map: Maps inode numbers to their current location in the log
//! - Segment Summaries: Track which blocks belong to which inodes for cleaning
//!
//! # Operations
//!
//! - `open`: Open or create a file by name
//! - `close`: Close a file descriptor
//! - `read`: Read data from an open file
//! - `write`: Write data to an open file
//! - `truncate`: Change the size of a file
//! - `clean`: Perform log cleaning (garbage collection)

#![deny(missing_docs)]

use std::collections::BTreeMap;

/// Block size in bytes.
const BLOCK_SIZE: usize = 4096;

/// Magic number for the superblock.
const MAGIC: u64 = 0x4C46535F53594E46; // "LFS_SYNF"

/// Maximum filename length.
const MAX_FILENAME_LEN: usize = 255;

/// Maximum number of direct block pointers in an inode.
const DIRECT_BLOCKS: usize = 12;

/// Number of block pointers per indirect block.
const PTRS_PER_BLOCK: usize = BLOCK_SIZE / 8;

/// Size of a directory entry in bytes.
const DIR_ENTRY_SIZE: u64 = 272;

////////////////////////////////////////////// InodeNumber /////////////////////////////////////////////

/// A strongly-typed inode number.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct InodeNumber(u64);

impl InodeNumber {
    /// The root directory inode number.
    pub const ROOT: InodeNumber = InodeNumber(1);

    /// The invalid/null inode number.
    pub const INVALID: InodeNumber = InodeNumber(0);

    /// Creates a new inode number from a raw value.
    fn new(value: u64) -> Self {
        Self(value)
    }

    /// Returns the raw u64 value.
    fn as_u64(self) -> u64 {
        self.0
    }

    /// Returns true if this is a valid inode number.
    fn is_valid(self) -> bool {
        self != Self::INVALID
    }

    /// Returns the next inode number.
    fn next(self) -> Self {
        Self(self.0 + 1)
    }
}

///////////////////////////////////////////// BlockAddress /////////////////////////////////////////////

/// A strongly-typed block address on disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BlockAddress(u64);

impl BlockAddress {
    /// The invalid/null block address.
    pub const INVALID: BlockAddress = BlockAddress(u64::MAX);

    /// Creates a new block address from a raw value.
    fn new(value: u64) -> Self {
        Self(value)
    }

    /// Returns the raw u64 value.
    fn as_u64(self) -> u64 {
        self.0
    }

    /// Returns true if this is a valid block address.
    fn is_valid(self) -> bool {
        self != Self::INVALID
    }

    /// Returns the byte offset in the data buffer for this block.
    fn byte_offset(self) -> usize {
        (self.0 as usize).saturating_mul(BLOCK_SIZE)
    }

    /// Returns the next block address.
    fn next(self) -> Self {
        Self(self.0 + 1)
    }
}

////////////////////////////////////////////// BlockIndex //////////////////////////////////////////////

/// A strongly-typed block index within a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BlockIndex(u64);

impl BlockIndex {
    /// Creates a new block index from a raw value.
    fn new(value: u64) -> Self {
        Self(value)
    }

    /// Returns the raw u64 value.
    fn as_u64(self) -> u64 {
        self.0
    }

    /// Computes the block index for a given byte offset within a file.
    fn from_byte_offset(offset: u64) -> Self {
        Self(offset / BLOCK_SIZE as u64)
    }

    /// Returns the number of blocks needed to store `size` bytes.
    fn blocks_for_size(size: u64) -> u64 {
        if size == 0 {
            0
        } else {
            (size - 1) / BLOCK_SIZE as u64 + 1
        }
    }
}

/////////////////////////////////////////// FileDescriptor /////////////////////////////////////////////

/// A strongly-typed file descriptor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FileDescriptor(u32);

impl FileDescriptor {
    /// Creates a new file descriptor from a raw value.
    fn new(value: u32) -> Self {
        Self(value)
    }

    /// Returns the next file descriptor.
    fn next(self) -> Self {
        Self(self.0 + 1)
    }
}

impl std::fmt::Display for FileDescriptor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "fd:{}", self.0)
    }
}

//////////////////////////////////////////////// Error /////////////////////////////////////////////////

/// Error type for filesystem operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// The filesystem is corrupt or invalid.
    CorruptFilesystem,
    /// The filesystem is full.
    NoSpace,
    /// The file was not found.
    NotFound,
    /// The file descriptor is invalid.
    InvalidFd,
    /// The filename is too long.
    FilenameTooLong,
    /// The file already exists.
    AlreadyExists,
    /// Invalid offset for read/write.
    InvalidOffset,
    /// The file is too large.
    FileTooLarge,
    /// The buffer is too small for the filesystem.
    BufferTooSmall,
    /// Invalid argument.
    InvalidArgument,
    /// The file is not open for the requested operation.
    NotOpen,
}

/// Result type for filesystem operations.
pub type Result<T> = std::result::Result<T, Error>;

////////////////////////////////////////////// Superblock //////////////////////////////////////////////

/// Superblock structure stored at offset 0.
#[derive(Debug, Clone, Copy)]
struct Superblock {
    magic: u64,
    block_size: u32,
    total_blocks: u64,
    log_start: BlockAddress,
    log_end: BlockAddress,
    tail: BlockAddress,
    inode_map_block: BlockAddress,
    next_inode: InodeNumber,
}

impl Superblock {
    fn as_bytes(self) -> [u8; 64] {
        let mut buf = [0u8; 64];
        buf[0..8].copy_from_slice(&self.magic.to_le_bytes());
        buf[8..12].copy_from_slice(&self.block_size.to_le_bytes());
        buf[16..24].copy_from_slice(&self.total_blocks.to_le_bytes());
        buf[24..32].copy_from_slice(&self.log_start.as_u64().to_le_bytes());
        buf[32..40].copy_from_slice(&self.log_end.as_u64().to_le_bytes());
        buf[40..48].copy_from_slice(&self.tail.as_u64().to_le_bytes());
        buf[48..56].copy_from_slice(&self.inode_map_block.as_u64().to_le_bytes());
        buf[56..64].copy_from_slice(&self.next_inode.as_u64().to_le_bytes());
        buf
    }

    fn from_bytes(buf: &[u8]) -> Option<Self> {
        if buf.len() < 64 {
            return None;
        }
        Some(Self {
            magic: u64::from_le_bytes(buf[0..8].try_into().ok()?),
            block_size: u32::from_le_bytes(buf[8..12].try_into().ok()?),
            total_blocks: u64::from_le_bytes(buf[16..24].try_into().ok()?),
            log_start: BlockAddress::new(u64::from_le_bytes(buf[24..32].try_into().ok()?)),
            log_end: BlockAddress::new(u64::from_le_bytes(buf[32..40].try_into().ok()?)),
            tail: BlockAddress::new(u64::from_le_bytes(buf[40..48].try_into().ok()?)),
            inode_map_block: BlockAddress::new(u64::from_le_bytes(buf[48..56].try_into().ok()?)),
            next_inode: InodeNumber::new(u64::from_le_bytes(buf[56..64].try_into().ok()?)),
        })
    }
}

//////////////////////////////////////////////// Inode /////////////////////////////////////////////////

/// On-disk inode structure.
#[derive(Debug, Clone)]
struct Inode {
    ino: InodeNumber,
    size: u64,
    direct: [BlockAddress; DIRECT_BLOCKS],
    indirect: BlockAddress,
    double_indirect: BlockAddress,
}

impl Inode {
    fn new(ino: InodeNumber) -> Self {
        Self {
            ino,
            size: 0,
            direct: [BlockAddress::INVALID; DIRECT_BLOCKS],
            indirect: BlockAddress::INVALID,
            double_indirect: BlockAddress::INVALID,
        }
    }

    fn to_bytes(&self) -> [u8; 128] {
        let mut buf = [0u8; 128];
        buf[0..8].copy_from_slice(&self.ino.as_u64().to_le_bytes());
        buf[8..16].copy_from_slice(&self.size.to_le_bytes());
        for (i, &block) in self.direct.iter().enumerate() {
            let offset = 16 + i * 8;
            buf[offset..offset + 8].copy_from_slice(&block.as_u64().to_le_bytes());
        }
        buf[112..120].copy_from_slice(&self.indirect.as_u64().to_le_bytes());
        buf[120..128].copy_from_slice(&self.double_indirect.as_u64().to_le_bytes());
        buf
    }

    fn from_bytes(buf: &[u8]) -> Option<Self> {
        if buf.len() < 128 {
            return None;
        }
        let mut direct = [BlockAddress::INVALID; DIRECT_BLOCKS];
        for (i, block) in direct.iter_mut().enumerate() {
            let offset = 16 + i * 8;
            *block =
                BlockAddress::new(u64::from_le_bytes(buf[offset..offset + 8].try_into().ok()?));
        }
        Some(Self {
            ino: InodeNumber::new(u64::from_le_bytes(buf[0..8].try_into().ok()?)),
            size: u64::from_le_bytes(buf[8..16].try_into().ok()?),
            direct,
            indirect: BlockAddress::new(u64::from_le_bytes(buf[112..120].try_into().ok()?)),
            double_indirect: BlockAddress::new(u64::from_le_bytes(buf[120..128].try_into().ok()?)),
        })
    }
}

////////////////////////////////////////////// DirEntry ////////////////////////////////////////////////

/// Directory entry.
#[derive(Debug, Clone)]
struct DirEntry {
    ino: InodeNumber,
    name_len: u16,
    name: [u8; MAX_FILENAME_LEN],
}

impl DirEntry {
    fn new(ino: InodeNumber, name: &str) -> Option<Self> {
        if name.len() > MAX_FILENAME_LEN {
            return None;
        }
        let mut entry = Self {
            ino,
            name_len: name.len() as u16,
            name: [0; MAX_FILENAME_LEN],
        };
        entry.name[..name.len()].copy_from_slice(name.as_bytes());
        Some(entry)
    }

    fn name(&self) -> &str {
        std::str::from_utf8(&self.name[..self.name_len as usize]).unwrap_or("")
    }

    fn to_bytes(&self) -> [u8; 272] {
        let mut buf = [0u8; 272];
        buf[0..8].copy_from_slice(&self.ino.as_u64().to_le_bytes());
        buf[8..10].copy_from_slice(&self.name_len.to_le_bytes());
        buf[10..10 + MAX_FILENAME_LEN].copy_from_slice(&self.name);
        buf
    }

    fn from_bytes(buf: &[u8]) -> Option<Self> {
        if buf.len() < 272 {
            return None;
        }
        let mut name = [0u8; MAX_FILENAME_LEN];
        name.copy_from_slice(&buf[10..10 + MAX_FILENAME_LEN]);
        Some(Self {
            ino: InodeNumber::new(u64::from_le_bytes(buf[0..8].try_into().ok()?)),
            name_len: u16::from_le_bytes(buf[8..10].try_into().ok()?),
            name,
        })
    }
}

////////////////////////////////////////// SegmentEntryType ////////////////////////////////////////////

/// Type of segment summary entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SegmentEntryType {
    /// Data block belonging to a file.
    Data,
    /// Inode block.
    Inode,
    /// Indirect block.
    Indirect,
    /// Inode map block.
    InodeMap,
}

//////////////////////////////////////// SegmentSummaryEntry ///////////////////////////////////////////

/// Segment summary entry, tracking which inode owns each block.
#[derive(Debug, Clone, Copy)]
struct SegmentSummaryEntry {
    ino: InodeNumber,
    block_index: BlockIndex,
    entry_type: SegmentEntryType,
}

////////////////////////////////////////////// OpenFile ////////////////////////////////////////////////

/// Open file descriptor state.
#[derive(Debug, Clone)]
struct OpenFile {
    ino: InodeNumber,
    position: u64,
}

///////////////////////////////////////////////// Lfs //////////////////////////////////////////////////

/// Log-structured File System.
pub struct Lfs {
    data: Vec<u8>,
    superblock: Superblock,
    inode_map: BTreeMap<InodeNumber, BlockAddress>,
    segment_summary: BTreeMap<BlockAddress, SegmentSummaryEntry>,
    open_files: BTreeMap<FileDescriptor, OpenFile>,
    next_fd: FileDescriptor,
    max_file_size: u64,
    /// The committed tail position from the last successful operation.
    /// Used to recover from partial writes on NoSpace errors.
    committed_tail: BlockAddress,
}

impl Lfs {
    /// Creates a new LFS on the given buffer.
    ///
    /// The buffer must be at least 16 blocks (64KB) in size.
    pub fn new(data: Vec<u8>) -> Result<Self> {
        let total_blocks = data.len() / BLOCK_SIZE;
        if total_blocks < 16 {
            return Err(Error::BufferTooSmall);
        }

        let max_file_size = (data.len() / 10) as u64;
        let log_start = BlockAddress::new(1);
        let log_end = BlockAddress::new(total_blocks as u64);

        let superblock = Superblock {
            magic: MAGIC,
            block_size: BLOCK_SIZE as u32,
            total_blocks: total_blocks as u64,
            log_start,
            log_end,
            tail: log_start,
            inode_map_block: BlockAddress::INVALID,
            next_inode: InodeNumber::ROOT.next(),
        };

        let committed_tail = log_start;
        let mut lfs = Self {
            data,
            superblock,
            inode_map: BTreeMap::new(),
            segment_summary: BTreeMap::new(),
            open_files: BTreeMap::new(),
            next_fd: FileDescriptor::new(0),
            max_file_size,
            committed_tail,
        };

        lfs.write_superblock()?;
        lfs.create_root_directory()?;
        lfs.committed_tail = lfs.superblock.tail;

        Ok(lfs)
    }

    /// Opens an existing LFS from the given buffer, using the provided tail offset.
    pub fn open(data: Vec<u8>, tail: BlockAddress) -> Result<Self> {
        if data.len() < BLOCK_SIZE {
            return Err(Error::BufferTooSmall);
        }

        let superblock =
            Superblock::from_bytes(&data[..BLOCK_SIZE]).ok_or(Error::CorruptFilesystem)?;

        if superblock.magic != MAGIC {
            return Err(Error::CorruptFilesystem);
        }

        let max_file_size = (data.len() / 10) as u64;

        let mut lfs = Self {
            data,
            superblock,
            inode_map: BTreeMap::new(),
            segment_summary: BTreeMap::new(),
            open_files: BTreeMap::new(),
            next_fd: FileDescriptor::new(0),
            max_file_size,
            committed_tail: tail,
        };

        lfs.superblock.tail = tail;
        lfs.load_inode_map()?;
        lfs.rebuild_segment_summary()?;

        Ok(lfs)
    }

    /// Returns the current tail offset.
    pub fn tail(&self) -> BlockAddress {
        self.superblock.tail
    }

    /// Returns the underlying data buffer.
    pub fn into_inner(self) -> Vec<u8> {
        self.data
    }

    /// Returns a reference to the underlying data.
    pub fn data(&self) -> &[u8] {
        &self.data
    }

    /// Opens or creates a file by name.
    ///
    /// Returns a file descriptor that can be used for read/write operations.
    pub fn open_file(&mut self, name: &str) -> Result<FileDescriptor> {
        match self.open_file_inner(name) {
            Ok(fd) => {
                self.commit();
                Ok(fd)
            }
            Err(Error::NoSpace) => {
                self.recover_from_partial_write()?;
                Err(Error::NoSpace)
            }
            Err(e) => Err(e),
        }
    }

    fn open_file_inner(&mut self, name: &str) -> Result<FileDescriptor> {
        if name.len() > MAX_FILENAME_LEN {
            return Err(Error::FilenameTooLong);
        }

        let ino = match self.lookup_file(name)? {
            Some(ino) => ino,
            None => self.create_file(name)?,
        };

        let fd = self.next_fd;
        self.next_fd = self.next_fd.next();
        self.open_files.insert(fd, OpenFile { ino, position: 0 });

        Ok(fd)
    }

    /// Closes an open file descriptor.
    pub fn close(&mut self, fd: FileDescriptor) -> Result<()> {
        self.open_files.remove(&fd).ok_or(Error::InvalidFd)?;
        Ok(())
    }

    /// Reads data from an open file.
    ///
    /// Returns the number of bytes read.
    pub fn read(&mut self, fd: FileDescriptor, buf: &mut [u8]) -> Result<usize> {
        let open_file = self.open_files.get(&fd).ok_or(Error::InvalidFd)?.clone();
        let inode = self.read_inode(open_file.ino)?;

        if open_file.position >= inode.size {
            return Ok(0);
        }

        let bytes_available = (inode.size - open_file.position) as usize;
        let bytes_to_read = buf.len().min(bytes_available);

        let mut bytes_read = 0;
        let mut position = open_file.position;

        while bytes_read < bytes_to_read {
            let block_idx = BlockIndex::from_byte_offset(position);
            let block_offset = (position % BLOCK_SIZE as u64) as usize;
            let block_addr = self.get_block_addr(&inode, block_idx)?;

            let bytes_in_block = (BLOCK_SIZE - block_offset).min(bytes_to_read - bytes_read);

            if !block_addr.is_valid() {
                buf[bytes_read..bytes_read + bytes_in_block].fill(0);
            } else {
                let disk_offset = block_addr.byte_offset() + block_offset;
                buf[bytes_read..bytes_read + bytes_in_block]
                    .copy_from_slice(&self.data[disk_offset..disk_offset + bytes_in_block]);
            }

            bytes_read += bytes_in_block;
            position += bytes_in_block as u64;
        }

        self.open_files.get_mut(&fd).unwrap().position = position;
        Ok(bytes_read)
    }

    /// Writes data to an open file.
    ///
    /// Returns the number of bytes written.
    pub fn write(&mut self, fd: FileDescriptor, buf: &[u8]) -> Result<usize> {
        match self.write_inner(fd, buf) {
            Ok(n) => {
                self.commit();
                Ok(n)
            }
            Err(Error::NoSpace) => {
                self.recover_from_partial_write()?;
                Err(Error::NoSpace)
            }
            Err(e) => Err(e),
        }
    }

    fn write_inner(&mut self, fd: FileDescriptor, buf: &[u8]) -> Result<usize> {
        let open_file = self.open_files.get(&fd).ok_or(Error::InvalidFd)?.clone();

        if open_file.position + buf.len() as u64 > self.max_file_size {
            return Err(Error::FileTooLarge);
        }

        let mut inode = self.read_inode(open_file.ino)?;
        let mut bytes_written = 0;
        let mut position = open_file.position;

        while bytes_written < buf.len() {
            let block_idx = BlockIndex::from_byte_offset(position);
            let block_offset = (position % BLOCK_SIZE as u64) as usize;
            let bytes_in_block = (BLOCK_SIZE - block_offset).min(buf.len() - bytes_written);

            let mut block_data = [0u8; BLOCK_SIZE];

            let old_block_addr = self.get_block_addr(&inode, block_idx)?;
            if old_block_addr.is_valid() && bytes_in_block < BLOCK_SIZE {
                let disk_offset = old_block_addr.byte_offset();
                block_data.copy_from_slice(&self.data[disk_offset..disk_offset + BLOCK_SIZE]);
            }

            block_data[block_offset..block_offset + bytes_in_block]
                .copy_from_slice(&buf[bytes_written..bytes_written + bytes_in_block]);

            let new_block_addr = self.allocate_block()?;
            self.write_block(new_block_addr, &block_data)?;
            self.segment_summary.insert(
                new_block_addr,
                SegmentSummaryEntry {
                    ino: inode.ino,
                    block_index: block_idx,
                    entry_type: SegmentEntryType::Data,
                },
            );

            if old_block_addr.is_valid() {
                self.segment_summary.remove(&old_block_addr);
            }

            self.set_block_addr(&mut inode, block_idx, new_block_addr)?;

            bytes_written += bytes_in_block;
            position += bytes_in_block as u64;
        }

        let new_size = position.max(inode.size);
        if new_size != inode.size {
            inode.size = new_size;
        }

        self.write_inode(&inode)?;
        self.open_files.get_mut(&fd).unwrap().position = position;
        self.persist_inode_map()?;

        Ok(bytes_written)
    }

    /// Seeks to a position in an open file.
    pub fn seek(&mut self, fd: FileDescriptor, position: u64) -> Result<()> {
        let open_file = self.open_files.get_mut(&fd).ok_or(Error::InvalidFd)?;
        open_file.position = position;
        Ok(())
    }

    /// Truncates a file to the specified size.
    pub fn truncate(&mut self, fd: FileDescriptor, size: u64) -> Result<()> {
        match self.truncate_inner(fd, size) {
            Ok(()) => {
                self.commit();
                Ok(())
            }
            Err(Error::NoSpace) => {
                self.recover_from_partial_write()?;
                Err(Error::NoSpace)
            }
            Err(e) => Err(e),
        }
    }

    fn truncate_inner(&mut self, fd: FileDescriptor, size: u64) -> Result<()> {
        if size > self.max_file_size {
            return Err(Error::FileTooLarge);
        }

        let open_file = self.open_files.get(&fd).ok_or(Error::InvalidFd)?.clone();
        let mut inode = self.read_inode(open_file.ino)?;

        if size >= inode.size {
            inode.size = size;
            self.write_inode(&inode)?;
            self.persist_inode_map()?;
            return Ok(());
        }

        let new_last_block = BlockIndex::blocks_for_size(size);
        let old_last_block = BlockIndex::blocks_for_size(inode.size);

        for block_num in new_last_block..old_last_block {
            let block_idx = BlockIndex::new(block_num);
            let block_addr = self.get_block_addr(&inode, block_idx)?;
            if block_addr.is_valid() {
                self.segment_summary.remove(&block_addr);
                self.set_block_addr(&mut inode, block_idx, BlockAddress::INVALID)?;
            }
        }

        inode.size = size;
        self.write_inode(&inode)?;
        self.persist_inode_map()?;

        Ok(())
    }

    /// Returns the size of an open file.
    pub fn file_size(&self, fd: FileDescriptor) -> Result<u64> {
        let open_file = self.open_files.get(&fd).ok_or(Error::InvalidFd)?;
        let inode = self.read_inode_from_map(open_file.ino)?;
        Ok(inode.size)
    }

    /// Performs log cleaning (garbage collection).
    ///
    /// This compacts live data and reclaims space from dead blocks.
    /// Returns the number of blocks reclaimed.
    pub fn clean(&mut self) -> Result<usize> {
        match self.clean_inner() {
            Ok(n) => {
                self.commit();
                Ok(n)
            }
            Err(Error::NoSpace) => {
                self.recover_from_partial_write()?;
                Err(Error::NoSpace)
            }
            Err(e) => Err(e),
        }
    }

    fn clean_inner(&mut self) -> Result<usize> {
        let live_blocks: Vec<(BlockAddress, SegmentSummaryEntry)> = self
            .segment_summary
            .iter()
            .filter(|(_, entry)| entry.entry_type == SegmentEntryType::Data)
            .map(|(&addr, &entry)| (addr, entry))
            .collect();

        if live_blocks.is_empty() {
            return Ok(0);
        }

        let mut blocks_by_inode: BTreeMap<InodeNumber, Vec<(BlockAddress, BlockIndex)>> =
            BTreeMap::new();
        for (addr, entry) in &live_blocks {
            blocks_by_inode
                .entry(entry.ino)
                .or_default()
                .push((*addr, entry.block_index));
        }

        let old_tail = self.superblock.tail;
        let mut blocks_reclaimed = 0;

        for (ino, blocks) in blocks_by_inode {
            if !self.inode_map.contains_key(&ino) {
                for (addr, _) in &blocks {
                    self.segment_summary.remove(addr);
                    blocks_reclaimed += 1;
                }
                continue;
            }

            let mut inode = match self.read_inode(ino) {
                Ok(inode) => inode,
                Err(_) => continue,
            };

            for (old_addr, block_index) in blocks {
                let current_addr = self.get_block_addr(&inode, block_index)?;
                if current_addr != old_addr {
                    self.segment_summary.remove(&old_addr);
                    blocks_reclaimed += 1;
                    continue;
                }

                let mut block_data = [0u8; BLOCK_SIZE];
                let disk_offset = old_addr.byte_offset();
                block_data.copy_from_slice(&self.data[disk_offset..disk_offset + BLOCK_SIZE]);

                let new_addr = self.allocate_block()?;
                self.write_block(new_addr, &block_data)?;

                self.segment_summary.remove(&old_addr);
                self.segment_summary.insert(
                    new_addr,
                    SegmentSummaryEntry {
                        ino,
                        block_index,
                        entry_type: SegmentEntryType::Data,
                    },
                );

                self.set_block_addr(&mut inode, block_index, new_addr)?;
            }

            self.write_inode(&inode)?;
        }

        self.persist_inode_map()?;

        if self.superblock.tail.as_u64() > old_tail.as_u64() {
            blocks_reclaimed += (self.superblock.tail.as_u64() - old_tail.as_u64()) as usize;
        }

        Ok(blocks_reclaimed)
    }

    /// Returns the number of free blocks available.
    pub fn free_blocks(&self) -> u64 {
        let total_log_blocks =
            self.superblock.log_end.as_u64() - self.superblock.log_start.as_u64();
        let used_blocks = self.segment_summary.len() as u64;
        total_log_blocks.saturating_sub(used_blocks)
    }

    /// Recovers from a partial write by reloading in-memory state from disk.
    ///
    /// This is called when an operation fails with NoSpace. Since LFS writes
    /// contiguously at the tail, we can simply reset to the committed tail
    /// position and reload all in-memory structures.
    fn recover_from_partial_write(&mut self) -> Result<()> {
        self.superblock =
            Superblock::from_bytes(&self.data[..BLOCK_SIZE]).ok_or(Error::CorruptFilesystem)?;
        self.superblock.tail = self.committed_tail;
        self.inode_map.clear();
        self.segment_summary.clear();
        self.load_inode_map()?;
        self.rebuild_segment_summary()?;
        Ok(())
    }

    /// Commits the current state by updating committed_tail to match the superblock.
    fn commit(&mut self) {
        self.committed_tail = self.superblock.tail;
    }

    fn write_superblock(&mut self) -> Result<()> {
        let bytes = self.superblock.as_bytes();
        self.data[..64].copy_from_slice(&bytes);
        Ok(())
    }

    fn create_root_directory(&mut self) -> Result<()> {
        let root_inode = Inode::new(InodeNumber::ROOT);
        self.write_inode(&root_inode)?;
        self.persist_inode_map()?;
        Ok(())
    }

    fn read_inode(&self, ino: InodeNumber) -> Result<Inode> {
        self.read_inode_from_map(ino)
    }

    fn read_inode_from_map(&self, ino: InodeNumber) -> Result<Inode> {
        let block_addr = self.inode_map.get(&ino).ok_or(Error::NotFound)?;
        let disk_offset = block_addr.byte_offset();
        Inode::from_bytes(&self.data[disk_offset..disk_offset + BLOCK_SIZE])
            .ok_or(Error::CorruptFilesystem)
    }

    fn write_inode(&mut self, inode: &Inode) -> Result<()> {
        let block_addr = self.allocate_block()?;
        let mut block_data = [0u8; BLOCK_SIZE];
        block_data[..128].copy_from_slice(&inode.to_bytes());
        self.write_block(block_addr, &block_data)?;

        if let Some(&old_addr) = self.inode_map.get(&inode.ino) {
            self.segment_summary.remove(&old_addr);
        }

        self.inode_map.insert(inode.ino, block_addr);
        self.segment_summary.insert(
            block_addr,
            SegmentSummaryEntry {
                ino: inode.ino,
                block_index: BlockIndex::new(0),
                entry_type: SegmentEntryType::Inode,
            },
        );

        Ok(())
    }

    fn allocate_block(&mut self) -> Result<BlockAddress> {
        let log_size = self.superblock.log_end.as_u64() - self.superblock.log_start.as_u64();
        let reserved_blocks = log_size / 4;
        let used_blocks = self.segment_summary.len() as u64;

        if used_blocks >= log_size.saturating_sub(reserved_blocks) {
            return Err(Error::NoSpace);
        }

        let block = self.superblock.tail;

        if self.segment_summary.contains_key(&block) {
            return Err(Error::NoSpace);
        }

        self.superblock.tail = self.superblock.tail.next();

        if self.superblock.tail.as_u64() >= self.superblock.log_end.as_u64() {
            self.superblock.tail = self.superblock.log_start;
        }

        Ok(block)
    }

    fn write_block(&mut self, block: BlockAddress, data: &[u8; BLOCK_SIZE]) -> Result<()> {
        let offset = block.byte_offset();
        if offset + BLOCK_SIZE > self.data.len() {
            return Err(Error::InvalidOffset);
        }
        self.data[offset..offset + BLOCK_SIZE].copy_from_slice(data);
        Ok(())
    }

    fn get_block_addr(&self, inode: &Inode, block_idx: BlockIndex) -> Result<BlockAddress> {
        let idx = block_idx.as_u64();

        if idx < DIRECT_BLOCKS as u64 {
            return Ok(inode.direct[idx as usize]);
        }

        let indirect_idx = idx - DIRECT_BLOCKS as u64;
        if indirect_idx < PTRS_PER_BLOCK as u64 {
            if !inode.indirect.is_valid() {
                return Ok(BlockAddress::INVALID);
            }
            let disk_offset = inode.indirect.byte_offset() + indirect_idx as usize * 8;
            let ptr =
                u64::from_le_bytes(self.data[disk_offset..disk_offset + 8].try_into().unwrap());
            return Ok(BlockAddress::new(ptr));
        }

        let double_idx = indirect_idx - PTRS_PER_BLOCK as u64;
        if double_idx < (PTRS_PER_BLOCK * PTRS_PER_BLOCK) as u64 {
            if !inode.double_indirect.is_valid() {
                return Ok(BlockAddress::INVALID);
            }

            let first_level_idx = double_idx / PTRS_PER_BLOCK as u64;
            let second_level_idx = double_idx % PTRS_PER_BLOCK as u64;

            let first_offset = inode.double_indirect.byte_offset() + first_level_idx as usize * 8;
            let first_ptr = u64::from_le_bytes(
                self.data[first_offset..first_offset + 8]
                    .try_into()
                    .unwrap(),
            );
            let first_addr = BlockAddress::new(first_ptr);

            if !first_addr.is_valid() {
                return Ok(BlockAddress::INVALID);
            }

            let second_offset = first_addr.byte_offset() + second_level_idx as usize * 8;
            let second_ptr = u64::from_le_bytes(
                self.data[second_offset..second_offset + 8]
                    .try_into()
                    .unwrap(),
            );
            return Ok(BlockAddress::new(second_ptr));
        }

        Err(Error::FileTooLarge)
    }

    fn set_block_addr(
        &mut self,
        inode: &mut Inode,
        block_idx: BlockIndex,
        addr: BlockAddress,
    ) -> Result<()> {
        let idx = block_idx.as_u64();

        if idx < DIRECT_BLOCKS as u64 {
            inode.direct[idx as usize] = addr;
            return Ok(());
        }

        let indirect_idx = idx - DIRECT_BLOCKS as u64;
        if indirect_idx < PTRS_PER_BLOCK as u64 {
            if !inode.indirect.is_valid() {
                let indirect_block = self.allocate_block()?;
                let mut block_data = [0u8; BLOCK_SIZE];
                for chunk in block_data.chunks_exact_mut(8) {
                    chunk.copy_from_slice(&BlockAddress::INVALID.as_u64().to_le_bytes());
                }
                self.write_block(indirect_block, &block_data)?;
                inode.indirect = indirect_block;
                self.segment_summary.insert(
                    indirect_block,
                    SegmentSummaryEntry {
                        ino: inode.ino,
                        block_index: BlockIndex::new(0),
                        entry_type: SegmentEntryType::Indirect,
                    },
                );
            }

            let disk_offset = inode.indirect.byte_offset() + indirect_idx as usize * 8;
            self.data[disk_offset..disk_offset + 8].copy_from_slice(&addr.as_u64().to_le_bytes());
            return Ok(());
        }

        let double_idx = indirect_idx - PTRS_PER_BLOCK as u64;
        if double_idx < (PTRS_PER_BLOCK * PTRS_PER_BLOCK) as u64 {
            if !inode.double_indirect.is_valid() {
                let double_block = self.allocate_block()?;
                let mut block_data = [0u8; BLOCK_SIZE];
                for chunk in block_data.chunks_exact_mut(8) {
                    chunk.copy_from_slice(&BlockAddress::INVALID.as_u64().to_le_bytes());
                }
                self.write_block(double_block, &block_data)?;
                inode.double_indirect = double_block;
                self.segment_summary.insert(
                    double_block,
                    SegmentSummaryEntry {
                        ino: inode.ino,
                        block_index: BlockIndex::new(0),
                        entry_type: SegmentEntryType::Indirect,
                    },
                );
            }

            let first_level_idx = double_idx / PTRS_PER_BLOCK as u64;
            let second_level_idx = double_idx % PTRS_PER_BLOCK as u64;

            let first_offset = inode.double_indirect.byte_offset() + first_level_idx as usize * 8;
            let first_ptr = u64::from_le_bytes(
                self.data[first_offset..first_offset + 8]
                    .try_into()
                    .unwrap(),
            );
            let mut first_addr = BlockAddress::new(first_ptr);

            if !first_addr.is_valid() {
                let new_block = self.allocate_block()?;
                let mut block_data = [0u8; BLOCK_SIZE];
                for chunk in block_data.chunks_exact_mut(8) {
                    chunk.copy_from_slice(&BlockAddress::INVALID.as_u64().to_le_bytes());
                }
                self.write_block(new_block, &block_data)?;
                self.data[first_offset..first_offset + 8]
                    .copy_from_slice(&new_block.as_u64().to_le_bytes());
                first_addr = new_block;
                self.segment_summary.insert(
                    new_block,
                    SegmentSummaryEntry {
                        ino: inode.ino,
                        block_index: BlockIndex::new(0),
                        entry_type: SegmentEntryType::Indirect,
                    },
                );
            }

            let second_offset = first_addr.byte_offset() + second_level_idx as usize * 8;
            self.data[second_offset..second_offset + 8]
                .copy_from_slice(&addr.as_u64().to_le_bytes());
            return Ok(());
        }

        Err(Error::FileTooLarge)
    }

    fn lookup_file(&self, name: &str) -> Result<Option<InodeNumber>> {
        let root_inode = self.read_inode(InodeNumber::ROOT)?;
        self.lookup_in_dir(&root_inode, name)
    }

    fn lookup_in_dir(&self, dir_inode: &Inode, name: &str) -> Result<Option<InodeNumber>> {
        let num_entries = dir_inode.size / DIR_ENTRY_SIZE;
        for i in 0..num_entries {
            let offset = i * DIR_ENTRY_SIZE;
            let block_idx = BlockIndex::from_byte_offset(offset);
            let block_offset = (offset % BLOCK_SIZE as u64) as usize;

            let block_addr = self.get_block_addr(dir_inode, block_idx)?;
            if !block_addr.is_valid() {
                continue;
            }

            let disk_offset = block_addr.byte_offset() + block_offset;
            if let Some(entry) = DirEntry::from_bytes(&self.data[disk_offset..])
                && entry.ino.is_valid()
                && entry.name() == name
            {
                return Ok(Some(entry.ino));
            }
        }
        Ok(None)
    }

    fn create_file(&mut self, name: &str) -> Result<InodeNumber> {
        let ino = self.superblock.next_inode;
        self.superblock.next_inode = self.superblock.next_inode.next();

        let file_inode = Inode::new(ino);
        self.write_inode(&file_inode)?;

        self.add_dir_entry(InodeNumber::ROOT, ino, name)?;

        self.write_superblock()?;
        self.persist_inode_map()?;

        Ok(ino)
    }

    fn add_dir_entry(
        &mut self,
        dir_ino: InodeNumber,
        file_ino: InodeNumber,
        name: &str,
    ) -> Result<()> {
        let entry = DirEntry::new(file_ino, name).ok_or(Error::FilenameTooLong)?;
        let entry_bytes = entry.to_bytes();

        let mut dir_inode = self.read_inode(dir_ino)?;
        let offset = dir_inode.size;
        let block_idx = BlockIndex::from_byte_offset(offset);
        let block_offset = (offset % BLOCK_SIZE as u64) as usize;

        let mut block_data = [0u8; BLOCK_SIZE];
        let old_block_addr = self.get_block_addr(&dir_inode, block_idx)?;

        if old_block_addr.is_valid() {
            let disk_offset = old_block_addr.byte_offset();
            block_data.copy_from_slice(&self.data[disk_offset..disk_offset + BLOCK_SIZE]);
        }

        let entry_end = block_offset + DIR_ENTRY_SIZE as usize;
        if entry_end > BLOCK_SIZE {
            return Err(Error::NoSpace);
        }

        block_data[block_offset..entry_end].copy_from_slice(&entry_bytes);

        let new_block_addr = self.allocate_block()?;
        self.write_block(new_block_addr, &block_data)?;

        if old_block_addr.is_valid() {
            self.segment_summary.remove(&old_block_addr);
        }

        self.segment_summary.insert(
            new_block_addr,
            SegmentSummaryEntry {
                ino: dir_ino,
                block_index: block_idx,
                entry_type: SegmentEntryType::Data,
            },
        );

        self.set_block_addr(&mut dir_inode, block_idx, new_block_addr)?;
        dir_inode.size += DIR_ENTRY_SIZE;
        self.write_inode(&dir_inode)?;

        Ok(())
    }

    fn persist_inode_map(&mut self) -> Result<()> {
        let entries: Vec<(InodeNumber, BlockAddress)> =
            self.inode_map.iter().map(|(&k, &v)| (k, v)).collect();
        let entries_per_block = BLOCK_SIZE / 16;
        let num_blocks = entries.len().div_ceil(entries_per_block);

        if num_blocks == 0 {
            return Ok(());
        }

        let mut prev_block = BlockAddress::INVALID;

        for chunk_idx in (0..num_blocks).rev() {
            let mut block_data = [0u8; BLOCK_SIZE];
            block_data[0..8].copy_from_slice(&prev_block.as_u64().to_le_bytes());

            let start = chunk_idx * entries_per_block;
            let end = (start + entries_per_block).min(entries.len());

            for (i, &(ino, addr)) in entries[start..end].iter().enumerate() {
                let offset = 8 + i * 16;
                block_data[offset..offset + 8].copy_from_slice(&ino.as_u64().to_le_bytes());
                block_data[offset + 8..offset + 16].copy_from_slice(&addr.as_u64().to_le_bytes());
            }

            let block_addr = self.allocate_block()?;
            self.write_block(block_addr, &block_data)?;

            if let Some(&old_map_block) = self.inode_map.get(&InodeNumber::INVALID)
                && old_map_block.is_valid()
            {
                self.segment_summary.remove(&old_map_block);
            }

            self.segment_summary.insert(
                block_addr,
                SegmentSummaryEntry {
                    ino: InodeNumber::INVALID,
                    block_index: BlockIndex::new(chunk_idx as u64),
                    entry_type: SegmentEntryType::InodeMap,
                },
            );

            prev_block = block_addr;
        }

        self.superblock.inode_map_block = prev_block;
        self.write_superblock()?;

        Ok(())
    }

    fn load_inode_map(&mut self) -> Result<()> {
        let mut block_addr = self.superblock.inode_map_block;
        if !block_addr.is_valid() {
            return Ok(());
        }

        let entries_per_block = BLOCK_SIZE / 16;

        while block_addr.is_valid() {
            let disk_offset = block_addr.byte_offset();
            if disk_offset + BLOCK_SIZE > self.data.len() {
                return Err(Error::CorruptFilesystem);
            }

            let next_block = BlockAddress::new(u64::from_le_bytes(
                self.data[disk_offset..disk_offset + 8].try_into().unwrap(),
            ));

            for i in 0..entries_per_block - 1 {
                let offset = disk_offset + 8 + i * 16;
                let ino = InodeNumber::new(u64::from_le_bytes(
                    self.data[offset..offset + 8].try_into().unwrap(),
                ));
                let addr = BlockAddress::new(u64::from_le_bytes(
                    self.data[offset + 8..offset + 16].try_into().unwrap(),
                ));

                if ino.is_valid() && addr.is_valid() {
                    self.inode_map.insert(ino, addr);
                }
            }

            block_addr = next_block;
        }

        Ok(())
    }

    fn rebuild_segment_summary(&mut self) -> Result<()> {
        for (&ino, &block_addr) in &self.inode_map {
            self.segment_summary.insert(
                block_addr,
                SegmentSummaryEntry {
                    ino,
                    block_index: BlockIndex::new(0),
                    entry_type: SegmentEntryType::Inode,
                },
            );

            if let Ok(inode) = self.read_inode_from_map(ino) {
                let num_blocks = BlockIndex::blocks_for_size(inode.size);

                for block_num in 0..num_blocks {
                    let block_idx = BlockIndex::new(block_num);
                    if let Ok(data_addr) = self.get_block_addr(&inode, block_idx)
                        && data_addr.is_valid()
                    {
                        self.segment_summary.insert(
                            data_addr,
                            SegmentSummaryEntry {
                                ino,
                                block_index: block_idx,
                                entry_type: SegmentEntryType::Data,
                            },
                        );
                    }
                }

                if inode.indirect.is_valid() {
                    self.segment_summary.insert(
                        inode.indirect,
                        SegmentSummaryEntry {
                            ino,
                            block_index: BlockIndex::new(0),
                            entry_type: SegmentEntryType::Indirect,
                        },
                    );
                }

                if inode.double_indirect.is_valid() {
                    self.segment_summary.insert(
                        inode.double_indirect,
                        SegmentSummaryEntry {
                            ino,
                            block_index: BlockIndex::new(0),
                            entry_type: SegmentEntryType::Indirect,
                        },
                    );
                }
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_fs(blocks: usize) -> Lfs {
        let data = vec![0u8; blocks * BLOCK_SIZE];
        Lfs::new(data).expect("Failed to create filesystem")
    }

    #[test]
    fn create_filesystem() {
        let lfs = create_test_fs(64);
        assert!(lfs.tail().as_u64() >= 1);
        println!(
            "Filesystem created with tail at block {}",
            lfs.tail().as_u64()
        );
    }

    #[test]
    fn open_and_close_file() {
        let mut lfs = create_test_fs(64);

        let fd = lfs.open_file("test.txt").expect("Failed to open file");
        println!("Opened file with {}", fd);

        lfs.close(fd).expect("Failed to close file");
        println!("Closed file successfully");
    }

    #[test]
    fn write_and_read_small() {
        let mut lfs = create_test_fs(64);

        let fd = lfs.open_file("test.txt").expect("Failed to open file");
        let data = b"Hello, LFS!";

        let written = lfs.write(fd, data).expect("Failed to write");
        assert_eq!(written, data.len());
        println!("Wrote {} bytes", written);

        lfs.seek(fd, 0).expect("Failed to seek");

        let mut buf = vec![0u8; data.len()];
        let read = lfs.read(fd, &mut buf).expect("Failed to read");
        assert_eq!(read, data.len());
        assert_eq!(&buf, data);
        println!("Read {} bytes: {:?}", read, String::from_utf8_lossy(&buf));

        lfs.close(fd).expect("Failed to close");
    }

    #[test]
    fn write_and_read_large() {
        let mut lfs = create_test_fs(256);

        let fd = lfs.open_file("large.bin").expect("Failed to open file");
        let data: Vec<u8> = (0..20000).map(|i| (i % 256) as u8).collect();

        let written = lfs.write(fd, &data).expect("Failed to write");
        assert_eq!(written, data.len());
        println!("Wrote {} bytes across multiple blocks", written);

        lfs.seek(fd, 0).expect("Failed to seek");

        let mut buf = vec![0u8; data.len()];
        let read = lfs.read(fd, &mut buf).expect("Failed to read");
        assert_eq!(read, data.len());
        assert_eq!(buf, data);
        println!("Read {} bytes successfully", read);

        lfs.close(fd).expect("Failed to close");
    }

    #[test]
    fn multiple_files() {
        let mut lfs = create_test_fs(128);

        let fd1 = lfs.open_file("file1.txt").expect("Failed to open file1");
        let fd2 = lfs.open_file("file2.txt").expect("Failed to open file2");

        lfs.write(fd1, b"File 1 content")
            .expect("Failed to write file1");
        lfs.write(fd2, b"File 2 content")
            .expect("Failed to write file2");

        lfs.seek(fd1, 0).expect("Failed to seek file1");
        lfs.seek(fd2, 0).expect("Failed to seek file2");

        let mut buf1 = vec![0u8; 14];
        let mut buf2 = vec![0u8; 14];

        lfs.read(fd1, &mut buf1).expect("Failed to read file1");
        lfs.read(fd2, &mut buf2).expect("Failed to read file2");

        assert_eq!(&buf1, b"File 1 content");
        assert_eq!(&buf2, b"File 2 content");
        println!("Multiple files work correctly");

        lfs.close(fd1).expect("Failed to close file1");
        lfs.close(fd2).expect("Failed to close file2");
    }

    #[test]
    fn truncate_file() {
        let mut lfs = create_test_fs(64);

        let fd = lfs.open_file("truncate.txt").expect("Failed to open file");
        lfs.write(fd, b"Hello, World!").expect("Failed to write");

        let size_before = lfs.file_size(fd).expect("Failed to get size");
        assert_eq!(size_before, 13);
        println!("Size before truncate: {}", size_before);

        lfs.truncate(fd, 5).expect("Failed to truncate");

        let size_after = lfs.file_size(fd).expect("Failed to get size");
        assert_eq!(size_after, 5);
        println!("Size after truncate: {}", size_after);

        lfs.seek(fd, 0).expect("Failed to seek");
        let mut buf = vec![0u8; 5];
        lfs.read(fd, &mut buf).expect("Failed to read");
        assert_eq!(&buf, b"Hello");
        println!(
            "Content after truncate: {:?}",
            String::from_utf8_lossy(&buf)
        );

        lfs.close(fd).expect("Failed to close");
    }

    #[test]
    fn truncate_extend() {
        let mut lfs = create_test_fs(64);

        let fd = lfs.open_file("extend.txt").expect("Failed to open file");
        lfs.write(fd, b"Hi").expect("Failed to write");

        lfs.truncate(fd, 10).expect("Failed to truncate");

        let size = lfs.file_size(fd).expect("Failed to get size");
        assert_eq!(size, 10);
        println!("Extended file to size: {}", size);

        lfs.close(fd).expect("Failed to close");
    }

    #[test]
    fn reopen_file() {
        let mut lfs = create_test_fs(64);

        let fd1 = lfs.open_file("reopen.txt").expect("Failed to open file");
        lfs.write(fd1, b"Persistent data").expect("Failed to write");
        lfs.close(fd1).expect("Failed to close");

        let fd2 = lfs.open_file("reopen.txt").expect("Failed to reopen file");
        let mut buf = vec![0u8; 15];
        lfs.read(fd2, &mut buf).expect("Failed to read");
        assert_eq!(&buf, b"Persistent data");
        println!(
            "Reopened file contains: {:?}",
            String::from_utf8_lossy(&buf)
        );

        lfs.close(fd2).expect("Failed to close");
    }

    #[test]
    fn log_cleaning() {
        let mut lfs = create_test_fs(64);

        let fd = lfs.open_file("clean.txt").expect("Failed to open file");

        for i in 0..10 {
            lfs.seek(fd, 0).expect("Failed to seek");
            let data = format!("Version {}", i);
            lfs.write(fd, data.as_bytes()).expect("Failed to write");
        }

        let free_before = lfs.free_blocks();
        println!("Free blocks before cleaning: {}", free_before);

        let reclaimed = lfs.clean().expect("Failed to clean");
        println!("Blocks reclaimed: {}", reclaimed);

        lfs.seek(fd, 0).expect("Failed to seek");
        let mut buf = vec![0u8; 9];
        lfs.read(fd, &mut buf).expect("Failed to read");
        assert_eq!(&buf, b"Version 9");
        println!(
            "Data preserved after cleaning: {:?}",
            String::from_utf8_lossy(&buf)
        );

        lfs.close(fd).expect("Failed to close");
    }

    #[test]
    fn persist_and_restore() {
        let data = vec![0u8; 64 * BLOCK_SIZE];
        let mut lfs = Lfs::new(data).expect("Failed to create filesystem");

        let fd = lfs.open_file("persist.txt").expect("Failed to open file");
        lfs.write(fd, b"Saved data").expect("Failed to write");
        lfs.close(fd).expect("Failed to close");

        let tail = lfs.tail();
        println!("Tail position: {}", tail.as_u64());

        let data = lfs.into_inner();

        let mut lfs2 = Lfs::open(data, tail).expect("Failed to reopen filesystem");
        let fd = lfs2.open_file("persist.txt").expect("Failed to open file");
        let mut buf = vec![0u8; 10];
        lfs2.read(fd, &mut buf).expect("Failed to read");
        assert_eq!(&buf, b"Saved data");
        println!("Restored data: {:?}", String::from_utf8_lossy(&buf));

        lfs2.close(fd).expect("Failed to close");
    }

    #[test]
    fn file_too_large() {
        let mut lfs = create_test_fs(64);
        let max_size = lfs.max_file_size;

        let fd = lfs.open_file("huge.bin").expect("Failed to open file");

        lfs.seek(fd, max_size).expect("Failed to seek");
        let result = lfs.write(fd, b"X");
        assert_eq!(result, Err(Error::FileTooLarge));
        println!(
            "Correctly rejected write beyond max file size ({})",
            max_size
        );

        lfs.close(fd).expect("Failed to close");
    }

    #[test]
    fn invalid_fd() {
        let mut lfs = create_test_fs(64);
        let bad_fd = FileDescriptor::new(999);

        let result = lfs.read(bad_fd, &mut [0u8; 10]);
        assert_eq!(result, Err(Error::InvalidFd));

        let result = lfs.write(bad_fd, b"test");
        assert_eq!(result, Err(Error::InvalidFd));

        let result = lfs.close(bad_fd);
        assert_eq!(result, Err(Error::InvalidFd));

        println!("Invalid fd operations correctly rejected");
    }

    #[test]
    fn buffer_too_small() {
        let data = vec![0u8; 8 * BLOCK_SIZE];
        let result = Lfs::new(data);
        assert!(result.is_err(), "Expected BufferTooSmall error");
        match result {
            Err(Error::BufferTooSmall) => println!("Buffer too small correctly rejected"),
            Err(e) => panic!("Expected BufferTooSmall, got {:?}", e),
            Ok(_) => panic!("Expected error, got Ok"),
        }
    }

    #[test]
    fn filename_too_long() {
        let mut lfs = create_test_fs(64);

        let long_name = "x".repeat(MAX_FILENAME_LEN + 1);
        let result = lfs.open_file(&long_name);
        assert_eq!(result, Err(Error::FilenameTooLong));
        println!("Long filename correctly rejected");
    }

    #[test]
    fn read_at_eof() {
        let mut lfs = create_test_fs(64);

        let fd = lfs.open_file("eof.txt").expect("Failed to open file");
        lfs.write(fd, b"Short").expect("Failed to write");

        lfs.seek(fd, 100).expect("Failed to seek");
        let mut buf = vec![0u8; 10];
        let read = lfs.read(fd, &mut buf).expect("Failed to read");
        assert_eq!(read, 0);
        println!("Read at EOF returns 0 bytes");

        lfs.close(fd).expect("Failed to close");
    }

    #[test]
    fn partial_block_write() {
        let mut lfs = create_test_fs(64);

        let fd = lfs.open_file("partial.txt").expect("Failed to open file");

        lfs.write(fd, b"First").expect("Failed to write");
        lfs.seek(fd, 10).expect("Failed to seek");
        lfs.write(fd, b"Second").expect("Failed to write");

        lfs.seek(fd, 0).expect("Failed to seek");
        let mut buf = vec![0u8; 16];
        lfs.read(fd, &mut buf).expect("Failed to read");

        assert_eq!(&buf[0..5], b"First");
        assert_eq!(&buf[5..10], &[0, 0, 0, 0, 0]);
        assert_eq!(&buf[10..16], b"Second");
        println!("Partial block writes work correctly");

        lfs.close(fd).expect("Failed to close");
    }

    #[test]
    fn indirect_blocks() {
        let mut lfs = create_test_fs(256);

        let fd = lfs.open_file("indirect.bin").expect("Failed to open file");

        let block_data: Vec<u8> = (0..BLOCK_SIZE).map(|i| (i % 256) as u8).collect();

        for i in 0..(DIRECT_BLOCKS + 5) {
            lfs.seek(fd, (i * BLOCK_SIZE) as u64)
                .expect("Failed to seek");
            lfs.write(fd, &block_data).expect("Failed to write block");
            println!("Wrote block {}", i);
        }

        for i in 0..(DIRECT_BLOCKS + 5) {
            lfs.seek(fd, (i * BLOCK_SIZE) as u64)
                .expect("Failed to seek");
            let mut buf = vec![0u8; BLOCK_SIZE];
            lfs.read(fd, &mut buf).expect("Failed to read block");
            assert_eq!(buf, block_data, "Block {} mismatch", i);
        }
        println!("Indirect blocks work correctly");

        lfs.close(fd).expect("Failed to close");
    }

    #[test]
    fn free_space_tracking() {
        let mut lfs = create_test_fs(64);

        let initial_free = lfs.free_blocks();
        println!("Initial free blocks: {}", initial_free);

        let fd = lfs.open_file("space.txt").expect("Failed to open file");
        lfs.write(fd, &vec![0u8; BLOCK_SIZE * 5])
            .expect("Failed to write");

        let after_write = lfs.free_blocks();
        println!("Free blocks after write: {}", after_write);
        assert!(after_write < initial_free);

        lfs.close(fd).expect("Failed to close");
    }

    #[test]
    fn strong_types_prevent_mixing() {
        let ino = InodeNumber::new(42);
        let block_addr = BlockAddress::new(100);
        let block_idx = BlockIndex::new(5);
        let fd = FileDescriptor::new(3);
        let fd_next = fd.next();

        assert_eq!(ino.as_u64(), 42);
        assert_eq!(block_addr.as_u64(), 100);
        assert_eq!(block_idx.as_u64(), 5);
        assert_ne!(fd, fd_next);

        assert!(InodeNumber::ROOT.is_valid());
        assert!(!InodeNumber::INVALID.is_valid());
        assert!(block_addr.is_valid());
        assert!(!BlockAddress::INVALID.is_valid());

        println!("Strong types work correctly");
    }

    #[test]
    fn write_empty_data() {
        let mut lfs = create_test_fs(64);

        let fd = lfs
            .open_file("empty_write.txt")
            .expect("Failed to open file");
        let written = lfs.write(fd, b"").expect("Failed to write empty");
        assert_eq!(written, 0);
        println!("Empty write returned {} bytes", written);

        let size = lfs.file_size(fd).expect("Failed to get size");
        assert_eq!(size, 0);
        println!("File size after empty write: {}", size);

        lfs.close(fd).expect("Failed to close");
    }

    #[test]
    fn read_empty_file() {
        let mut lfs = create_test_fs(64);

        let fd = lfs
            .open_file("empty_read.txt")
            .expect("Failed to open file");
        let mut buf = vec![0u8; 100];
        let read = lfs.read(fd, &mut buf).expect("Failed to read");
        assert_eq!(read, 0);
        println!("Read from empty file returned {} bytes", read);

        lfs.close(fd).expect("Failed to close");
    }

    #[test]
    fn read_with_empty_buffer() {
        let mut lfs = create_test_fs(64);

        let fd = lfs.open_file("test.txt").expect("Failed to open file");
        lfs.write(fd, b"Some data").expect("Failed to write");
        lfs.seek(fd, 0).expect("Failed to seek");

        let mut buf: [u8; 0] = [];
        let read = lfs.read(fd, &mut buf).expect("Failed to read");
        assert_eq!(read, 0);
        println!("Read with empty buffer returned {} bytes", read);

        lfs.close(fd).expect("Failed to close");
    }

    #[test]
    fn seek_beyond_file_size() {
        let mut lfs = create_test_fs(64);

        let fd = lfs.open_file("seek_test.txt").expect("Failed to open file");
        lfs.write(fd, b"Hello").expect("Failed to write");

        lfs.seek(fd, 1000).expect("Failed to seek beyond end");
        lfs.write(fd, b"World")
            .expect("Failed to write at offset 1000");

        let size = lfs.file_size(fd).expect("Failed to get size");
        assert_eq!(size, 1005);
        println!("File size after sparse write: {}", size);

        lfs.seek(fd, 0).expect("Failed to seek");
        let mut buf = vec![0u8; 5];
        lfs.read(fd, &mut buf).expect("Failed to read");
        assert_eq!(&buf, b"Hello");

        lfs.seek(fd, 1000).expect("Failed to seek");
        let mut buf = vec![0u8; 5];
        lfs.read(fd, &mut buf).expect("Failed to read");
        assert_eq!(&buf, b"World");

        lfs.seek(fd, 500).expect("Failed to seek to hole");
        let mut buf = vec![0u8; 10];
        let read = lfs.read(fd, &mut buf).expect("Failed to read hole");
        assert_eq!(read, 10);
        assert_eq!(&buf, &[0u8; 10]);
        println!("Sparse file hole contains zeros");

        lfs.close(fd).expect("Failed to close");
    }

    #[test]
    fn overwrite_existing_data() {
        let mut lfs = create_test_fs(64);

        let fd = lfs.open_file("overwrite.txt").expect("Failed to open file");
        lfs.write(fd, b"AAAAAAAAAA").expect("Failed to write As");

        lfs.seek(fd, 3).expect("Failed to seek");
        lfs.write(fd, b"BBB").expect("Failed to write Bs");

        lfs.seek(fd, 0).expect("Failed to seek");
        let mut buf = vec![0u8; 10];
        lfs.read(fd, &mut buf).expect("Failed to read");
        assert_eq!(&buf, b"AAABBBAAAA");
        println!("Overwrite result: {:?}", String::from_utf8_lossy(&buf));

        lfs.close(fd).expect("Failed to close");
    }

    #[test]
    fn truncate_to_zero() {
        let mut lfs = create_test_fs(64);

        let fd = lfs
            .open_file("truncate_zero.txt")
            .expect("Failed to open file");
        lfs.write(fd, b"Some content here")
            .expect("Failed to write");

        let size_before = lfs.file_size(fd).expect("Failed to get size");
        assert_eq!(size_before, 17);

        lfs.truncate(fd, 0).expect("Failed to truncate to zero");

        let size_after = lfs.file_size(fd).expect("Failed to get size");
        assert_eq!(size_after, 0);
        println!("Truncated file to zero bytes");

        lfs.seek(fd, 0).expect("Failed to seek");
        let mut buf = vec![0u8; 10];
        let read = lfs.read(fd, &mut buf).expect("Failed to read");
        assert_eq!(read, 0);
        println!("Read from truncated file returns 0 bytes");

        lfs.close(fd).expect("Failed to close");
    }

    #[test]
    fn truncate_invalid_fd() {
        let mut lfs = create_test_fs(64);
        let bad_fd = FileDescriptor::new(999);

        let result = lfs.truncate(bad_fd, 100);
        assert_eq!(result, Err(Error::InvalidFd));
        println!("Truncate with invalid fd correctly rejected");
    }

    #[test]
    fn seek_invalid_fd() {
        let mut lfs = create_test_fs(64);
        let bad_fd = FileDescriptor::new(999);

        let result = lfs.seek(bad_fd, 100);
        assert_eq!(result, Err(Error::InvalidFd));
        println!("Seek with invalid fd correctly rejected");
    }

    #[test]
    fn file_size_invalid_fd() {
        let lfs = create_test_fs(64);
        let bad_fd = FileDescriptor::new(999);

        let result = lfs.file_size(bad_fd);
        assert_eq!(result, Err(Error::InvalidFd));
        println!("File size with invalid fd correctly rejected");
    }

    #[test]
    fn truncate_too_large() {
        let mut lfs = create_test_fs(64);
        let max_size = lfs.max_file_size;

        let fd = lfs
            .open_file("truncate_large.txt")
            .expect("Failed to open file");

        let result = lfs.truncate(fd, max_size + 1);
        assert_eq!(result, Err(Error::FileTooLarge));
        println!("Truncate beyond max size correctly rejected");

        lfs.close(fd).expect("Failed to close");
    }

    #[test]
    fn multiple_opens_same_file() {
        let mut lfs = create_test_fs(64);

        let fd1 = lfs.open_file("shared.txt").expect("Failed to open file");
        lfs.write(fd1, b"Written by fd1").expect("Failed to write");

        let fd2 = lfs
            .open_file("shared.txt")
            .expect("Failed to open same file");

        let mut buf = vec![0u8; 14];
        lfs.read(fd2, &mut buf).expect("Failed to read from fd2");
        assert_eq!(&buf, b"Written by fd1");
        println!("Second fd can read data written by first fd");

        lfs.write(fd2, b" and fd2")
            .expect("Failed to write from fd2");

        lfs.seek(fd1, 0).expect("Failed to seek fd1");
        let mut buf = vec![0u8; 22];
        lfs.read(fd1, &mut buf).expect("Failed to read from fd1");
        assert_eq!(&buf, b"Written by fd1 and fd2");
        println!("First fd sees writes from second fd");

        lfs.close(fd1).expect("Failed to close fd1");
        lfs.close(fd2).expect("Failed to close fd2");
    }

    #[test]
    fn write_at_block_boundary() {
        let mut lfs = create_test_fs(64);

        let fd = lfs.open_file("boundary.txt").expect("Failed to open file");

        let first_block = vec![b'A'; BLOCK_SIZE];
        lfs.write(fd, &first_block)
            .expect("Failed to write first block");

        let second_block = vec![b'B'; BLOCK_SIZE];
        lfs.write(fd, &second_block)
            .expect("Failed to write second block");

        let size = lfs.file_size(fd).expect("Failed to get size");
        assert_eq!(size, 2 * BLOCK_SIZE as u64);
        println!("File spans exactly 2 blocks: {} bytes", size);

        lfs.seek(fd, BLOCK_SIZE as u64 - 1).expect("Failed to seek");
        let mut buf = vec![0u8; 2];
        lfs.read(fd, &mut buf)
            .expect("Failed to read across boundary");
        assert_eq!(&buf, b"AB");
        println!("Read across block boundary: {:?}", buf);

        lfs.close(fd).expect("Failed to close");
    }

    #[test]
    fn write_spanning_block_boundary() {
        let mut lfs = create_test_fs(64);

        let fd = lfs.open_file("span.txt").expect("Failed to open file");

        lfs.seek(fd, BLOCK_SIZE as u64 - 5).expect("Failed to seek");
        lfs.write(fd, b"0123456789")
            .expect("Failed to write spanning boundary");

        lfs.seek(fd, BLOCK_SIZE as u64 - 5).expect("Failed to seek");
        let mut buf = vec![0u8; 10];
        lfs.read(fd, &mut buf).expect("Failed to read");
        assert_eq!(&buf, b"0123456789");
        println!("Write spanning block boundary preserved correctly");

        lfs.close(fd).expect("Failed to close");
    }

    #[test]
    fn partial_read_at_eof() {
        let mut lfs = create_test_fs(64);

        let fd = lfs
            .open_file("partial_eof.txt")
            .expect("Failed to open file");
        lfs.write(fd, b"Short").expect("Failed to write");

        lfs.seek(fd, 2).expect("Failed to seek");
        let mut buf = vec![0u8; 100];
        let read = lfs.read(fd, &mut buf).expect("Failed to read");
        assert_eq!(read, 3);
        assert_eq!(&buf[..3], b"ort");
        println!("Partial read at EOF returned {} bytes", read);

        lfs.close(fd).expect("Failed to close");
    }

    #[test]
    fn sequential_file_creation() {
        let mut lfs = create_test_fs(128);

        for i in 0..10 {
            let name = format!("file_{}.txt", i);
            let fd = lfs.open_file(&name).expect("Failed to open file");
            let content = format!("Content of file {}", i);
            lfs.write(fd, content.as_bytes()).expect("Failed to write");
            lfs.close(fd).expect("Failed to close");
        }
        println!("Created 10 files");

        for i in 0..10 {
            let name = format!("file_{}.txt", i);
            let fd = lfs.open_file(&name).expect("Failed to reopen file");
            let expected = format!("Content of file {}", i);
            let mut buf = vec![0u8; expected.len()];
            lfs.read(fd, &mut buf).expect("Failed to read");
            assert_eq!(buf, expected.as_bytes());
            lfs.close(fd).expect("Failed to close");
        }
        println!("Verified all 10 files");
    }

    #[test]
    fn clean_with_no_garbage() {
        let mut lfs = create_test_fs(64);

        let fd = lfs
            .open_file("clean_test.txt")
            .expect("Failed to open file");
        lfs.write(fd, b"Original data").expect("Failed to write");

        let reclaimed = lfs.clean().expect("Failed to clean");
        println!("Reclaimed with no garbage: {}", reclaimed);

        lfs.seek(fd, 0).expect("Failed to seek");
        let mut buf = vec![0u8; 13];
        lfs.read(fd, &mut buf).expect("Failed to read");
        assert_eq!(&buf, b"Original data");
        println!("Data preserved after clean with no garbage");

        lfs.close(fd).expect("Failed to close");
    }

    #[test]
    fn clean_after_truncate() {
        let mut lfs = create_test_fs(64);

        let fd = lfs
            .open_file("truncate_clean.txt")
            .expect("Failed to open file");
        let large_data = vec![b'X'; BLOCK_SIZE * 3];
        lfs.write(fd, &large_data).expect("Failed to write");

        lfs.truncate(fd, BLOCK_SIZE as u64)
            .expect("Failed to truncate");

        let reclaimed = lfs.clean().expect("Failed to clean");
        println!("Reclaimed after truncate: {}", reclaimed);

        lfs.seek(fd, 0).expect("Failed to seek");
        let mut buf = vec![0u8; BLOCK_SIZE];
        lfs.read(fd, &mut buf).expect("Failed to read");
        assert!(buf.iter().all(|&b| b == b'X'));
        println!("Remaining data preserved after clean");

        lfs.close(fd).expect("Failed to close");
    }

    #[test]
    fn persist_multiple_files() {
        let data = vec![0u8; 128 * BLOCK_SIZE];
        let mut lfs = Lfs::new(data).expect("Failed to create filesystem");

        let files = [
            ("alpha.txt", "Alpha content"),
            ("beta.txt", "Beta content"),
            ("gamma.txt", "Gamma content"),
        ];

        for (name, content) in &files {
            let fd = lfs.open_file(name).expect("Failed to open file");
            lfs.write(fd, content.as_bytes()).expect("Failed to write");
            lfs.close(fd).expect("Failed to close");
        }

        let tail = lfs.tail();
        let data = lfs.into_inner();

        let mut lfs2 = Lfs::open(data, tail).expect("Failed to reopen filesystem");

        for (name, expected_content) in &files {
            let fd = lfs2.open_file(name).expect("Failed to open file");
            let mut buf = vec![0u8; expected_content.len()];
            lfs2.read(fd, &mut buf).expect("Failed to read");
            assert_eq!(buf, expected_content.as_bytes());
            lfs2.close(fd).expect("Failed to close");
        }
        println!("All files persisted and restored correctly");
    }

    #[test]
    fn max_filename_length() {
        let mut lfs = create_test_fs(64);

        let max_name = "x".repeat(MAX_FILENAME_LEN);
        let fd = lfs
            .open_file(&max_name)
            .expect("Failed to open file with max name length");
        lfs.write(fd, b"test").expect("Failed to write");
        lfs.close(fd).expect("Failed to close");

        let fd = lfs.open_file(&max_name).expect("Failed to reopen file");
        let mut buf = vec![0u8; 4];
        lfs.read(fd, &mut buf).expect("Failed to read");
        assert_eq!(&buf, b"test");
        println!("Max filename length ({}) works", MAX_FILENAME_LEN);

        lfs.close(fd).expect("Failed to close");
    }

    #[test]
    fn close_then_reuse_fd_number() {
        let mut lfs = create_test_fs(64);

        let fd1 = lfs.open_file("first.txt").expect("Failed to open first");
        lfs.write(fd1, b"First file").expect("Failed to write");
        lfs.close(fd1).expect("Failed to close first");

        let result = lfs.read(fd1, &mut [0u8; 10]);
        assert_eq!(result, Err(Error::InvalidFd));
        println!("Closed fd correctly invalid");

        let fd2 = lfs.open_file("second.txt").expect("Failed to open second");
        assert_ne!(fd1, fd2);
        println!("New fd is different from closed fd");

        lfs.close(fd2).expect("Failed to close second");
    }

    #[test]
    fn write_then_read_without_seek() {
        let mut lfs = create_test_fs(64);

        let fd = lfs.open_file("position.txt").expect("Failed to open file");
        lfs.write(fd, b"Hello").expect("Failed to write");

        let mut buf = vec![0u8; 10];
        let read = lfs.read(fd, &mut buf).expect("Failed to read");
        assert_eq!(read, 0);
        println!("Read after write (no seek) returns 0 - position is at EOF");

        lfs.close(fd).expect("Failed to close");
    }

    #[test]
    fn block_index_calculations() {
        assert_eq!(BlockIndex::from_byte_offset(0).as_u64(), 0);
        assert_eq!(
            BlockIndex::from_byte_offset(BLOCK_SIZE as u64 - 1).as_u64(),
            0
        );
        assert_eq!(BlockIndex::from_byte_offset(BLOCK_SIZE as u64).as_u64(), 1);
        assert_eq!(
            BlockIndex::from_byte_offset(BLOCK_SIZE as u64 * 2).as_u64(),
            2
        );
        println!("BlockIndex::from_byte_offset calculations correct");

        assert_eq!(BlockIndex::blocks_for_size(0), 0);
        assert_eq!(BlockIndex::blocks_for_size(1), 1);
        assert_eq!(BlockIndex::blocks_for_size(BLOCK_SIZE as u64), 1);
        assert_eq!(BlockIndex::blocks_for_size(BLOCK_SIZE as u64 + 1), 2);
        println!("BlockIndex::blocks_for_size calculations correct");
    }

    #[test]
    fn block_address_byte_offset() {
        assert_eq!(BlockAddress::new(0).byte_offset(), 0);
        assert_eq!(BlockAddress::new(1).byte_offset(), BLOCK_SIZE);
        assert_eq!(BlockAddress::new(10).byte_offset(), BLOCK_SIZE * 10);
        println!("BlockAddress::byte_offset calculations correct");
    }

    #[test]
    fn inode_number_chaining() {
        let ino = InodeNumber::ROOT;
        let next = ino.next();
        let next_next = next.next();

        assert_eq!(ino.as_u64(), 1);
        assert_eq!(next.as_u64(), 2);
        assert_eq!(next_next.as_u64(), 3);
        println!("InodeNumber::next chaining works correctly");
    }

    #[test]
    fn minimum_filesystem_size() {
        let data = vec![0u8; 16 * BLOCK_SIZE];
        let lfs = Lfs::new(data).expect("Minimum size filesystem should work");
        assert!(lfs.free_blocks() > 0);
        println!("Minimum filesystem has {} free blocks", lfs.free_blocks());
    }

    #[test]
    fn filesystem_15_blocks_fails() {
        let data = vec![0u8; 15 * BLOCK_SIZE];
        let result = Lfs::new(data);
        match result {
            Err(Error::BufferTooSmall) => println!("15 blocks correctly rejected"),
            Err(e) => panic!("Expected BufferTooSmall, got {:?}", e),
            Ok(_) => panic!("Expected error for 15 blocks"),
        }
    }

    #[test]
    fn corrupt_magic_number() {
        let mut data = vec![0u8; 64 * BLOCK_SIZE];
        data[0..8].copy_from_slice(&0xDEADBEEFu64.to_le_bytes());

        let result = Lfs::open(data, BlockAddress::new(1));
        match result {
            Err(Error::CorruptFilesystem) => println!("Corrupt magic number correctly detected"),
            Err(e) => panic!("Expected CorruptFilesystem, got {:?}", e),
            Ok(_) => panic!("Expected error for corrupt magic"),
        }
    }

    #[test]
    fn truncate_multiple_blocks() {
        let mut lfs = create_test_fs(128);

        let fd = lfs
            .open_file("multi_truncate.txt")
            .expect("Failed to open file");

        let data = vec![b'X'; BLOCK_SIZE * 5];
        lfs.write(fd, &data).expect("Failed to write");

        let size = lfs.file_size(fd).expect("Failed to get size");
        assert_eq!(size, BLOCK_SIZE as u64 * 5);

        lfs.truncate(fd, BLOCK_SIZE as u64 * 2 + 100)
            .expect("Failed to truncate");

        let new_size = lfs.file_size(fd).expect("Failed to get size");
        assert_eq!(new_size, BLOCK_SIZE as u64 * 2 + 100);
        println!("Truncated from 5 blocks to 2.x blocks");

        lfs.seek(fd, 0).expect("Failed to seek");
        let mut buf = vec![0u8; BLOCK_SIZE * 2 + 100];
        let read = lfs.read(fd, &mut buf).expect("Failed to read");
        assert_eq!(read, BLOCK_SIZE * 2 + 100);
        assert!(buf.iter().all(|&b| b == b'X'));
        println!("Truncated content preserved");

        lfs.close(fd).expect("Failed to close");
    }

    #[test]
    fn data_accessor() {
        let mut lfs = create_test_fs(64);
        let initial_len = lfs.data().len();
        assert_eq!(initial_len, 64 * BLOCK_SIZE);
        println!("data() accessor returns correct length: {}", initial_len);

        let fd = lfs.open_file("test.txt").expect("Failed to open");
        lfs.write(fd, b"test").expect("Failed to write");
        lfs.close(fd).expect("Failed to close");

        assert_eq!(lfs.data().len(), initial_len);
        println!("data() length unchanged after writes");
    }

    #[test]
    fn recovery_from_nospace_preserves_data() {
        let mut lfs = create_test_fs(32);

        let fd = lfs.open_file("test.txt").expect("Failed to open file");
        lfs.write(fd, b"Important data").expect("Failed to write");
        lfs.close(fd).expect("Failed to close");
        println!("Wrote initial data");

        let fd = lfs.open_file("fill.txt").expect("Failed to open fill file");
        loop {
            let result = lfs.write(fd, &vec![b'X'; BLOCK_SIZE]);
            match result {
                Ok(_) => continue,
                Err(Error::NoSpace) => {
                    println!("Got NoSpace error as expected");
                    break;
                }
                Err(Error::FileTooLarge) => {
                    println!("Got FileTooLarge, continuing...");
                    break;
                }
                Err(e) => panic!("Unexpected error: {:?}", e),
            }
        }
        lfs.close(fd).expect("Failed to close fill file");

        let fd = lfs
            .open_file("test.txt")
            .expect("Failed to reopen test file");
        let mut buf = vec![0u8; 14];
        lfs.read(fd, &mut buf)
            .expect("Failed to read after NoSpace recovery");
        assert_eq!(&buf, b"Important data");
        println!("Original data preserved after NoSpace recovery");

        lfs.close(fd).expect("Failed to close");
    }

    #[test]
    fn recovery_from_nospace_during_file_creation() {
        let mut lfs = create_test_fs(64);

        let fd = lfs.open_file("original.txt").expect("Failed to open file");
        lfs.write(fd, b"Original content").expect("Failed to write");
        lfs.close(fd).expect("Failed to close");
        println!("Created original file");

        let fd = lfs.open_file("filler.txt").expect("Failed to open filler");
        let mut fill_count = 0;
        loop {
            match lfs.write(fd, &vec![b'X'; BLOCK_SIZE]) {
                Ok(_) => fill_count += 1,
                Err(Error::FileTooLarge) | Err(Error::NoSpace) => break,
                Err(e) => panic!("Unexpected error during fill: {:?}", e),
            }
        }
        lfs.close(fd).expect("Failed to close filler");
        println!("Filled up the filesystem with {} blocks", fill_count);

        let mut created_count = 0;
        for i in 0..100 {
            let name = format!("new_file_{}.txt", i);
            match lfs.open_file(&name) {
                Ok(new_fd) => {
                    created_count += 1;
                    lfs.close(new_fd).expect("Failed to close");
                }
                Err(Error::NoSpace) => {
                    println!("Got NoSpace on file {} creation", i);
                    break;
                }
                Err(e) => panic!("Unexpected error: {:?}", e),
            }
        }
        println!("Created {} files before NoSpace", created_count);

        let fd = lfs
            .open_file("original.txt")
            .expect("Failed to reopen original file");
        let mut buf = vec![0u8; 16];
        lfs.read(fd, &mut buf)
            .expect("Failed to read after recovery");
        assert_eq!(&buf, b"Original content");
        println!("Original file content preserved after file creation NoSpace");

        lfs.close(fd).expect("Failed to close");
    }

    #[test]
    fn multiple_nospace_recoveries() {
        let mut lfs = create_test_fs(64);

        for iteration in 0..3 {
            println!(
                "Iteration {}: committed_tail={}, current_tail={}",
                iteration,
                lfs.committed_tail.as_u64(),
                lfs.superblock.tail.as_u64()
            );
            let fd = lfs
                .open_file("persistent.txt")
                .expect("Failed to open persistent file");
            let content = format!("Iteration {}", iteration);
            lfs.seek(fd, 0).expect("Failed to seek");
            lfs.write(fd, content.as_bytes()).expect("Failed to write");
            lfs.close(fd).expect("Failed to close");
            println!(
                "Iteration {}: wrote content, tail now {}",
                iteration,
                lfs.superblock.tail.as_u64()
            );

            let temp_fd = lfs.open_file("temp.txt").expect("Failed to open temp file");
            let mut write_count = 0;
            loop {
                match lfs.write(temp_fd, &vec![b'Y'; BLOCK_SIZE]) {
                    Ok(_) => write_count += 1,
                    Err(Error::FileTooLarge) | Err(Error::NoSpace) => break,
                    Err(e) => panic!("Unexpected error: {:?}", e),
                }
            }
            lfs.close(temp_fd).expect("Failed to close temp file");
            println!(
                "Iteration {}: wrote {} blocks before NoSpace/FileTooLarge, tail now {}",
                iteration,
                write_count,
                lfs.superblock.tail.as_u64()
            );

            println!(
                "Iteration {}: about to verify, committed_tail={}, current_tail={}",
                iteration,
                lfs.committed_tail.as_u64(),
                lfs.superblock.tail.as_u64()
            );
            let verify_fd = lfs
                .open_file("persistent.txt")
                .expect("Failed to reopen persistent file");
            let mut buf = vec![0u8; content.len()];
            lfs.read(verify_fd, &mut buf).expect("Failed to read");
            assert_eq!(String::from_utf8_lossy(&buf), content);
            println!("Iteration {}: content verified after NoSpace", iteration);
            lfs.close(verify_fd).expect("Failed to close");
        }
    }
}

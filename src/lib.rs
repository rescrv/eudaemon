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
//! - `remove`: Unlink a file (UNIX semantics - open FDs continue to work)
//! - `clean`: Perform log cleaning (garbage collection)

#![deny(missing_docs)]

use std::collections::BTreeMap;
use std::collections::BTreeSet;

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

////////////////////////////////////////////// BlockDevice /////////////////////////////////////////////

/// A minimal block device trait for the LFS implementation.
///
/// Implementations of this trait provide block-level read and write operations.
/// All operations work with fixed-size blocks of `BLOCK_SIZE` bytes.
pub trait BlockDevice {
    /// Reads a block from the device into the provided buffer.
    ///
    /// # Arguments
    /// * `block` - The block address to read from.
    /// * `buf` - A mutable buffer of exactly `BLOCK_SIZE` bytes to read into.
    ///
    /// # Errors
    /// Returns an error if the block address is invalid or the read fails.
    fn read_block(&self, block: BlockAddress, buf: &mut [u8; BLOCK_SIZE]) -> Result<()>;

    /// Writes a block to the device from the provided buffer.
    ///
    /// # Arguments
    /// * `block` - The block address to write to.
    /// * `buf` - A buffer of exactly `BLOCK_SIZE` bytes to write.
    ///
    /// # Errors
    /// Returns an error if the block address is invalid or the write fails.
    fn write_block(&mut self, block: BlockAddress, buf: &[u8; BLOCK_SIZE]) -> Result<()>;

    /// Resets any sequential write tracking maintained by the device.
    ///
    /// This is called before recovery operations that may write to earlier blocks.
    /// The default implementation does nothing.
    fn reset_sequence(&mut self) {}
}

/////////////////////////////////////////// MemoryBlockDevice //////////////////////////////////////////

/// An in-memory block device backed by a `Vec<u8>`.
pub struct MemoryBlockDevice {
    data: Vec<u8>,
}

impl MemoryBlockDevice {
    /// Creates a new memory block device with the given data buffer.
    pub fn new(data: Vec<u8>) -> Self {
        Self { data }
    }

    /// Returns the total number of blocks in the device.
    pub fn total_blocks(&self) -> u64 {
        (self.data.len() / BLOCK_SIZE) as u64
    }

    /// Consumes the device and returns the underlying data buffer.
    pub fn into_inner(self) -> Vec<u8> {
        self.data
    }

    /// Returns a reference to the underlying data.
    pub fn data(&self) -> &[u8] {
        &self.data
    }
}

impl BlockDevice for MemoryBlockDevice {
    fn read_block(&self, block: BlockAddress, buf: &mut [u8; BLOCK_SIZE]) -> Result<()> {
        let offset = block.byte_offset();
        if offset + BLOCK_SIZE > self.data.len() {
            return Err(Error::InvalidOffset);
        }
        buf.copy_from_slice(&self.data[offset..offset + BLOCK_SIZE]);
        Ok(())
    }

    fn write_block(&mut self, block: BlockAddress, buf: &[u8; BLOCK_SIZE]) -> Result<()> {
        let offset = block.byte_offset();
        if offset + BLOCK_SIZE > self.data.len() {
            return Err(Error::InvalidOffset);
        }
        self.data[offset..offset + BLOCK_SIZE].copy_from_slice(buf);
        Ok(())
    }
}

//////////////////////////////////////// SequentialBlockDevice /////////////////////////////////////////

/// A block device wrapper that enforces sequential block writes.
///
/// This is intended for testing to verify that the LFS implementation writes blocks
/// in strictly sequential order, which is a key property of log-structured filesystems.
/// Block 0 (superblock) is exempt from sequential ordering requirements.
///
/// The device is aware of log boundaries and handles wraparound correctly: when the
/// expected next block equals `log_end`, writing to `log_start` is considered sequential.
pub struct SequentialBlockDevice<D: BlockDevice> {
    inner: D,
    next_write_block: Option<BlockAddress>,
    log_start: BlockAddress,
    log_end: BlockAddress,
}

impl<D: BlockDevice> SequentialBlockDevice<D> {
    /// Creates a new sequential block device wrapper with log boundaries.
    ///
    /// The first non-superblock write establishes the starting point for sequential writes.
    /// Log boundaries are used to handle wraparound: when the tail reaches `log_end`,
    /// the next sequential write should be to `log_start`.
    pub fn new(inner: D, log_start: BlockAddress, log_end: BlockAddress) -> Self {
        Self {
            inner,
            next_write_block: None,
            log_start,
            log_end,
        }
    }

    /// Consumes the wrapper and returns the inner device.
    pub fn into_inner(self) -> D {
        self.inner
    }

    /// Returns the next expected block, handling wraparound at log boundaries.
    fn next_block(&self, block: BlockAddress) -> BlockAddress {
        let next = block.next();
        if next.as_u64() >= self.log_end.as_u64() {
            self.log_start
        } else {
            next
        }
    }
}

impl<D: BlockDevice> BlockDevice for SequentialBlockDevice<D> {
    fn read_block(&self, block: BlockAddress, buf: &mut [u8; BLOCK_SIZE]) -> Result<()> {
        self.inner.read_block(block, buf)
    }

    fn write_block(&mut self, block: BlockAddress, buf: &[u8; BLOCK_SIZE]) -> Result<()> {
        // Block 0 (superblock) is exempt from sequential ordering
        if block.as_u64() != 0 {
            match self.next_write_block {
                None => {
                    // First non-superblock write establishes the sequence
                    self.next_write_block = Some(self.next_block(block));
                }
                Some(expected) => {
                    assert_eq!(
                        block,
                        expected,
                        "Non-sequential write detected: expected block {}, got block {}",
                        expected.as_u64(),
                        block.as_u64()
                    );
                    self.next_write_block = Some(self.next_block(block));
                }
            }
        }
        self.inner.write_block(block, buf)
    }

    fn reset_sequence(&mut self) {
        self.next_write_block = None;
        self.inner.reset_sequence();
    }
}

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
    pub fn new(value: u64) -> Self {
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
pub struct Lfs<D: BlockDevice> {
    device: D,
    superblock: Superblock,
    inode_map: BTreeMap<InodeNumber, BlockAddress>,
    segment_summary: BTreeMap<BlockAddress, SegmentSummaryEntry>,
    open_files: BTreeMap<FileDescriptor, OpenFile>,
    next_fd: FileDescriptor,
    max_file_size: u64,
    /// The committed tail position from the last successful operation.
    /// Used to recover from partial writes on NoSpace errors.
    committed_tail: BlockAddress,
    /// Inodes that have been unlinked but still have open file descriptors.
    /// These will be fully removed when the last FD is closed.
    unlinked_inodes: BTreeSet<InodeNumber>,
}

impl<D: BlockDevice> Lfs<D> {
    /// Creates a new LFS on the given block device.
    ///
    /// The device must have at least 16 blocks.
    pub fn new(mut device: D, total_blocks: u64) -> Result<Self> {
        if total_blocks < 16 {
            return Err(Error::BufferTooSmall);
        }

        let max_file_size = (total_blocks as usize * BLOCK_SIZE / 10) as u64;
        let log_start = BlockAddress::new(1);
        let log_end = BlockAddress::new(total_blocks);

        let superblock = Superblock {
            magic: MAGIC,
            block_size: BLOCK_SIZE as u32,
            total_blocks,
            log_start,
            log_end,
            tail: log_start,
            inode_map_block: BlockAddress::INVALID,
            next_inode: InodeNumber::ROOT.next(),
        };

        let committed_tail = log_start;

        // Write the superblock to block 0
        let mut superblock_block = [0u8; BLOCK_SIZE];
        superblock_block[..64].copy_from_slice(&superblock.as_bytes());
        device.write_block(BlockAddress::new(0), &superblock_block)?;

        let mut lfs = Self {
            device,
            superblock,
            inode_map: BTreeMap::new(),
            segment_summary: BTreeMap::new(),
            open_files: BTreeMap::new(),
            next_fd: FileDescriptor::new(0),
            max_file_size,
            committed_tail,
            unlinked_inodes: BTreeSet::new(),
        };

        lfs.create_root_directory()?;
        lfs.committed_tail = lfs.superblock.tail;

        Ok(lfs)
    }

    /// Opens an existing LFS from the given block device.
    ///
    /// The tail position is read from the superblock on disk.
    pub fn open(device: D, total_blocks: u64) -> Result<Self> {
        if total_blocks < 1 {
            return Err(Error::BufferTooSmall);
        }

        // Read superblock from block 0
        let mut superblock_block = [0u8; BLOCK_SIZE];
        device.read_block(BlockAddress::new(0), &mut superblock_block)?;

        let superblock =
            Superblock::from_bytes(&superblock_block).ok_or(Error::CorruptFilesystem)?;

        if superblock.magic != MAGIC {
            return Err(Error::CorruptFilesystem);
        }

        let max_file_size = (total_blocks as usize * BLOCK_SIZE / 10) as u64;
        let tail = superblock.tail;

        let mut lfs = Self {
            device,
            superblock,
            inode_map: BTreeMap::new(),
            segment_summary: BTreeMap::new(),
            open_files: BTreeMap::new(),
            next_fd: FileDescriptor::new(0),
            max_file_size,
            committed_tail: tail,
            unlinked_inodes: BTreeSet::new(),
        };

        lfs.load_inode_map()?;
        lfs.rebuild_segment_summary()?;

        Ok(lfs)
    }

    /// Returns the current tail offset.
    pub fn tail(&self) -> BlockAddress {
        self.superblock.tail
    }

    /// Consumes the filesystem and returns the underlying block device.
    pub fn into_device(self) -> D {
        self.device
    }

    /// Returns a reference to the underlying block device.
    pub fn device(&self) -> &D {
        &self.device
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
    ///
    /// If the file was unlinked while open, and this is the last open FD,
    /// the inode and data blocks are freed.
    pub fn close(&mut self, fd: FileDescriptor) -> Result<()> {
        let open_file = self.open_files.remove(&fd).ok_or(Error::InvalidFd)?;
        let ino = open_file.ino;

        // Check if this inode was unlinked and this was the last open FD
        if self.unlinked_inodes.contains(&ino) {
            let still_open = self.open_files.values().any(|f| f.ino == ino);
            if !still_open {
                self.unlinked_inodes.remove(&ino);
                self.free_inode_blocks(ino)?;
                self.persist_inode_map()?;
                self.commit();
            }
        }

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
                let mut block_data = [0u8; BLOCK_SIZE];
                self.read_block(block_addr, &mut block_data)?;
                buf[bytes_read..bytes_read + bytes_in_block]
                    .copy_from_slice(&block_data[block_offset..block_offset + bytes_in_block]);
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
                self.read_block(old_block_addr, &mut block_data)?;
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

        // If the new size doesn't align to a block boundary, we need to zero
        // out the bytes beyond the new size in the last partial block.
        let block_offset = (size % BLOCK_SIZE as u64) as usize;
        if block_offset > 0 && new_last_block > 0 {
            let last_block_idx = BlockIndex::new(new_last_block - 1);
            let block_addr = self.get_block_addr(&inode, last_block_idx)?;
            if block_addr.is_valid() {
                // Read the existing block
                let mut block_data = [0u8; BLOCK_SIZE];
                self.read_block(block_addr, &mut block_data)?;

                // Zero out bytes beyond the new size
                block_data[block_offset..].fill(0);

                // Write as a new block
                let new_block_addr = self.allocate_block()?;
                self.write_block(new_block_addr, &block_data)?;

                // Update segment summary
                self.segment_summary.remove(&block_addr);
                self.segment_summary.insert(
                    new_block_addr,
                    SegmentSummaryEntry {
                        ino: inode.ino,
                        block_index: last_block_idx,
                        entry_type: SegmentEntryType::Data,
                    },
                );

                // Update inode to point to new block
                self.set_block_addr(&mut inode, last_block_idx, new_block_addr)?;
            }
        }

        // Remove blocks that are entirely beyond the new size
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

    /// Removes a file from the filesystem.
    ///
    /// Uses UNIX semantics: the directory entry is removed immediately, but if
    /// the file has open file descriptors, the inode and data blocks remain
    /// accessible until all FDs are closed. When the last FD is closed, the
    /// space is reclaimed.
    pub fn remove(&mut self, name: &str) -> Result<()> {
        match self.remove_inner(name) {
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

    fn remove_inner(&mut self, name: &str) -> Result<()> {
        // Find the file's inode number
        let ino = self.lookup_file(name)?.ok_or(Error::NotFound)?;

        // Remove the directory entry
        self.remove_dir_entry(InodeNumber::ROOT, name)?;

        // Check if the file is currently open
        let is_open = self.open_files.values().any(|f| f.ino == ino);

        if is_open {
            // Mark as unlinked - will be fully removed when last FD is closed
            self.unlinked_inodes.insert(ino);
        } else {
            // No open FDs, remove immediately
            self.free_inode_blocks(ino)?;
        }

        self.persist_inode_map()?;

        Ok(())
    }

    /// Frees all blocks associated with an inode and removes it from the inode map.
    fn free_inode_blocks(&mut self, ino: InodeNumber) -> Result<()> {
        // Get the inode to find all its blocks
        let inode = self.read_inode(ino)?;

        // Remove data blocks from segment summary
        let num_blocks = BlockIndex::blocks_for_size(inode.size);
        for block_num in 0..num_blocks {
            let block_idx = BlockIndex::new(block_num);
            let block_addr = self.get_block_addr(&inode, block_idx)?;
            if block_addr.is_valid() {
                self.segment_summary.remove(&block_addr);
            }
        }

        // Remove indirect block if present
        if inode.indirect.is_valid() {
            self.segment_summary.remove(&inode.indirect);
        }

        // Remove double indirect blocks if present
        if inode.double_indirect.is_valid() {
            let mut double_block = [0u8; BLOCK_SIZE];
            self.read_block(inode.double_indirect, &mut double_block)?;

            for i in 0..PTRS_PER_BLOCK {
                let offset = i * 8;
                let ptr = u64::from_le_bytes(double_block[offset..offset + 8].try_into().unwrap());
                let first_addr = BlockAddress::new(ptr);
                if first_addr.is_valid() {
                    self.segment_summary.remove(&first_addr);
                }
            }
            self.segment_summary.remove(&inode.double_indirect);
        }

        // Remove inode from inode map and segment summary
        if let Some(&inode_addr) = self.inode_map.get(&ino) {
            self.segment_summary.remove(&inode_addr);
        }
        self.inode_map.remove(&ino);

        Ok(())
    }

    fn remove_dir_entry(&mut self, dir_ino: InodeNumber, name: &str) -> Result<()> {
        let mut dir_inode = self.read_inode(dir_ino)?;
        let num_entries = dir_inode.size / DIR_ENTRY_SIZE;

        for i in 0..num_entries {
            let offset = i * DIR_ENTRY_SIZE;
            let block_idx = BlockIndex::from_byte_offset(offset);
            let block_offset = (offset % BLOCK_SIZE as u64) as usize;

            let block_addr = self.get_block_addr(&dir_inode, block_idx)?;
            if !block_addr.is_valid() {
                continue;
            }

            let mut block_data = [0u8; BLOCK_SIZE];
            self.read_block(block_addr, &mut block_data)?;

            let entry_bytes = &block_data[block_offset..block_offset + DIR_ENTRY_SIZE as usize];
            if let Some(entry) = DirEntry::from_bytes(entry_bytes)
                && entry.ino != InodeNumber::INVALID
                && entry.name() == name
            {
                // Mark entry as deleted by setting ino to INVALID
                let deleted_entry = DirEntry::new(InodeNumber::INVALID, "").unwrap();
                block_data[block_offset..block_offset + DIR_ENTRY_SIZE as usize]
                    .copy_from_slice(&deleted_entry.to_bytes());

                // Write as a new block (copy-on-write)
                let new_block_addr = self.allocate_block()?;
                self.write_block(new_block_addr, &block_data)?;

                self.segment_summary.remove(&block_addr);
                self.segment_summary.insert(
                    new_block_addr,
                    SegmentSummaryEntry {
                        ino: dir_ino,
                        block_index: block_idx,
                        entry_type: SegmentEntryType::Data,
                    },
                );

                self.set_block_addr(&mut dir_inode, block_idx, new_block_addr)?;
                self.write_inode(&dir_inode)?;

                return Ok(());
            }
        }

        Err(Error::NotFound)
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
                self.read_block(old_addr, &mut block_data)?;

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
        // Reset device sequence tracking since we're about to write to earlier blocks
        self.device.reset_sequence();
        let mut block = [0u8; BLOCK_SIZE];
        self.read_block(BlockAddress::new(0), &mut block)?;
        self.superblock = Superblock::from_bytes(&block).ok_or(Error::CorruptFilesystem)?;
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
        let mut block = [0u8; BLOCK_SIZE];
        block[..64].copy_from_slice(&self.superblock.as_bytes());
        self.device.write_block(BlockAddress::new(0), &block)
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
        let mut block = [0u8; BLOCK_SIZE];
        self.read_block(*block_addr, &mut block)?;
        Inode::from_bytes(&block).ok_or(Error::CorruptFilesystem)
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
        self.device.write_block(block, data)
    }

    fn read_block(&self, block: BlockAddress, buf: &mut [u8; BLOCK_SIZE]) -> Result<()> {
        self.device.read_block(block, buf)
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
            let mut block = [0u8; BLOCK_SIZE];
            self.read_block(inode.indirect, &mut block)?;
            let offset = indirect_idx as usize * 8;
            let ptr = u64::from_le_bytes(block[offset..offset + 8].try_into().unwrap());
            return Ok(BlockAddress::new(ptr));
        }

        let double_idx = indirect_idx - PTRS_PER_BLOCK as u64;
        if double_idx < (PTRS_PER_BLOCK * PTRS_PER_BLOCK) as u64 {
            if !inode.double_indirect.is_valid() {
                return Ok(BlockAddress::INVALID);
            }

            let first_level_idx = double_idx / PTRS_PER_BLOCK as u64;
            let second_level_idx = double_idx % PTRS_PER_BLOCK as u64;

            let mut double_block = [0u8; BLOCK_SIZE];
            self.read_block(inode.double_indirect, &mut double_block)?;
            let first_offset = first_level_idx as usize * 8;
            let first_ptr = u64::from_le_bytes(
                double_block[first_offset..first_offset + 8]
                    .try_into()
                    .unwrap(),
            );
            let first_addr = BlockAddress::new(first_ptr);

            if !first_addr.is_valid() {
                return Ok(BlockAddress::INVALID);
            }

            let mut first_block = [0u8; BLOCK_SIZE];
            self.read_block(first_addr, &mut first_block)?;
            let second_offset = second_level_idx as usize * 8;
            let second_ptr = u64::from_le_bytes(
                first_block[second_offset..second_offset + 8]
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
            // Read existing indirect block or create empty one
            let mut block = [0u8; BLOCK_SIZE];
            if inode.indirect.is_valid() {
                self.read_block(inode.indirect, &mut block)?;
                // Remove old indirect block from segment summary
                self.segment_summary.remove(&inode.indirect);
            } else {
                // Initialize with INVALID pointers
                for chunk in block.chunks_exact_mut(8) {
                    chunk.copy_from_slice(&BlockAddress::INVALID.as_u64().to_le_bytes());
                }
            }

            // Update the pointer in the block
            let offset = indirect_idx as usize * 8;
            block[offset..offset + 8].copy_from_slice(&addr.as_u64().to_le_bytes());

            // Allocate new block and write (copy-on-write)
            let new_indirect_block = self.allocate_block()?;
            self.write_block(new_indirect_block, &block)?;
            inode.indirect = new_indirect_block;
            self.segment_summary.insert(
                new_indirect_block,
                SegmentSummaryEntry {
                    ino: inode.ino,
                    block_index: BlockIndex::new(0),
                    entry_type: SegmentEntryType::Indirect,
                },
            );
            return Ok(());
        }

        let double_idx = indirect_idx - PTRS_PER_BLOCK as u64;
        if double_idx < (PTRS_PER_BLOCK * PTRS_PER_BLOCK) as u64 {
            let first_level_idx = double_idx / PTRS_PER_BLOCK as u64;
            let second_level_idx = double_idx % PTRS_PER_BLOCK as u64;

            // Read existing double indirect block or create empty one
            let mut double_block = [0u8; BLOCK_SIZE];
            if inode.double_indirect.is_valid() {
                self.read_block(inode.double_indirect, &mut double_block)?;
            } else {
                // Initialize with INVALID pointers
                for chunk in double_block.chunks_exact_mut(8) {
                    chunk.copy_from_slice(&BlockAddress::INVALID.as_u64().to_le_bytes());
                }
            }

            // Get the first-level indirect block address
            let first_offset = first_level_idx as usize * 8;
            let first_ptr = u64::from_le_bytes(
                double_block[first_offset..first_offset + 8]
                    .try_into()
                    .unwrap(),
            );
            let old_first_addr = BlockAddress::new(first_ptr);

            // Read existing first-level block or create empty one
            let mut first_block = [0u8; BLOCK_SIZE];
            if old_first_addr.is_valid() {
                self.read_block(old_first_addr, &mut first_block)?;
                // Remove old first-level block from segment summary
                self.segment_summary.remove(&old_first_addr);
            } else {
                // Initialize with INVALID pointers
                for chunk in first_block.chunks_exact_mut(8) {
                    chunk.copy_from_slice(&BlockAddress::INVALID.as_u64().to_le_bytes());
                }
            }

            // Update the data pointer in the first-level block
            let second_offset = second_level_idx as usize * 8;
            first_block[second_offset..second_offset + 8]
                .copy_from_slice(&addr.as_u64().to_le_bytes());

            // Allocate new first-level block and write (copy-on-write)
            let new_first_addr = self.allocate_block()?;
            self.write_block(new_first_addr, &first_block)?;
            self.segment_summary.insert(
                new_first_addr,
                SegmentSummaryEntry {
                    ino: inode.ino,
                    block_index: BlockIndex::new(0),
                    entry_type: SegmentEntryType::Indirect,
                },
            );

            // Update the pointer to the first-level block in the double indirect block
            double_block[first_offset..first_offset + 8]
                .copy_from_slice(&new_first_addr.as_u64().to_le_bytes());

            // Remove old double indirect block from segment summary if it existed
            if inode.double_indirect.is_valid() {
                self.segment_summary.remove(&inode.double_indirect);
            }

            // Allocate new double indirect block and write (copy-on-write)
            let new_double_block = self.allocate_block()?;
            self.write_block(new_double_block, &double_block)?;
            inode.double_indirect = new_double_block;
            self.segment_summary.insert(
                new_double_block,
                SegmentSummaryEntry {
                    ino: inode.ino,
                    block_index: BlockIndex::new(0),
                    entry_type: SegmentEntryType::Indirect,
                },
            );
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

            let mut block = [0u8; BLOCK_SIZE];
            self.read_block(block_addr, &mut block)?;
            if let Some(entry) = DirEntry::from_bytes(&block[block_offset..])
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
            self.read_block(old_block_addr, &mut block_data)?;
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
        // First 8 bytes are reserved for next-block pointer, each entry is 16 bytes
        let entries_per_block = (BLOCK_SIZE - 8) / 16;
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

        // First 8 bytes are reserved for next-block pointer, each entry is 16 bytes
        let entries_per_block = (BLOCK_SIZE - 8) / 16;

        while block_addr.is_valid() {
            let mut block = [0u8; BLOCK_SIZE];
            self.read_block(block_addr, &mut block)?;

            let next_block = BlockAddress::new(u64::from_le_bytes(block[0..8].try_into().unwrap()));

            for i in 0..entries_per_block {
                let offset = 8 + i * 16;
                let ino = InodeNumber::new(u64::from_le_bytes(
                    block[offset..offset + 8].try_into().unwrap(),
                ));
                let addr = BlockAddress::new(u64::from_le_bytes(
                    block[offset + 8..offset + 16].try_into().unwrap(),
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

impl Lfs<MemoryBlockDevice> {
    /// Creates a new LFS on the given buffer.
    ///
    /// This is a convenience constructor for using a `Vec<u8>` as the backing store.
    /// The buffer must be at least 16 blocks (64KB) in size.
    pub fn from_vec(data: Vec<u8>) -> Result<Self> {
        let total_blocks = (data.len() / BLOCK_SIZE) as u64;
        let device = MemoryBlockDevice::new(data);
        Self::new(device, total_blocks)
    }

    /// Opens an existing LFS from the given buffer.
    ///
    /// This is a convenience constructor for using a `Vec<u8>` as the backing store.
    /// The tail position is read from the superblock on disk.
    pub fn open_vec(data: Vec<u8>) -> Result<Self> {
        let total_blocks = (data.len() / BLOCK_SIZE) as u64;
        let device = MemoryBlockDevice::new(data);
        Self::open(device, total_blocks)
    }

    /// Consumes the filesystem and returns the underlying data buffer.
    pub fn into_inner(self) -> Vec<u8> {
        self.device.into_inner()
    }

    /// Returns a reference to the underlying data.
    pub fn data(&self) -> &[u8] {
        self.device.data()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_fs(blocks: usize) -> Lfs<MemoryBlockDevice> {
        let data = vec![0u8; blocks * BLOCK_SIZE];
        Lfs::from_vec(data).expect("Failed to create filesystem")
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
        let mut lfs = Lfs::from_vec(data).expect("Failed to create filesystem");

        let fd = lfs.open_file("persist.txt").expect("Failed to open file");
        lfs.write(fd, b"Saved data").expect("Failed to write");
        lfs.close(fd).expect("Failed to close");

        println!("Tail position: {}", lfs.tail().as_u64());

        let data = lfs.into_inner();

        let mut lfs2 = Lfs::open_vec(data).expect("Failed to reopen filesystem");
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
        let result = Lfs::from_vec(data);
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
        let mut lfs = Lfs::from_vec(data).expect("Failed to create filesystem");

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

        let data = lfs.into_inner();

        let mut lfs2 = Lfs::open_vec(data).expect("Failed to reopen filesystem");

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
        let lfs = Lfs::from_vec(data).expect("Minimum size filesystem should work");
        assert!(lfs.free_blocks() > 0);
        println!("Minimum filesystem has {} free blocks", lfs.free_blocks());
    }

    #[test]
    fn filesystem_15_blocks_fails() {
        let data = vec![0u8; 15 * BLOCK_SIZE];
        let result = Lfs::from_vec(data);
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

        let result = Lfs::open_vec(data);
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

    #[test]
    fn debug_inode_map_persist_restore() {
        // Create filesystem and write a file
        let data = vec![0u8; 64 * BLOCK_SIZE];
        let mut lfs = Lfs::from_vec(data).expect("Failed to create filesystem");

        let fd = lfs.open_file("test.txt").expect("Failed to open file");
        let write_data = b"Hello, World!";
        lfs.write(fd, write_data).expect("Failed to write");
        lfs.close(fd).expect("Failed to close");

        // Check inode map before persist
        println!("Inode map before persist: {:?}", lfs.inode_map);
        println!(
            "Superblock inode_map_block: {:?}",
            lfs.superblock.inode_map_block.as_u64()
        );

        println!("Tail: {}", lfs.tail().as_u64());
        let data = lfs.into_inner();

        // Restore and check inode map
        let mut lfs2 = Lfs::open_vec(data).expect("Failed to restore");
        println!("Inode map after restore: {:?}", lfs2.inode_map);
        println!(
            "Superblock inode_map_block after restore: {:?}",
            lfs2.superblock.inode_map_block.as_u64()
        );

        // Try to open the file
        let fd2 = lfs2
            .open_file("test.txt")
            .expect("Failed to open after restore");
        let size = lfs2.file_size(fd2).expect("Failed to get size");
        println!("File size after restore: {}", size);

        let mut buf = vec![0u8; 20];
        let n = lfs2.read(fd2, &mut buf).expect("Failed to read");
        println!("Read {} bytes: {:?}", n, &buf[..n]);

        assert_eq!(n, write_data.len(), "Size mismatch");
        assert_eq!(&buf[..n], write_data, "Data mismatch");
    }

    #[test]
    fn debug_open_after_restore_creates_new_file() {
        // This test checks if opening a NEW file after restore works correctly
        let data = vec![0u8; 64 * BLOCK_SIZE];
        let mut lfs = Lfs::from_vec(data).expect("Failed to create filesystem");

        // Write to file a.txt
        let fd = lfs.open_file("a.txt").expect("open a.txt");
        lfs.write(fd, b"Content A").expect("write");
        lfs.close(fd).expect("close");

        let data = lfs.into_inner();

        // Restore
        let mut lfs2 = Lfs::open_vec(data).expect("restore");

        // Open a NEW file b.txt (not existing before)
        let fd_b = lfs2.open_file("b.txt").expect("open b.txt");
        lfs2.write(fd_b, b"Content B").expect("write b");
        lfs2.close(fd_b).expect("close b");

        // Now open a.txt and verify its content
        let fd_a = lfs2.open_file("a.txt").expect("open a.txt after");
        let mut buf = vec![0u8; 20];
        let n = lfs2.read(fd_a, &mut buf).expect("read a");
        println!(
            "a.txt: read {} bytes: {:?}",
            n,
            String::from_utf8_lossy(&buf[..n])
        );
        assert_eq!(&buf[..n], b"Content A", "a.txt content mismatch");

        // Open b.txt and verify
        let fd_b2 = lfs2.open_file("b.txt").expect("open b.txt after");
        let mut buf2 = vec![0u8; 20];
        let n2 = lfs2.read(fd_b2, &mut buf2).expect("read b");
        println!(
            "b.txt: read {} bytes: {:?}",
            n2,
            String::from_utf8_lossy(&buf2[..n2])
        );
        assert_eq!(&buf2[..n2], b"Content B", "b.txt content mismatch");
    }

    #[test]
    fn debug_large_write_after_restore() {
        // Test with larger data that spans multiple blocks
        let data = vec![0u8; 256 * BLOCK_SIZE];
        let mut lfs = Lfs::from_vec(data).expect("create fs");

        let fd = lfs.open_file("large.txt").expect("open");
        let write_data: Vec<u8> = (0..10000).map(|i| (i % 256) as u8).collect();
        lfs.write(fd, &write_data).expect("write");
        lfs.close(fd).expect("close");

        println!("Before restore: inode_map = {:?}", lfs.inode_map);

        let data = lfs.into_inner();

        let mut lfs2 = Lfs::open_vec(data).expect("restore");
        println!("After restore: inode_map = {:?}", lfs2.inode_map);

        let fd2 = lfs2.open_file("large.txt").expect("open after restore");
        let size = lfs2.file_size(fd2).expect("size");
        println!("File size after restore: {}", size);
        assert_eq!(size, 10000, "Size mismatch");

        let mut buf = vec![0u8; 10000];
        let n = lfs2.read(fd2, &mut buf).expect("read");
        println!("Read {} bytes, first 20: {:?}", n, &buf[..20]);

        // Find first mismatch if any
        for i in 0..n {
            if buf[i] != write_data[i] {
                println!(
                    "First mismatch at {}: got {}, expected {}",
                    i, buf[i], write_data[i]
                );
                break;
            }
        }

        assert_eq!(n, 10000, "Read size mismatch");
        assert_eq!(&buf[..], &write_data[..], "Data mismatch");
    }

    #[test]
    fn debug_many_files_persist_restore() {
        // Test with enough files to potentially overflow one inode map block
        // (BLOCK_SIZE - 8) / 16 = 255 entries per block
        let data = vec![0u8; 512 * BLOCK_SIZE];
        let mut lfs = Lfs::from_vec(data).expect("create fs");

        // Create 10 files
        for i in 0..10 {
            let name = format!("file{}.txt", i);
            let fd = lfs.open_file(&name).expect("open");
            let content = format!("Content of file {}", i);
            lfs.write(fd, content.as_bytes()).expect("write");
            lfs.close(fd).expect("close");
        }

        println!("Before restore: {} inodes", lfs.inode_map.len());

        let data = lfs.into_inner();

        let mut lfs2 = Lfs::open_vec(data).expect("restore");
        println!("After restore: {} inodes", lfs2.inode_map.len());

        // Verify all files
        for i in 0..10 {
            let name = format!("file{}.txt", i);
            let expected = format!("Content of file {}", i);
            let fd = lfs2.open_file(&name).expect("open after restore");
            let mut buf = vec![0u8; 50];
            let n = lfs2.read(fd, &mut buf).expect("read");
            let got = String::from_utf8_lossy(&buf[..n]).to_string();
            println!("file{}.txt: '{}'", i, got);
            assert_eq!(got, expected, "Content mismatch for file{}.txt", i);
            lfs2.close(fd).expect("close");
        }
    }

    #[test]
    fn debug_regression_open_open_write() {
        // Regression case: [Open { name: "a.txt" }, Open { name: "a.txt" }, Write { fd_index: 0, data: [...] }]
        let write_data: Vec<u8> = vec![
            252, 3, 189, 132, 108, 138, 241, 109, 95, 125, 118, 177, 187, 98, 111, 236, 12, 241,
            68, 146, 38, 187, 212, 122, 139, 96, 173, 125, 97, 11, 155, 202, 35, 87, 33, 4, 144,
            101, 61, 105, 103, 36, 148, 154, 138, 187, 158, 123, 97, 46, 90, 48, 36, 10, 93, 96,
            64, 91, 82, 121, 211, 40, 20, 103, 71, 186, 80, 43, 159, 116, 21, 100, 157, 155, 236,
            169, 100, 119, 239, 135, 128, 33, 98, 23, 52, 209, 130, 226, 112, 208, 248, 112, 163,
            77, 45, 255, 238, 83, 133, 251, 194, 247, 241, 82, 99, 112, 82, 234, 203, 209, 32, 129,
            252, 132, 238, 164, 81, 221, 143, 230, 93, 140, 153, 70, 81, 176, 245, 207, 32, 23,
            229, 129, 208, 107, 219, 160, 226, 89, 81, 171, 139, 72, 49, 101, 194, 238, 2, 6, 14,
            83, 62, 10, 157, 72, 51, 192, 132, 21, 182, 57, 110, 133, 155, 16, 176, 131, 34, 74,
            134, 24, 189, 106, 179, 91, 210, 87, 182, 249, 228, 219, 224, 209, 124, 151, 217, 172,
            153, 32, 48, 179, 61, 0, 64, 75, 67, 89, 206, 159, 241, 145, 241, 119, 162, 111, 160,
            197, 43, 154, 70, 74, 70, 170, 217, 56, 127, 245, 34, 152, 186, 215, 123, 202, 147, 29,
            225, 150, 55, 178, 47, 36, 99, 84, 147, 56, 188, 189, 224, 243, 251, 28, 228, 163, 187,
            144, 47, 176, 39, 41, 111, 160, 171, 25, 140, 68, 238, 6,
        ];

        let data = vec![0u8; 256 * BLOCK_SIZE];
        let mut lfs = Lfs::from_vec(data).expect("create fs");

        // Open { name: "a.txt" }
        let fd0 = lfs.open_file("a.txt").expect("open 1");
        // Open { name: "a.txt" }
        let _fd1 = lfs.open_file("a.txt").expect("open 2");
        // Write { fd_index: 0, data: [...] }
        lfs.write(fd0, &write_data).expect("write");

        let data = lfs.into_inner();

        let mut lfs_verify = Lfs::open_vec(data).expect("reopen");

        let fd = lfs_verify.open_file("a.txt").expect("open for verify");
        lfs_verify.seek(fd, 0).expect("seek");

        let mut buf = vec![0u8; write_data.len()];
        let n = lfs_verify.read(fd, &mut buf).expect("read");

        println!("Expected {} bytes, got {}", write_data.len(), n);
        println!("First 16 expected: {:?}", &write_data[..16]);
        println!("First 16 got: {:?}", &buf[..16.min(n)]);

        assert_eq!(n, write_data.len(), "Size mismatch");
        assert_eq!(&buf[..n], &write_data[..], "Data mismatch");
    }

    #[test]
    fn debug_regression_open_open_write_large() {
        // Same pattern but with data large enough to span multiple blocks
        let write_data: Vec<u8> = (0..8000).map(|i| (i % 256) as u8).collect();

        let data = vec![0u8; 256 * BLOCK_SIZE];
        let mut lfs = Lfs::from_vec(data).expect("create fs");

        let fd0 = lfs.open_file("a.txt").expect("open 1");
        let _fd1 = lfs.open_file("a.txt").expect("open 2");
        lfs.write(fd0, &write_data).expect("write");

        let data = lfs.into_inner();

        let mut lfs_verify = Lfs::open_vec(data).expect("reopen");

        let fd = lfs_verify.open_file("a.txt").expect("open for verify");
        lfs_verify.seek(fd, 0).expect("seek");

        let mut buf = vec![0u8; write_data.len()];
        let n = lfs_verify.read(fd, &mut buf).expect("read");

        println!("Expected {} bytes, got {}", write_data.len(), n);

        // Find first mismatch
        for i in 0..n.min(write_data.len()) {
            if buf[i] != write_data[i] {
                println!(
                    "First mismatch at byte {}: got {}, expected {}",
                    i, buf[i], write_data[i]
                );
                break;
            }
        }

        assert_eq!(n, write_data.len(), "Size mismatch");
        assert_eq!(&buf[..n], &write_data[..], "Data mismatch");
    }

    #[test]
    fn debug_regression_seek_truncate_write() {
        // Exact failing case from proptest:
        // Open a.txt 5 times, Seek fd4 to 18509, Write 10642 bytes, Truncate to 29150, Write 9688 bytes
        let data = vec![0u8; 256 * BLOCK_SIZE];
        let mut lfs = Lfs::from_vec(data).expect("create fs");

        // Open "a.txt" 5 times
        let _fd0 = lfs.open_file("a.txt").expect("open 0");
        let _fd1 = lfs.open_file("a.txt").expect("open 1");
        let _fd2 = lfs.open_file("a.txt").expect("open 2");
        let _fd3 = lfs.open_file("a.txt").expect("open 3");
        let fd4 = lfs.open_file("a.txt").expect("open 4");

        // Seek fd4 to pos 18509
        lfs.seek(fd4, 18509).expect("seek");

        // Write 10642 bytes
        let write1: Vec<u8> = (0..10642).map(|i| (i % 256) as u8).collect();
        lfs.write(fd4, &write1).expect("write 1");

        // Truncate to 29150
        lfs.truncate(fd4, 29150).expect("truncate");

        // Write 9688 bytes
        let write2: Vec<u8> = (0..9688).map(|i| ((i + 100) % 256) as u8).collect();
        lfs.write(fd4, &write2).expect("write 2");

        // Build expected data (what reference implementation would have)
        let mut expected = vec![0u8; 29150];
        // After seek to 18509 and write 10642 bytes: positions 18509..29151
        for (i, &b) in write1.iter().enumerate() {
            let pos = 18509 + i;
            if pos < expected.len() {
                expected[pos] = b;
            }
        }
        // After truncate to 29150: expected is already 29150 long, but data beyond 29150 is gone
        // The write of 10642 bytes ends at 18509+10642=29151, but truncate cuts it to 29150
        // So position 29150 is gone
        expected.truncate(29150);
        // The seek position after truncate depends on implementation...
        // Actually the file descriptor position was at 18509+10642=29151, truncate doesn't change it
        // Then write2 starts at position 29151 which extends the file
        // So final size should be 29151 + 9688 = 38839
        let final_size = 29151 + 9688;
        expected.resize(final_size, 0);
        for (i, &b) in write2.iter().enumerate() {
            expected[29151 + i] = b;
        }

        // Now verify
        let data = lfs.into_inner();
        let mut lfs_verify = Lfs::open_vec(data).expect("reopen");

        let fd = lfs_verify.open_file("a.txt").expect("open for verify");
        lfs_verify.seek(fd, 0).expect("seek to start");

        let size = lfs_verify.file_size(fd).expect("get size");
        println!("File size: {}, expected: {}", size, final_size);

        let mut buf = vec![0u8; final_size];
        let n = lfs_verify.read(fd, &mut buf).expect("read");
        println!("Read {} bytes", n);

        // Find first mismatch
        for i in 0..n.min(expected.len()) {
            if buf[i] != expected[i] {
                println!(
                    "First mismatch at byte {}: got {}, expected {}",
                    i, buf[i], expected[i]
                );
                println!(
                    "Context: buf[{}..{}] = {:?}",
                    i.saturating_sub(5),
                    (i + 10).min(n),
                    &buf[i.saturating_sub(5)..(i + 10).min(n)]
                );
                println!(
                    "Context: expected[{}..{}] = {:?}",
                    i.saturating_sub(5),
                    (i + 10).min(expected.len()),
                    &expected[i.saturating_sub(5)..(i + 10).min(expected.len())]
                );
                break;
            }
        }

        assert_eq!(n, final_size, "Size mismatch");
        assert_eq!(&buf[..n], &expected[..], "Data mismatch");
    }

    #[test]
    fn debug_truncate_zeroing() {
        // Simpler test: does truncate properly zero data?
        let data = vec![0u8; 64 * BLOCK_SIZE];
        let mut lfs = Lfs::from_vec(data).expect("create fs");

        let fd = lfs.open_file("test.txt").expect("open");

        // Write 1000 bytes of 0xFF
        let write1 = vec![0xFF; 1000];
        lfs.write(fd, &write1).expect("write");

        // Truncate to 500
        lfs.truncate(fd, 500).expect("truncate");

        // Verify size
        let size = lfs.file_size(fd).expect("size");
        println!("Size after truncate: {}", size);
        assert_eq!(size, 500);

        // Read and verify - should be 500 bytes of 0xFF
        lfs.seek(fd, 0).expect("seek");
        let mut buf = vec![0u8; 500];
        let n = lfs.read(fd, &mut buf).expect("read");
        assert_eq!(n, 500);
        assert!(buf.iter().all(|&b| b == 0xFF), "Data should be 0xFF");

        // Now persist/restore
        let data = lfs.into_inner();
        let mut lfs2 = Lfs::open_vec(data).expect("restore");

        let fd2 = lfs2.open_file("test.txt").expect("open after restore");
        let size2 = lfs2.file_size(fd2).expect("size after restore");
        println!("Size after restore: {}", size2);
        assert_eq!(size2, 500, "Size changed after restore!");

        let mut buf2 = vec![0u8; 500];
        let n2 = lfs2.read(fd2, &mut buf2).expect("read after restore");
        assert_eq!(n2, 500);
        assert!(
            buf2.iter().all(|&b| b == 0xFF),
            "Data should still be 0xFF after restore"
        );
    }

    #[test]
    fn truncate_then_write_zeros_gap() {
        // Test: write n bytes, truncate to n-1, then write more.
        // The byte at position n-1 (after truncate and before new write) should be zero.
        let data = vec![0u8; 64 * BLOCK_SIZE];
        let mut lfs = Lfs::from_vec(data).expect("create fs");

        let fd = lfs.open_file("test.txt").expect("open");

        // Write 100 bytes of 0xFF
        let write1 = vec![0xFF; 100];
        lfs.write(fd, &write1).expect("write 100 bytes");
        // Position is now at 100

        // Truncate to 50 - this should logically zero bytes 50..100
        lfs.truncate(fd, 50).expect("truncate to 50");

        // Write 10 more bytes at position 100 (seek there first)
        // This creates a hole from 50 to 100 that should be zeros
        lfs.seek(fd, 100).expect("seek to 100");
        let write2 = vec![0xAA; 10];
        lfs.write(fd, &write2).expect("write 10 bytes at 100");

        // Now read the whole file and verify
        lfs.seek(fd, 0).expect("seek to start");
        let mut buf = vec![0u8; 110];
        let n = lfs.read(fd, &mut buf).expect("read all");

        println!("Read {} bytes", n);
        println!("Bytes 0..50 (should be 0xFF): {:?}", &buf[0..50]);
        println!("Bytes 50..100 (should be 0x00): {:?}", &buf[50..100.min(n)]);
        println!("Bytes 100..110 (should be 0xAA): {:?}", &buf[100.min(n)..n]);

        assert_eq!(n, 110, "File size should be 110");
        assert!(
            buf[0..50].iter().all(|&b| b == 0xFF),
            "First 50 bytes should be 0xFF"
        );
        assert!(
            buf[50..100].iter().all(|&b| b == 0x00),
            "Bytes 50..100 should be 0x00 (truncated region)"
        );
        assert!(
            buf[100..110].iter().all(|&b| b == 0xAA),
            "Last 10 bytes should be 0xAA"
        );
    }

    #[test]
    fn sequential_block_device_enforces_ordering() {
        let data = vec![0u8; 64 * BLOCK_SIZE];
        let mem_device = MemoryBlockDevice::new(data);
        let log_start = BlockAddress::new(1);
        let log_end = BlockAddress::new(64);
        let device = SequentialBlockDevice::new(mem_device, log_start, log_end);

        let mut lfs = Lfs::new(device, 64).expect("create fs with sequential device");

        let fd = lfs.open_file("test.txt").expect("open");
        lfs.write(fd, b"Hello, sequential world!")
            .expect("write to sequential device");
        lfs.close(fd).expect("close");
        println!("Write succeeded on sequential block device");

        let fd = lfs.open_file("test.txt").expect("reopen");
        let mut buf = vec![0u8; 24];
        lfs.read(fd, &mut buf).expect("read");
        assert_eq!(&buf, b"Hello, sequential world!");
        println!("Data verified: {:?}", String::from_utf8_lossy(&buf));
        lfs.close(fd).expect("close");
    }

    #[test]
    fn sequential_block_device_with_multiple_files() {
        let data = vec![0u8; 128 * BLOCK_SIZE];
        let mem_device = MemoryBlockDevice::new(data);
        let log_start = BlockAddress::new(1);
        let log_end = BlockAddress::new(128);
        let device = SequentialBlockDevice::new(mem_device, log_start, log_end);

        let mut lfs = Lfs::new(device, 128).expect("create fs with sequential device");

        for i in 0..5 {
            let name = format!("file{}.txt", i);
            let fd = lfs.open_file(&name).expect("open");
            let content = format!("Content of file {} with some padding data", i);
            lfs.write(fd, content.as_bytes()).expect("write");
            lfs.close(fd).expect("close");
        }
        println!("Created 5 files on sequential block device");

        for i in 0..5 {
            let name = format!("file{}.txt", i);
            let fd = lfs.open_file(&name).expect("reopen");
            let expected = format!("Content of file {} with some padding data", i);
            let mut buf = vec![0u8; expected.len()];
            lfs.read(fd, &mut buf).expect("read");
            assert_eq!(buf, expected.as_bytes());
            lfs.close(fd).expect("close");
        }
        println!("All 5 files verified on sequential block device");
    }

    #[test]
    fn sequential_block_device_with_large_write() {
        let data = vec![0u8; 256 * BLOCK_SIZE];
        let mem_device = MemoryBlockDevice::new(data);
        let log_start = BlockAddress::new(1);
        let log_end = BlockAddress::new(256);
        let device = SequentialBlockDevice::new(mem_device, log_start, log_end);

        let mut lfs = Lfs::new(device, 256).expect("create fs with sequential device");

        let fd = lfs.open_file("large.bin").expect("open");
        let large_data: Vec<u8> = (0..BLOCK_SIZE * 10).map(|i| (i % 256) as u8).collect();
        lfs.write(fd, &large_data)
            .expect("write large data to sequential device");
        lfs.close(fd).expect("close");
        println!(
            "Wrote {} bytes across multiple blocks sequentially",
            large_data.len()
        );

        let fd = lfs.open_file("large.bin").expect("reopen");
        let mut buf = vec![0u8; large_data.len()];
        lfs.read(fd, &mut buf).expect("read");
        assert_eq!(buf, large_data);
        lfs.close(fd).expect("close");
        println!("Large file data verified on sequential block device");
    }

    #[test]
    fn sequential_block_device_with_overwrites() {
        let data = vec![0u8; 64 * BLOCK_SIZE];
        let mem_device = MemoryBlockDevice::new(data);
        let log_start = BlockAddress::new(1);
        let log_end = BlockAddress::new(64);
        let device = SequentialBlockDevice::new(mem_device, log_start, log_end);

        let mut lfs = Lfs::new(device, 64).expect("create fs with sequential device");

        let fd = lfs.open_file("overwrite.txt").expect("open");
        lfs.write(fd, b"First version of data")
            .expect("write first version");
        lfs.seek(fd, 0).expect("seek to start");
        lfs.write(fd, b"Second version!!!!!!!!")
            .expect("write second version (should allocate new block sequentially)");
        lfs.close(fd).expect("close");
        println!("Overwrites succeeded on sequential block device");

        let fd = lfs.open_file("overwrite.txt").expect("reopen");
        let mut buf = vec![0u8; 22];
        lfs.read(fd, &mut buf).expect("read");
        assert_eq!(&buf, b"Second version!!!!!!!!");
        println!(
            "Overwritten data verified: {:?}",
            String::from_utf8_lossy(&buf)
        );
        lfs.close(fd).expect("close");
    }

    #[test]
    fn remove_file() {
        let mut lfs = create_test_fs(64);

        let fd = lfs.open_file("removeme.txt").expect("create file");
        lfs.write(fd, b"This file will be removed").expect("write");
        lfs.close(fd).expect("close");

        lfs.remove("removeme.txt").expect("remove file");
        println!("File removed successfully");

        let result = lfs.open_file("removeme.txt");
        let fd = result.expect("opening removed filename creates new file");
        let mut buf = vec![0u8; 10];
        let n = lfs.read(fd, &mut buf).expect("read");
        assert_eq!(n, 0, "New file should be empty");
        lfs.close(fd).expect("close");
        println!("Confirmed: opening removed filename creates fresh empty file");
    }

    #[test]
    fn remove_nonexistent_file() {
        let mut lfs = create_test_fs(64);

        let result = lfs.remove("nonexistent.txt");
        assert_eq!(result, Err(Error::NotFound));
        println!("Removing nonexistent file correctly returns NotFound");
    }

    #[test]
    fn remove_open_file_unix_semantics() {
        let mut lfs = create_test_fs(64);

        let fd = lfs.open_file("open.txt").expect("create file");
        lfs.write(fd, b"File is open").expect("write");

        lfs.remove("open.txt").expect("remove while open succeeds");
        println!("Removing open file succeeds (UNIX semantics)");

        lfs.seek(fd, 0).expect("seek");
        let mut buf = vec![0u8; 12];
        let n = lfs.read(fd, &mut buf).expect("read from unlinked file");
        assert_eq!(n, 12);
        assert_eq!(&buf, b"File is open");
        println!("Can still read from unlinked file via open FD");

        lfs.write(fd, b" - more data")
            .expect("write to unlinked file");
        println!("Can still write to unlinked file via open FD");

        lfs.close(fd).expect("close");
        println!("Closed FD, file should now be fully removed");

        let fd2 = lfs.open_file("open.txt").expect("recreate file");
        let mut buf2 = vec![0u8; 10];
        let n2 = lfs.read(fd2, &mut buf2).expect("read");
        assert_eq!(n2, 0, "Recreated file should be empty");
        lfs.close(fd2).expect("close");
        println!("Recreated file is empty as expected");
    }

    #[test]
    fn remove_and_recreate() {
        let mut lfs = create_test_fs(64);

        let fd = lfs.open_file("recreate.txt").expect("create file");
        lfs.write(fd, b"Original content").expect("write");
        lfs.close(fd).expect("close");

        lfs.remove("recreate.txt").expect("remove");

        let fd = lfs.open_file("recreate.txt").expect("recreate file");
        lfs.write(fd, b"New content").expect("write new content");
        lfs.close(fd).expect("close");

        let fd = lfs.open_file("recreate.txt").expect("reopen");
        let mut buf = vec![0u8; 11];
        lfs.read(fd, &mut buf).expect("read");
        assert_eq!(&buf, b"New content");
        lfs.close(fd).expect("close");
        println!("File recreated with new content successfully");
    }

    #[test]
    fn remove_persists_across_restore() {
        let data = vec![0u8; 64 * BLOCK_SIZE];
        let mut lfs = Lfs::from_vec(data).expect("create fs");

        let fd = lfs.open_file("persist.txt").expect("create file");
        lfs.write(fd, b"Will be removed").expect("write");
        lfs.close(fd).expect("close");

        lfs.remove("persist.txt").expect("remove");

        let data = lfs.into_inner();
        let mut lfs2 = Lfs::open_vec(data).expect("restore fs");

        let fd = lfs2
            .open_file("persist.txt")
            .expect("open creates new file");
        let mut buf = vec![0u8; 10];
        let n = lfs2.read(fd, &mut buf).expect("read");
        assert_eq!(n, 0, "File should be empty after restore");
        lfs2.close(fd).expect("close");
        println!("Remove persists correctly across restore");
    }

    #[test]
    fn remove_multiple_files() {
        let mut lfs = create_test_fs(128);

        for i in 0..5 {
            let name = format!("file{}.txt", i);
            let fd = lfs.open_file(&name).expect("create file");
            let content = format!("Content {}", i);
            lfs.write(fd, content.as_bytes()).expect("write");
            lfs.close(fd).expect("close");
        }

        lfs.remove("file1.txt").expect("remove file1");
        lfs.remove("file3.txt").expect("remove file3");

        let fd = lfs.open_file("file0.txt").expect("open file0");
        let mut buf = vec![0u8; 9];
        lfs.read(fd, &mut buf).expect("read");
        assert_eq!(&buf, b"Content 0");
        lfs.close(fd).expect("close");

        let fd = lfs.open_file("file2.txt").expect("open file2");
        let mut buf = vec![0u8; 9];
        lfs.read(fd, &mut buf).expect("read");
        assert_eq!(&buf, b"Content 2");
        lfs.close(fd).expect("close");

        let fd = lfs.open_file("file4.txt").expect("open file4");
        let mut buf = vec![0u8; 9];
        lfs.read(fd, &mut buf).expect("read");
        assert_eq!(&buf, b"Content 4");
        lfs.close(fd).expect("close");

        let fd = lfs.open_file("file1.txt").expect("file1 recreated");
        let mut buf = vec![0u8; 10];
        let n = lfs.read(fd, &mut buf).expect("read");
        assert_eq!(n, 0, "Recreated file1 should be empty");
        lfs.close(fd).expect("close");

        println!("Multiple file removal works correctly");
    }
}

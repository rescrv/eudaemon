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
const DIRECT_BLOCKS: usize = 9;

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

/////////////////////////////////////////// FileBlockDevice ////////////////////////////////////////////

use std::fs::File;
use std::fs::OpenOptions;
use std::io::Read;
use std::io::Seek;
use std::io::SeekFrom;
use std::io::Write;
use std::path::Path;
use std::sync::Mutex;

/// A block device backed by a file on disk.
///
/// This implementation uses interior mutability via `Mutex` to allow the
/// `read_block` method to work with `&self` while still performing file I/O.
pub struct FileBlockDevice {
    file: Mutex<File>,
    total_blocks: u64,
}

impl FileBlockDevice {
    /// Creates a new file block device, opening an existing file.
    ///
    /// The file must already exist. The total number of blocks is computed
    /// from the file size.
    pub fn open<P: AsRef<Path>>(path: P) -> std::io::Result<Self> {
        let file = OpenOptions::new().read(true).write(true).open(path)?;
        let metadata = file.metadata()?;
        let total_blocks = metadata.len() / BLOCK_SIZE as u64;
        Ok(Self {
            file: Mutex::new(file),
            total_blocks,
        })
    }

    /// Creates a new file block device, creating the file if it doesn't exist.
    ///
    /// If the file doesn't exist, it is created with the specified size.
    /// If the file exists, it is opened and the size parameter is ignored.
    pub fn create<P: AsRef<Path>>(path: P, total_blocks: u64) -> std::io::Result<Self> {
        let path = path.as_ref();
        if path.exists() {
            Self::open(path)
        } else {
            let file = OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(true)
                .open(path)?;
            let size = total_blocks * BLOCK_SIZE as u64;
            file.set_len(size)?;
            Ok(Self {
                file: Mutex::new(file),
                total_blocks,
            })
        }
    }

    /// Returns the total number of blocks in the device.
    pub fn total_blocks(&self) -> u64 {
        self.total_blocks
    }

    /// Syncs all pending writes to disk.
    pub fn sync(&self) -> std::io::Result<()> {
        let file = self.file.lock().unwrap();
        file.sync_all()
    }
}

impl BlockDevice for FileBlockDevice {
    fn read_block(&self, block: BlockAddress, buf: &mut [u8; BLOCK_SIZE]) -> Result<()> {
        if block.as_u64() >= self.total_blocks {
            return Err(Error::InvalidOffset);
        }
        let offset = block.byte_offset() as u64;
        let mut file = self.file.lock().unwrap();
        file.seek(SeekFrom::Start(offset))
            .map_err(|_| Error::CorruptFilesystem)?;
        file.read_exact(buf).map_err(|_| Error::CorruptFilesystem)?;
        Ok(())
    }

    fn write_block(&mut self, block: BlockAddress, buf: &[u8; BLOCK_SIZE]) -> Result<()> {
        if block.as_u64() >= self.total_blocks {
            return Err(Error::InvalidOffset);
        }
        let offset = block.byte_offset() as u64;
        let mut file = self.file.lock().unwrap();
        file.seek(SeekFrom::Start(offset))
            .map_err(|_| Error::CorruptFilesystem)?;
        file.write_all(buf).map_err(|_| Error::CorruptFilesystem)?;
        Ok(())
    }
}

/////////////////////////////////////////////// InodeType //////////////////////////////////////////////

/// The type of an inode (file, directory, or symlink).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InodeType {
    /// A regular file.
    File = 0,
    /// A directory.
    Directory = 1,
    /// A symbolic link.
    Symlink = 2,
}

impl InodeType {
    fn from_u8(val: u8) -> Option<Self> {
        match val {
            0 => Some(InodeType::File),
            1 => Some(InodeType::Directory),
            2 => Some(InodeType::Symlink),
            _ => None,
        }
    }
}

/////////////////////////////////////////////// FileType ////////////////////////////////////////////////

/// The type of a file system entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileType {
    /// A regular file.
    RegularFile,
    /// A directory.
    Directory,
    /// A symbolic link.
    Symlink,
    /// Other file type.
    Other,
}

impl From<InodeType> for FileType {
    fn from(inode_type: InodeType) -> Self {
        match inode_type {
            InodeType::File => FileType::RegularFile,
            InodeType::Directory => FileType::Directory,
            InodeType::Symlink => FileType::Symlink,
        }
    }
}

/////////////////////////////////////////////// StatInfo ////////////////////////////////////////////////

/// Metadata information about a file or directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatInfo {
    /// The type of the file (regular file, directory, symlink, etc.).
    pub file_type: FileType,
    /// Size in bytes.
    pub size: u64,
    /// Access time in milliseconds since UNIX epoch.
    pub atime_ms: i64,
    /// Modification time in milliseconds since UNIX epoch.
    pub mtime_ms: i64,
    /// Device ID.
    pub dev: u64,
    /// Inode number.
    pub ino: u64,
    /// Number of hard links.
    pub link_count: u32,
}

/////////////////////////////////////////////// TimeSpec ////////////////////////////////////////////////

/// Specification for setting file timestamps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeSpec {
    /// Set to current time.
    Now,
    /// Leave unchanged.
    Omit,
    /// Set to specific milliseconds since UNIX epoch.
    Time(i64),
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

/////////////////////////////////////////////// DeviceId ////////////////////////////////////////////////

/// A strongly-typed device identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DeviceId(u64);

impl DeviceId {
    /// Creates a new device ID from a raw value.
    pub fn new(value: u64) -> Self {
        Self(value)
    }

    /// Returns the raw u64 value.
    pub fn as_u64(self) -> u64 {
        self.0
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
    /// The path refers to a directory, not a file.
    IsDirectory,
    /// The path refers to a file, not a directory.
    NotADirectory,
    /// The directory is not empty.
    DirectoryNotEmpty,
}

impl Error {
    /// Converts this error to a `std::io::Error`.
    pub fn to_io_error(self) -> std::io::Error {
        use std::io::ErrorKind;
        let kind = match self {
            Error::NotFound => ErrorKind::NotFound,
            Error::AlreadyExists => ErrorKind::AlreadyExists,
            Error::IsDirectory => ErrorKind::IsADirectory,
            Error::NotADirectory => ErrorKind::NotADirectory,
            Error::DirectoryNotEmpty => ErrorKind::DirectoryNotEmpty,
            Error::NoSpace => ErrorKind::StorageFull,
            Error::InvalidFd => ErrorKind::InvalidInput,
            Error::FilenameTooLong => ErrorKind::InvalidInput,
            Error::InvalidOffset => ErrorKind::InvalidInput,
            Error::FileTooLarge => ErrorKind::FileTooLarge,
            Error::InvalidArgument => ErrorKind::InvalidInput,
            Error::NotOpen => ErrorKind::InvalidInput,
            Error::CorruptFilesystem => ErrorKind::InvalidData,
            Error::BufferTooSmall => ErrorKind::InvalidInput,
        };
        std::io::Error::new(kind, format!("{:?}", self))
    }
}

impl From<Error> for std::io::Error {
    fn from(err: Error) -> Self {
        err.to_io_error()
    }
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
    /// Head of the log - oldest live data. Cleaner advances this.
    head: BlockAddress,
    /// Tail of the log - where new writes go.
    tail: BlockAddress,
    inode_map_block: BlockAddress,
    next_inode: InodeNumber,
}

impl Superblock {
    fn as_bytes(self) -> [u8; 72] {
        let mut buf = [0u8; 72];
        buf[0..8].copy_from_slice(&self.magic.to_le_bytes());
        buf[8..12].copy_from_slice(&self.block_size.to_le_bytes());
        buf[16..24].copy_from_slice(&self.total_blocks.to_le_bytes());
        buf[24..32].copy_from_slice(&self.log_start.as_u64().to_le_bytes());
        buf[32..40].copy_from_slice(&self.log_end.as_u64().to_le_bytes());
        buf[40..48].copy_from_slice(&self.head.as_u64().to_le_bytes());
        buf[48..56].copy_from_slice(&self.tail.as_u64().to_le_bytes());
        buf[56..64].copy_from_slice(&self.inode_map_block.as_u64().to_le_bytes());
        buf[64..72].copy_from_slice(&self.next_inode.as_u64().to_le_bytes());
        buf
    }

    fn from_bytes(buf: &[u8]) -> Option<Self> {
        if buf.len() < 72 {
            return None;
        }
        Some(Self {
            magic: u64::from_le_bytes(buf[0..8].try_into().ok()?),
            block_size: u32::from_le_bytes(buf[8..12].try_into().ok()?),
            total_blocks: u64::from_le_bytes(buf[16..24].try_into().ok()?),
            log_start: BlockAddress::new(u64::from_le_bytes(buf[24..32].try_into().ok()?)),
            log_end: BlockAddress::new(u64::from_le_bytes(buf[32..40].try_into().ok()?)),
            head: BlockAddress::new(u64::from_le_bytes(buf[40..48].try_into().ok()?)),
            tail: BlockAddress::new(u64::from_le_bytes(buf[48..56].try_into().ok()?)),
            inode_map_block: BlockAddress::new(u64::from_le_bytes(buf[56..64].try_into().ok()?)),
            next_inode: InodeNumber::new(u64::from_le_bytes(buf[64..72].try_into().ok()?)),
        })
    }
}

//////////////////////////////////////////////// Inode /////////////////////////////////////////////////

/// On-disk inode structure.
///
/// Layout (128 bytes):
/// - bytes 0-7: inode number (u64)
/// - byte 8: inode type (u8)
/// - bytes 9-12: link count (u32)
/// - bytes 13-15: reserved (3 bytes for alignment)
/// - bytes 16-23: size (u64)
/// - bytes 24-31: atime_ms (i64)
/// - bytes 32-39: mtime_ms (i64)
/// - bytes 40-111: direct block pointers (9 * 8 = 72 bytes)
/// - bytes 112-119: indirect block pointer (u64)
/// - bytes 120-127: double indirect block pointer (u64)
#[derive(Debug, Clone)]
struct Inode {
    ino: InodeNumber,
    inode_type: InodeType,
    link_count: u32,
    size: u64,
    atime_ms: i64,
    mtime_ms: i64,
    direct: [BlockAddress; DIRECT_BLOCKS],
    indirect: BlockAddress,
    double_indirect: BlockAddress,
}

impl Inode {
    fn new_file(ino: InodeNumber, now_ms: i64) -> Self {
        Self {
            ino,
            inode_type: InodeType::File,
            link_count: 1,
            size: 0,
            atime_ms: now_ms,
            mtime_ms: now_ms,
            direct: [BlockAddress::INVALID; DIRECT_BLOCKS],
            indirect: BlockAddress::INVALID,
            double_indirect: BlockAddress::INVALID,
        }
    }

    fn new_directory(ino: InodeNumber, now_ms: i64) -> Self {
        Self {
            ino,
            inode_type: InodeType::Directory,
            link_count: 1,
            size: 0,
            atime_ms: now_ms,
            mtime_ms: now_ms,
            direct: [BlockAddress::INVALID; DIRECT_BLOCKS],
            indirect: BlockAddress::INVALID,
            double_indirect: BlockAddress::INVALID,
        }
    }

    fn new_symlink(ino: InodeNumber, now_ms: i64) -> Self {
        Self {
            ino,
            inode_type: InodeType::Symlink,
            link_count: 1,
            size: 0,
            atime_ms: now_ms,
            mtime_ms: now_ms,
            direct: [BlockAddress::INVALID; DIRECT_BLOCKS],
            indirect: BlockAddress::INVALID,
            double_indirect: BlockAddress::INVALID,
        }
    }

    fn is_directory(&self) -> bool {
        self.inode_type == InodeType::Directory
    }

    fn is_symlink(&self) -> bool {
        self.inode_type == InodeType::Symlink
    }

    fn to_stat_info(&self, dev: DeviceId) -> StatInfo {
        StatInfo {
            file_type: self.inode_type.into(),
            size: self.size,
            atime_ms: self.atime_ms,
            mtime_ms: self.mtime_ms,
            dev: dev.as_u64(),
            ino: self.ino.as_u64(),
            link_count: self.link_count,
        }
    }

    fn to_bytes(&self) -> [u8; 128] {
        let mut buf = [0u8; 128];
        buf[0..8].copy_from_slice(&self.ino.as_u64().to_le_bytes());
        buf[8] = self.inode_type as u8;
        buf[9..13].copy_from_slice(&self.link_count.to_le_bytes());
        // bytes 13-15 reserved for alignment
        buf[16..24].copy_from_slice(&self.size.to_le_bytes());
        buf[24..32].copy_from_slice(&self.atime_ms.to_le_bytes());
        buf[32..40].copy_from_slice(&self.mtime_ms.to_le_bytes());
        for (i, &block) in self.direct.iter().enumerate() {
            let offset = 40 + i * 8;
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
            let offset = 40 + i * 8;
            *block =
                BlockAddress::new(u64::from_le_bytes(buf[offset..offset + 8].try_into().ok()?));
        }
        Some(Self {
            ino: InodeNumber::new(u64::from_le_bytes(buf[0..8].try_into().ok()?)),
            inode_type: InodeType::from_u8(buf[8]).unwrap_or(InodeType::File),
            link_count: u32::from_le_bytes(buf[9..13].try_into().ok()?),
            size: u64::from_le_bytes(buf[16..24].try_into().ok()?),
            atime_ms: i64::from_le_bytes(buf[24..32].try_into().ok()?),
            mtime_ms: i64::from_le_bytes(buf[32..40].try_into().ok()?),
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
pub struct Lfs<D: BlockDevice, T: Fn() -> i64> {
    device: D,
    superblock: Superblock,
    inode_map: BTreeMap<InodeNumber, BlockAddress>,
    segment_summary: BTreeMap<BlockAddress, SegmentSummaryEntry>,
    open_files: BTreeMap<FileDescriptor, OpenFile>,
    next_fd: FileDescriptor,
    max_file_size: u64,
    /// The committed head position from the last successful operation.
    /// Used to recover from partial writes on NoSpace errors.
    committed_head: BlockAddress,
    /// The committed tail position from the last successful operation.
    /// Used to recover from partial writes on NoSpace errors.
    committed_tail: BlockAddress,
    /// Inodes that have been unlinked but still have open file descriptors.
    /// These will be fully removed when the last FD is closed.
    unlinked_inodes: BTreeSet<InodeNumber>,
    /// Device ID for this filesystem instance.
    dev: DeviceId,
    /// Time source function returning milliseconds since UNIX epoch.
    time_source: T,
}

impl<D: BlockDevice, T: Fn() -> i64> Lfs<D, T> {
    /// Creates a new LFS on the given block device.
    ///
    /// # Arguments
    /// * `device` - The block device to use for storage.
    /// * `total_blocks` - Total number of blocks in the device (must be at least 16).
    /// * `dev` - Device ID for this filesystem instance.
    /// * `time_source` - Function returning current time in milliseconds since UNIX epoch.
    pub fn new(mut device: D, total_blocks: u64, dev: DeviceId, time_source: T) -> Result<Self> {
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
            head: log_start,
            tail: log_start,
            inode_map_block: BlockAddress::INVALID,
            next_inode: InodeNumber::ROOT.next(),
        };

        let committed_tail = log_start;

        // Write the superblock to block 0
        let mut superblock_block = [0u8; BLOCK_SIZE];
        superblock_block[..72].copy_from_slice(&superblock.as_bytes());
        device.write_block(BlockAddress::new(0), &superblock_block)?;

        let mut lfs = Self {
            device,
            superblock,
            inode_map: BTreeMap::new(),
            segment_summary: BTreeMap::new(),
            open_files: BTreeMap::new(),
            next_fd: FileDescriptor::new(0),
            max_file_size,
            committed_head: log_start,
            committed_tail,
            unlinked_inodes: BTreeSet::new(),
            dev,
            time_source,
        };

        lfs.create_root_directory()?;
        lfs.committed_head = lfs.superblock.head;
        lfs.committed_tail = lfs.superblock.tail;

        Ok(lfs)
    }

    /// Opens an existing LFS from the given block device.
    ///
    /// # Arguments
    /// * `device` - The block device containing an existing filesystem.
    /// * `total_blocks` - Total number of blocks in the device.
    /// * `dev` - Device ID for this filesystem instance.
    /// * `time_source` - Function returning current time in milliseconds since UNIX epoch.
    pub fn open(device: D, total_blocks: u64, dev: DeviceId, time_source: T) -> Result<Self> {
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
        let head = superblock.head;
        let tail = superblock.tail;

        let mut lfs = Self {
            device,
            superblock,
            inode_map: BTreeMap::new(),
            segment_summary: BTreeMap::new(),
            open_files: BTreeMap::new(),
            next_fd: FileDescriptor::new(0),
            max_file_size,
            committed_head: head,
            committed_tail: tail,
            unlinked_inodes: BTreeSet::new(),
            dev,
            time_source,
        };

        lfs.load_inode_map()?;
        lfs.rebuild_segment_summary()?;

        Ok(lfs)
    }

    /// Returns the device ID for this filesystem.
    pub fn dev(&self) -> DeviceId {
        self.dev
    }

    /// Returns the current time in milliseconds since UNIX epoch.
    fn now_ms(&self) -> i64 {
        (self.time_source)()
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

    /// Gets metadata for a path, following symlinks.
    pub fn stat(&self, path: &str) -> Result<StatInfo> {
        let ino = self.resolve_path_to_inode(path)?;
        let inode = self.read_inode(ino)?;
        Ok(inode.to_stat_info(self.dev))
    }

    /// Gets metadata for a path, not following the final symlink component.
    pub fn lstat(&self, path: &str) -> Result<StatInfo> {
        let ino = self.resolve_path_no_follow_final(path)?;
        let inode = self.read_inode(ino)?;
        Ok(inode.to_stat_info(self.dev))
    }

    /// Resolves a path to its inode number, following all symlinks.
    fn resolve_path_to_inode(&self, path: &str) -> Result<InodeNumber> {
        let (parent_ino, name) = self.resolve_path(path)?;
        let parent_inode = self.read_inode(parent_ino)?;

        let ino = self
            .lookup_in_dir(&parent_inode, name)?
            .ok_or(Error::NotFound)?;

        // Follow symlink if it is one
        let inode = self.read_inode(ino)?;
        if inode.is_symlink() {
            let target = self.read_symlink_target(&inode)?;
            // Resolve the symlink target
            self.resolve_path_to_inode(&target)
        } else {
            Ok(ino)
        }
    }

    /// Lists directory contents with metadata.
    pub fn read_dir(&self, path: &str) -> Result<Vec<(String, StatInfo)>> {
        let ino = self.resolve_path_to_inode(path)?;
        let dir_inode = self.read_inode(ino)?;

        if !dir_inode.is_directory() {
            return Err(Error::NotADirectory);
        }

        let mut entries = Vec::new();
        let num_entries = dir_inode.size;

        for i in 0..num_entries {
            let offset = Self::dir_entry_offset(i);
            let block_idx = BlockIndex::from_byte_offset(offset);
            let block_offset = (offset % BLOCK_SIZE as u64) as usize;

            let block_addr = self.get_block_addr(&dir_inode, block_idx)?;
            if !block_addr.is_valid() {
                continue;
            }

            let mut block = [0u8; BLOCK_SIZE];
            self.read_block(block_addr, &mut block)?;

            if let Some(entry) = DirEntry::from_bytes(&block[block_offset..])
                && entry.ino.is_valid()
            {
                let entry_inode = self.read_inode(entry.ino)?;
                let stat_info = entry_inode.to_stat_info(self.dev);
                entries.push((entry.name().to_string(), stat_info));
            }
        }

        Ok(entries)
    }

    /// Checks if a path exists.
    pub fn exists(&self, path: &str) -> bool {
        self.resolve_path_to_inode(path).is_ok()
    }

    /// Checks if a path is a directory.
    pub fn is_dir(&self, path: &str) -> bool {
        match self.resolve_path_to_inode(path) {
            Ok(ino) => match self.read_inode(ino) {
                Ok(inode) => inode.is_directory(),
                Err(_) => false,
            },
            Err(_) => false,
        }
    }

    /// Reads entire file contents as bytes.
    pub fn read_file(&mut self, path: &str) -> Result<Vec<u8>> {
        let fd = self.open_file(path)?;
        let size = self.file_size(fd)?;
        let mut buf = vec![0u8; size as usize];
        self.read(fd, &mut buf)?;
        self.close(fd)?;
        Ok(buf)
    }

    /// Writes bytes to a file (create or overwrite).
    pub fn write_file(&mut self, path: &str, contents: &[u8]) -> Result<()> {
        let fd = self.open_file(path)?;
        self.truncate(fd, 0)?;
        self.write(fd, contents)?;
        self.close(fd)?;
        Ok(())
    }

    /// Appends bytes to a file (create if needed).
    pub fn append_file(&mut self, path: &str, contents: &[u8]) -> Result<()> {
        let fd = self.open_file(path)?;
        let size = self.file_size(fd)?;
        self.seek(fd, size)?;
        self.write(fd, contents)?;
        self.close(fd)?;
        Ok(())
    }

    /// Truncates or extends a file to the given size. Creates if not exists.
    pub fn truncate_path(&mut self, path: &str, size: u64) -> Result<()> {
        let fd = self.open_file(path)?;
        self.truncate(fd, size)?;
        self.close(fd)?;
        Ok(())
    }

    /// Truncates only if file exists. Returns Ok(false) if not found.
    pub fn truncate_existing(&mut self, path: &str, size: u64) -> Result<bool> {
        // Check if file exists first by trying to resolve the path
        let (parent_ino, name) = self.resolve_path(path)?;
        let parent_inode = self.read_inode(parent_ino)?;

        match self.lookup_in_dir(&parent_inode, name)? {
            Some(ino) => {
                // Check that it's not a directory
                let inode = self.read_inode(ino)?;
                if inode.is_directory() {
                    return Err(Error::IsDirectory);
                }
                let fd = self.open_file(path)?;
                self.truncate(fd, size)?;
                self.close(fd)?;
                Ok(true)
            }
            None => Ok(false),
        }
    }

    /// Sets access and modification times for a path (follows symlinks).
    pub fn set_times(&mut self, path: &str, atime: TimeSpec, mtime: TimeSpec) -> Result<()> {
        let ino = self.resolve_path_to_inode(path)?;
        self.set_times_for_inode(ino, atime, mtime)
    }

    /// Sets times without following symlinks.
    pub fn lset_times(&mut self, path: &str, atime: TimeSpec, mtime: TimeSpec) -> Result<()> {
        let ino = self.resolve_path_no_follow_final(path)?;
        self.set_times_for_inode(ino, atime, mtime)
    }

    fn set_times_for_inode(
        &mut self,
        ino: InodeNumber,
        atime: TimeSpec,
        mtime: TimeSpec,
    ) -> Result<()> {
        match self.set_times_for_inode_inner(ino, atime, mtime) {
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

    fn set_times_for_inode_inner(
        &mut self,
        ino: InodeNumber,
        atime: TimeSpec,
        mtime: TimeSpec,
    ) -> Result<()> {
        let mut inode = self.read_inode(ino)?;
        let now = self.now_ms();

        match atime {
            TimeSpec::Now => inode.atime_ms = now,
            TimeSpec::Time(t) => inode.atime_ms = t,
            TimeSpec::Omit => {}
        }

        match mtime {
            TimeSpec::Now => inode.mtime_ms = now,
            TimeSpec::Time(t) => inode.mtime_ms = t,
            TimeSpec::Omit => {}
        }

        self.write_inode(&inode)?;
        self.persist_inode_map()?;
        Ok(())
    }

    /// Creates an empty file if it doesn't exist.
    /// Returns true if created, false if already existed.
    pub fn create_file(&mut self, path: &str) -> Result<bool> {
        let (parent_ino, name) = self.resolve_path(path)?;
        let parent_inode = self.read_inode(parent_ino)?;

        if self.lookup_in_dir(&parent_inode, name)?.is_some() {
            // File already exists
            return Ok(false);
        }

        // Create the file
        let fd = self.open_file(path)?;
        self.close(fd)?;
        Ok(true)
    }

    /// Renames a file or directory from src to dst.
    pub fn rename(&mut self, src: &str, dst: &str) -> Result<()> {
        match self.rename_inner(src, dst) {
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

    fn rename_inner(&mut self, src: &str, dst: &str) -> Result<()> {
        // Resolve source path
        let (src_parent_ino, src_name) = self.resolve_path(src)?;
        let src_name = src_name.to_string();
        let src_parent_inode = self.read_inode(src_parent_ino)?;

        let src_ino = self
            .lookup_in_dir(&src_parent_inode, &src_name)?
            .ok_or(Error::NotFound)?;

        // Resolve destination path
        let (dst_parent_ino, dst_name) = self.resolve_path(dst)?;
        let dst_name = dst_name.to_string();
        let dst_parent_inode = self.read_inode(dst_parent_ino)?;

        // Check if destination exists
        if let Some(dst_ino) = self.lookup_in_dir(&dst_parent_inode, &dst_name)? {
            // Remove the existing destination
            let dst_inode = self.read_inode(dst_ino)?;
            if dst_inode.is_directory() {
                // Check if empty before removing
                if !self.is_directory_empty(&dst_inode)? {
                    return Err(Error::DirectoryNotEmpty);
                }
                self.remove_dir_entry(dst_parent_ino, &dst_name)?;
                self.free_inode_blocks(dst_ino)?;
            } else {
                // Remove the file
                self.remove_dir_entry(dst_parent_ino, &dst_name)?;
                let is_open = self.open_files.values().any(|f| f.ino == dst_ino);
                if is_open {
                    self.unlinked_inodes.insert(dst_ino);
                } else {
                    self.free_inode_blocks(dst_ino)?;
                }
            }
        }

        // Remove from source directory
        self.remove_dir_entry(src_parent_ino, &src_name)?;

        // Add to destination directory
        self.add_dir_entry(dst_parent_ino, src_ino, &dst_name)?;

        self.persist_inode_map()?;
        Ok(())
    }

    /// Creates a directory and all parent directories as needed.
    pub fn mkdir_all(&mut self, path: &str) -> Result<()> {
        // Split path into components
        let components: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();

        if components.is_empty() {
            return Err(Error::InvalidArgument);
        }

        // Try to create each directory in the path
        let mut current_path = String::new();
        for component in components {
            current_path.push('/');
            current_path.push_str(component);

            // Try to create this directory, ignoring AlreadyExists errors
            match self.mkdir(&current_path) {
                Ok(()) => {}
                Err(Error::AlreadyExists) => {
                    // Check that it's actually a directory
                    if !self.is_dir(&current_path) {
                        return Err(Error::NotADirectory);
                    }
                }
                Err(e) => return Err(e),
            }
        }

        Ok(())
    }

    /// Writes NUL bytes at offset for length bytes.
    /// Extends file if offset+length exceeds current size.
    pub fn punch_hole(&mut self, path: &str, offset: u64, length: u64) -> Result<()> {
        let fd = self.open_file(path)?;
        let current_size = self.file_size(fd)?;

        // Write zeros at the specified range
        self.seek(fd, offset)?;
        let zeros = vec![0u8; length as usize];
        self.write(fd, &zeros)?;

        // If the file was larger, we may have extended it; truncate back if needed
        let new_size = self.file_size(fd)?;
        if new_size > current_size.max(offset + length) {
            self.truncate(fd, current_size.max(offset + length))?;
        }

        self.close(fd)?;
        Ok(())
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

    fn open_file_inner(&mut self, path: &str) -> Result<FileDescriptor> {
        self.open_file_inner_with_hops(path, 0)
    }

    fn open_file_inner_with_hops(&mut self, path: &str, hops: usize) -> Result<FileDescriptor> {
        if hops > Self::MAX_SYMLINK_HOPS {
            return Err(Error::InvalidArgument); // Too many symlink hops (loop)
        }

        let (parent_ino, name) = self.resolve_path(path)?;
        let name = name.to_string(); // Copy to avoid borrow issues
        let parent_inode = self.read_inode(parent_ino)?;

        let ino = match self.lookup_in_dir(&parent_inode, &name)? {
            Some(ino) => {
                let inode = self.read_inode(ino)?;
                if inode.is_directory() {
                    return Err(Error::IsDirectory);
                }
                if inode.is_symlink() {
                    // Follow the symlink
                    let target = self.read_symlink_target(&inode)?;
                    return self.open_file_inner_with_hops(&target, hops + 1);
                }
                ino
            }
            None => self.create_file_in_dir(parent_ino, &name)?,
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

    fn remove_inner(&mut self, path: &str) -> Result<()> {
        let (parent_ino, name) = self.resolve_path(path)?;
        let name = name.to_string(); // Copy to avoid borrow issues
        let parent_inode = self.read_inode(parent_ino)?;

        // Find the file's inode number
        let ino = self
            .lookup_in_dir(&parent_inode, &name)?
            .ok_or(Error::NotFound)?;

        // Check that it's not a directory (use rmdir for directories)
        let mut inode = self.read_inode(ino)?;
        if inode.is_directory() {
            return Err(Error::IsDirectory);
        }

        // Remove the directory entry
        self.remove_dir_entry(parent_ino, &name)?;

        // Decrement link count
        inode.link_count = inode.link_count.saturating_sub(1);

        if inode.link_count == 0 {
            // No more links - check if the file is currently open
            let is_open = self.open_files.values().any(|f| f.ino == ino);

            if is_open {
                // Mark as unlinked - will be fully removed when last FD is closed
                self.unlinked_inodes.insert(ino);
                // Still write the updated inode with link_count=0
                self.write_inode(&inode)?;
            } else {
                // No open FDs, remove immediately
                self.free_inode_blocks(ino)?;
            }
        } else {
            // Still has other links, just update the inode with decremented link_count
            self.write_inode(&inode)?;
        }

        self.persist_inode_map()?;

        Ok(())
    }

    /// Creates a hard link at `dst` pointing to the file at `src`.
    ///
    /// Both paths must be in existing directories. The source must be a regular
    /// file (not a directory). After linking, both paths refer to the same inode.
    pub fn link(&mut self, src: &str, dst: &str) -> Result<()> {
        match self.link_inner(src, dst) {
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

    fn link_inner(&mut self, src: &str, dst: &str) -> Result<()> {
        // Resolve source path to get the inode
        let (src_parent_ino, src_name) = self.resolve_path(src)?;
        let src_name = src_name.to_string();
        let src_parent_inode = self.read_inode(src_parent_ino)?;

        let src_ino = self
            .lookup_in_dir(&src_parent_inode, &src_name)?
            .ok_or(Error::NotFound)?;

        // Check that source is not a directory (hard links to directories not allowed)
        let mut src_inode = self.read_inode(src_ino)?;
        if src_inode.is_directory() {
            return Err(Error::IsDirectory);
        }

        // Resolve destination path
        let (dst_parent_ino, dst_name) = self.resolve_path(dst)?;
        let dst_name = dst_name.to_string();
        let dst_parent_inode = self.read_inode(dst_parent_ino)?;

        // Check that destination doesn't already exist
        if self.lookup_in_dir(&dst_parent_inode, &dst_name)?.is_some() {
            return Err(Error::AlreadyExists);
        }

        // Increment link count
        src_inode.link_count = src_inode.link_count.saturating_add(1);
        self.write_inode(&src_inode)?;

        // Add directory entry for the new link
        self.add_dir_entry(dst_parent_ino, src_ino, &dst_name)?;

        self.persist_inode_map()?;

        Ok(())
    }

    /// Creates a symbolic link at `linkpath` pointing to `target`.
    ///
    /// The `target` is stored as-is and is not validated. The symlink's parent
    /// directory must exist. Unlike hard links, symlinks can point to directories
    /// and to paths that don't exist.
    pub fn symlink(&mut self, target: &str, linkpath: &str) -> Result<()> {
        match self.symlink_inner(target, linkpath) {
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

    fn symlink_inner(&mut self, target: &str, linkpath: &str) -> Result<()> {
        if target.is_empty() {
            return Err(Error::InvalidArgument);
        }

        // Resolve linkpath to get the parent directory
        let (parent_ino, link_name) = self.resolve_path(linkpath)?;
        let link_name = link_name.to_string();
        let parent_inode = self.read_inode(parent_ino)?;

        // Check that destination doesn't already exist
        if self.lookup_in_dir(&parent_inode, &link_name)?.is_some() {
            return Err(Error::AlreadyExists);
        }

        // Create the symlink inode
        let ino = self.superblock.next_inode;
        self.superblock.next_inode = self.superblock.next_inode.next();

        let now = self.now_ms();
        let mut symlink_inode = Inode::new_symlink(ino, now);

        // Write the target string as the symlink's content (stored in data blocks)
        let target_bytes = target.as_bytes();
        symlink_inode.size = target_bytes.len() as u64;

        // Write target to data blocks
        let mut offset = 0;
        while offset < target_bytes.len() {
            let block_idx = BlockIndex::from_byte_offset(offset as u64);
            let block_offset = offset % BLOCK_SIZE;
            let bytes_in_block = (BLOCK_SIZE - block_offset).min(target_bytes.len() - offset);

            let mut block_data = [0u8; BLOCK_SIZE];
            block_data[block_offset..block_offset + bytes_in_block]
                .copy_from_slice(&target_bytes[offset..offset + bytes_in_block]);

            let block_addr = self.allocate_block()?;
            self.write_block(block_addr, &block_data)?;
            self.segment_summary.insert(
                block_addr,
                SegmentSummaryEntry {
                    ino,
                    block_index: block_idx,
                    entry_type: SegmentEntryType::Data,
                },
            );

            self.set_block_addr(&mut symlink_inode, block_idx, block_addr)?;
            offset += bytes_in_block;
        }

        self.write_inode(&symlink_inode)?;

        // Add directory entry for the symlink
        self.add_dir_entry(parent_ino, ino, &link_name)?;

        self.write_superblock()?;
        self.persist_inode_map()?;

        Ok(())
    }

    /// Reads the target of a symbolic link.
    ///
    /// Returns an error if the path is not a symlink.
    pub fn readlink(&self, path: &str) -> Result<String> {
        // Use resolve_path_no_follow to get the symlink inode without following it
        let ino = self.resolve_path_no_follow_final(path)?;
        let inode = self.read_inode(ino)?;

        if !inode.is_symlink() {
            return Err(Error::InvalidArgument);
        }

        self.read_symlink_target(&inode)
    }

    /// Reads the target string from a symlink inode's data blocks.
    fn read_symlink_target(&self, inode: &Inode) -> Result<String> {
        let mut target = vec![0u8; inode.size as usize];
        let mut offset = 0;

        while offset < target.len() {
            let block_idx = BlockIndex::from_byte_offset(offset as u64);
            let block_offset = offset % BLOCK_SIZE;
            let bytes_in_block = (BLOCK_SIZE - block_offset).min(target.len() - offset);

            let block_addr = self.get_block_addr(inode, block_idx)?;
            if !block_addr.is_valid() {
                // Sparse region - fill with zeros
                target[offset..offset + bytes_in_block].fill(0);
            } else {
                let mut block_data = [0u8; BLOCK_SIZE];
                self.read_block(block_addr, &mut block_data)?;
                target[offset..offset + bytes_in_block]
                    .copy_from_slice(&block_data[block_offset..block_offset + bytes_in_block]);
            }

            offset += bytes_in_block;
        }

        String::from_utf8(target).map_err(|_| Error::CorruptFilesystem)
    }

    /// Resolves a path to an inode number without following the final symlink.
    ///
    /// This is used by `readlink` and `lstat` operations.
    fn resolve_path_no_follow_final(&self, path: &str) -> Result<InodeNumber> {
        let (parent_ino, name) = self.resolve_path(path)?;
        let parent_inode = self.read_inode(parent_ino)?;

        self.lookup_in_dir(&parent_inode, name)?
            .ok_or(Error::NotFound)
    }

    /// Creates a directory at the given path.
    ///
    /// All parent directories must already exist.
    pub fn mkdir(&mut self, path: &str) -> Result<()> {
        match self.mkdir_inner(path) {
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

    fn mkdir_inner(&mut self, path: &str) -> Result<()> {
        let (parent_ino, name) = self.resolve_path(path)?;
        let name = name.to_string(); // Copy to avoid borrow issues
        let parent_inode = self.read_inode(parent_ino)?;

        // Check if the name already exists
        if self.lookup_in_dir(&parent_inode, &name)?.is_some() {
            return Err(Error::AlreadyExists);
        }

        self.create_directory_in_dir(parent_ino, &name)?;
        Ok(())
    }

    /// Removes an empty directory at the given path.
    ///
    /// Returns an error if the directory is not empty or is not a directory.
    pub fn rmdir(&mut self, path: &str) -> Result<()> {
        match self.rmdir_inner(path) {
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

    fn rmdir_inner(&mut self, path: &str) -> Result<()> {
        let (parent_ino, name) = self.resolve_path(path)?;
        let name = name.to_string(); // Copy to avoid borrow issues
        let parent_inode = self.read_inode(parent_ino)?;

        // Find the directory's inode number
        let ino = self
            .lookup_in_dir(&parent_inode, &name)?
            .ok_or(Error::NotFound)?;

        // Check that it's a directory
        let inode = self.read_inode(ino)?;
        if !inode.is_directory() {
            return Err(Error::NotADirectory);
        }

        // Check that the directory is empty
        if !self.is_directory_empty(&inode)? {
            return Err(Error::DirectoryNotEmpty);
        }

        // Remove the directory entry from parent
        self.remove_dir_entry(parent_ino, &name)?;

        // Free the directory's inode (no data blocks since it's empty)
        self.free_inode_blocks(ino)?;

        self.persist_inode_map()?;

        Ok(())
    }

    /// Checks if a directory is empty (contains no valid entries).
    fn is_directory_empty(&self, dir_inode: &Inode) -> Result<bool> {
        // dir_inode.size stores the number of entries
        let num_entries = dir_inode.size;
        for i in 0..num_entries {
            let offset = Self::dir_entry_offset(i);
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
            {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// Frees all blocks associated with an inode and removes it from the inode map.
    fn free_inode_blocks(&mut self, ino: InodeNumber) -> Result<()> {
        // Get the inode to find all its blocks
        let inode = self.read_inode(ino)?;

        // Remove data blocks from segment summary
        // For directories, size is entry count; for files/symlinks, size is bytes
        let num_blocks = if inode.is_directory() {
            // Number of blocks needed to hold all directory entries
            if inode.size == 0 {
                0
            } else {
                (inode.size - 1) / Self::DIR_ENTRIES_PER_BLOCK + 1
            }
        } else {
            BlockIndex::blocks_for_size(inode.size)
        };
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
        // dir_inode.size stores the number of entries
        let num_entries = dir_inode.size;

        for i in 0..num_entries {
            let offset = Self::dir_entry_offset(i);
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

    /// Advances `addr` by one block in the circular log.
    fn next_log_block(&self, addr: BlockAddress) -> BlockAddress {
        let next = addr.next();
        if next.as_u64() >= self.superblock.log_end.as_u64() {
            self.superblock.log_start
        } else {
            next
        }
    }

    fn clean_inner(&mut self) -> Result<usize> {
        let mut blocks_cleaned = 0;
        let mut inodes_to_update: BTreeMap<InodeNumber, Inode> = BTreeMap::new();

        // Clean blocks starting from the head, moving towards the tail
        // We stop when head reaches tail (log is empty) or we've cleaned enough
        let log_size = self.superblock.log_end.as_u64() - self.superblock.log_start.as_u64();
        let target_free = log_size / 4; // Try to free at least 25%

        while self.superblock.head != self.superblock.tail {
            let current_free = self.log_distance(self.superblock.tail, self.superblock.head);
            if current_free >= target_free {
                break;
            }

            let head_block = self.superblock.head;

            // Check if this block is live
            if let Some(entry) = self.segment_summary.get(&head_block).copied() {
                // Block is live - need to relocate it
                let ino = entry.ino;

                // Get or load the inode
                let inode = if let Some(inode) = inodes_to_update.get(&ino) {
                    inode.clone()
                } else if self.inode_map.contains_key(&ino) {
                    match self.read_inode(ino) {
                        Ok(inode) => inode,
                        Err(_) => {
                            // Can't read inode, mark block as dead
                            self.segment_summary.remove(&head_block);
                            self.superblock.head = self.next_log_block(head_block);
                            blocks_cleaned += 1;
                            continue;
                        }
                    }
                } else {
                    // Inode no longer exists, block is garbage
                    self.segment_summary.remove(&head_block);
                    self.superblock.head = self.next_log_block(head_block);
                    blocks_cleaned += 1;
                    continue;
                };

                // For data blocks, check if this is still the current block for this file position
                if entry.entry_type == SegmentEntryType::Data {
                    let current_addr = self.get_block_addr(&inode, entry.block_index)?;
                    if current_addr != head_block {
                        // This block is stale, just remove it
                        self.segment_summary.remove(&head_block);
                        self.superblock.head = self.next_log_block(head_block);
                        blocks_cleaned += 1;
                        continue;
                    }
                }

                // Read the block data
                let mut block_data = [0u8; BLOCK_SIZE];
                self.read_block(head_block, &mut block_data)?;

                // Remove from segment summary before allocating (so head advances for free space calc)
                self.segment_summary.remove(&head_block);

                // Advance head first to free up space for allocation
                self.superblock.head = self.next_log_block(head_block);

                // Allocate new block at tail
                let new_addr = self.allocate_block()?;
                self.write_block(new_addr, &block_data)?;

                // Add to segment summary at new location
                self.segment_summary.insert(new_addr, entry);

                // Update the inode if it's a data block
                if entry.entry_type == SegmentEntryType::Data {
                    let mut updated_inode = inodes_to_update.remove(&ino).unwrap_or(inode);
                    self.set_block_addr(&mut updated_inode, entry.block_index, new_addr)?;
                    inodes_to_update.insert(ino, updated_inode);
                } else if entry.entry_type == SegmentEntryType::Inode {
                    // Update inode map to point to new location
                    self.inode_map.insert(ino, new_addr);
                } else if entry.entry_type == SegmentEntryType::Indirect {
                    // For indirect blocks, we need to update the parent inode
                    let mut updated_inode = inodes_to_update.remove(&ino).unwrap_or(inode);
                    // Check if it's the indirect or double indirect block
                    if updated_inode.indirect == head_block {
                        updated_inode.indirect = new_addr;
                    } else if updated_inode.double_indirect == head_block {
                        updated_inode.double_indirect = new_addr;
                    }
                    // Could also be a first-level block within double indirect - skip for now
                    inodes_to_update.insert(ino, updated_inode);
                }

                blocks_cleaned += 1;
            } else {
                // Block is not in segment summary, it's already free
                // Just advance head
                self.superblock.head = self.next_log_block(head_block);
                blocks_cleaned += 1;
            }
        }

        // Write all updated inodes
        for (_, inode) in inodes_to_update {
            self.write_inode(&inode)?;
        }

        self.persist_inode_map()?;

        Ok(blocks_cleaned)
    }

    /// Returns the number of free blocks available.
    pub fn free_blocks(&self) -> u64 {
        let total_log_blocks =
            self.superblock.log_end.as_u64() - self.superblock.log_start.as_u64();
        let used_blocks = self.segment_summary.len() as u64;
        total_log_blocks.saturating_sub(used_blocks)
    }

    /// Returns the total number of blocks in the log region.
    pub fn total_log_blocks(&self) -> u64 {
        self.superblock.log_end.as_u64() - self.superblock.log_start.as_u64()
    }

    /// Returns the current usage percentage of the filesystem (0-100).
    pub fn usage_percent(&self) -> u64 {
        let total = self.total_log_blocks();
        if total == 0 {
            return 100;
        }
        let used = total - self.free_blocks();
        (used * 100) / total
    }

    /// Recovers from a partial write by reloading in-memory state from disk.
    ///
    /// This is called when an operation fails with NoSpace. Since LFS writes
    /// contiguously at the tail, we can simply reset to the committed head/tail
    /// positions and reload all in-memory structures.
    fn recover_from_partial_write(&mut self) -> Result<()> {
        // Reset device sequence tracking since we're about to write to earlier blocks
        self.device.reset_sequence();
        let mut block = [0u8; BLOCK_SIZE];
        self.read_block(BlockAddress::new(0), &mut block)?;
        self.superblock = Superblock::from_bytes(&block).ok_or(Error::CorruptFilesystem)?;
        self.superblock.head = self.committed_head;
        self.superblock.tail = self.committed_tail;
        self.inode_map.clear();
        self.segment_summary.clear();
        self.load_inode_map()?;
        self.rebuild_segment_summary()?;
        Ok(())
    }

    /// Commits the current state by updating committed head/tail to match the superblock.
    fn commit(&mut self) {
        self.committed_head = self.superblock.head;
        self.committed_tail = self.superblock.tail;
    }

    fn write_superblock(&mut self) -> Result<()> {
        let mut block = [0u8; BLOCK_SIZE];
        block[..72].copy_from_slice(&self.superblock.as_bytes());
        self.device.write_block(BlockAddress::new(0), &block)
    }

    fn create_root_directory(&mut self) -> Result<()> {
        let now = self.now_ms();
        let root_inode = Inode::new_directory(InodeNumber::ROOT, now);
        self.write_inode(&root_inode)?;

        // Add "." and ".." entries pointing to root itself (standard Unix behavior)
        self.add_dir_entry(InodeNumber::ROOT, InodeNumber::ROOT, ".")?;
        self.add_dir_entry(InodeNumber::ROOT, InodeNumber::ROOT, "..")?;

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

    /// Computes the distance from `from` to `to` in the circular log.
    fn log_distance(&self, from: BlockAddress, to: BlockAddress) -> u64 {
        let log_start = self.superblock.log_start.as_u64();
        let log_end = self.superblock.log_end.as_u64();

        let from_val = from.as_u64();
        let to_val = to.as_u64();

        if to_val >= from_val {
            to_val - from_val
        } else {
            // Wraparound case
            (log_end - from_val) + (to_val - log_start)
        }
    }

    fn allocate_block(&mut self) -> Result<BlockAddress> {
        let log_size = self.superblock.log_end.as_u64() - self.superblock.log_start.as_u64();
        // Reserve 20% of space for cleaner headroom (NoSpace at 80% usage)
        let reserved_blocks = log_size / 5;

        // Check if tail would catch up to head
        // Distance from tail to head is the free space available
        // Special case: when head == tail, the log is either empty or completely full
        // We use segment_summary to distinguish: empty means free, non-empty means full
        let free_space = if self.superblock.tail == self.superblock.head {
            if self.segment_summary.is_empty() {
                // Log is empty, all space is free
                log_size
            } else {
                // Log is full (tail has wrapped around to meet head)
                0
            }
        } else {
            self.log_distance(self.superblock.tail, self.superblock.head)
        };

        // We need at least reserved_blocks of free space
        if free_space <= reserved_blocks {
            return Err(Error::NoSpace);
        }

        let block = self.superblock.tail;

        // Advance tail
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

    /// Maximum number of symlink hops to follow before returning an error.
    const MAX_SYMLINK_HOPS: usize = 40;

    /// Resolves a path to its parent directory inode and the final component name.
    ///
    /// For example, "/a/b/c" returns (inode of "/a/b", "c").
    /// For "file.txt" or "/file.txt", returns (ROOT inode, "file.txt").
    /// For "/" returns (ROOT inode, ".").
    ///
    /// Symlinks in intermediate path components are followed. The final component
    /// is NOT followed (the caller decides whether to follow it).
    ///
    /// Returns an error if any intermediate directory component doesn't exist,
    /// is not a directory (after following symlinks), or if too many symlinks
    /// are encountered (loop detection).
    fn resolve_path<'a>(&self, path: &'a str) -> Result<(InodeNumber, &'a str)> {
        let (parent_ino, _) = self.resolve_path_impl(path, 0)?;

        // Extract the final component from the original path to return a &str
        let components: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
        if components.is_empty() {
            return Ok((InodeNumber::ROOT, "."));
        }

        Ok((parent_ino, components[components.len() - 1]))
    }

    /// Path resolution with symlink following.
    fn resolve_path_impl(&self, path: &str, hops: usize) -> Result<(InodeNumber, String)> {
        if hops > Self::MAX_SYMLINK_HOPS {
            return Err(Error::InvalidArgument); // Too many symlink hops (loop)
        }

        // Handle empty path
        if path.is_empty() {
            return Err(Error::InvalidArgument);
        }

        // Split path into components
        let components: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();

        // Special case: "/" means root directory - return (ROOT, ".")
        if components.is_empty() {
            return Ok((InodeNumber::ROOT, ".".to_string()));
        }

        let final_name = components[components.len() - 1].to_string();

        if final_name.len() > MAX_FILENAME_LEN {
            return Err(Error::FilenameTooLong);
        }

        if components.len() == 1 {
            return Ok((InodeNumber::ROOT, final_name));
        }

        // Walk through intermediate directories, following symlinks
        let mut current_ino = InodeNumber::ROOT;

        for (i, &component) in components[..components.len() - 1].iter().enumerate() {
            if component.len() > MAX_FILENAME_LEN {
                return Err(Error::FilenameTooLong);
            }

            let current_inode = self.read_inode(current_ino)?;
            if !current_inode.is_directory() {
                return Err(Error::NotADirectory);
            }

            let next_ino = self
                .lookup_in_dir(&current_inode, component)?
                .ok_or(Error::NotFound)?;

            let next_inode = self.read_inode(next_ino)?;
            if next_inode.is_symlink() {
                // Read the symlink target
                let target = self.read_symlink_target(&next_inode)?;

                // Build the remaining path
                let remaining: Vec<&str> = components[i + 1..].to_vec();
                let remaining_path = remaining.join("/");

                // Resolve the symlink target + remaining path
                let full_path = if remaining_path.is_empty() {
                    target.clone()
                } else {
                    format!("{}/{}", target, remaining_path)
                };

                // If target is absolute, start from root; otherwise from current dir
                // For simplicity, we treat all targets as if starting from root for now
                // This is because we don't track parent directories
                let resolved_path = if target.starts_with('/') {
                    full_path
                } else {
                    // Relative path - prepend "/" to resolve from root
                    // This is a simplification; proper implementation needs parent tracking
                    format!("/{}", full_path)
                };

                return self.resolve_path_impl(&resolved_path, hops + 1);
            }

            current_ino = next_ino;
        }

        // Verify the parent is actually a directory
        let parent_inode = self.read_inode(current_ino)?;
        if !parent_inode.is_directory() {
            return Err(Error::NotADirectory);
        }

        Ok((current_ino, final_name))
    }

    fn lookup_in_dir(&self, dir_inode: &Inode, name: &str) -> Result<Option<InodeNumber>> {
        // dir_inode.size stores the number of entries
        let num_entries = dir_inode.size;
        for i in 0..num_entries {
            let offset = Self::dir_entry_offset(i);
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

    fn create_file_in_dir(&mut self, dir_ino: InodeNumber, name: &str) -> Result<InodeNumber> {
        let ino = self.superblock.next_inode;
        self.superblock.next_inode = self.superblock.next_inode.next();

        let now = self.now_ms();
        let file_inode = Inode::new_file(ino, now);
        self.write_inode(&file_inode)?;

        self.add_dir_entry(dir_ino, ino, name)?;

        self.write_superblock()?;
        self.persist_inode_map()?;

        Ok(ino)
    }

    fn create_directory_in_dir(&mut self, dir_ino: InodeNumber, name: &str) -> Result<InodeNumber> {
        let ino = self.superblock.next_inode;
        self.superblock.next_inode = self.superblock.next_inode.next();

        let now = self.now_ms();
        let dir_inode = Inode::new_directory(ino, now);
        self.write_inode(&dir_inode)?;

        self.add_dir_entry(dir_ino, ino, name)?;

        self.write_superblock()?;
        self.persist_inode_map()?;

        Ok(ino)
    }

    /// Number of directory entries that fit in a single block.
    const DIR_ENTRIES_PER_BLOCK: u64 = BLOCK_SIZE as u64 / DIR_ENTRY_SIZE;

    /// Computes the byte offset of the n-th directory entry, accounting for
    /// block-aligned layout (entries don't span blocks).
    fn dir_entry_offset(entry_index: u64) -> u64 {
        let block_num = entry_index / Self::DIR_ENTRIES_PER_BLOCK;
        let entry_in_block = entry_index % Self::DIR_ENTRIES_PER_BLOCK;
        block_num * BLOCK_SIZE as u64 + entry_in_block * DIR_ENTRY_SIZE
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

        // Directory size stores the number of entries (not bytes)
        let entry_index = dir_inode.size;
        let offset = Self::dir_entry_offset(entry_index);

        let block_idx = BlockIndex::from_byte_offset(offset);
        let block_offset = (offset % BLOCK_SIZE as u64) as usize;

        let mut block_data = [0u8; BLOCK_SIZE];
        let old_block_addr = self.get_block_addr(&dir_inode, block_idx)?;

        if old_block_addr.is_valid() {
            self.read_block(old_block_addr, &mut block_data)?;
        }

        block_data[block_offset..block_offset + DIR_ENTRY_SIZE as usize]
            .copy_from_slice(&entry_bytes);

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
        // Size stores the number of entries
        dir_inode.size = entry_index + 1;
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
                // For directories, size is entry count; for files/symlinks, size is bytes
                let num_blocks = if inode.is_directory() {
                    if inode.size == 0 {
                        0
                    } else {
                        (inode.size - 1) / Self::DIR_ENTRIES_PER_BLOCK + 1
                    }
                } else {
                    BlockIndex::blocks_for_size(inode.size)
                };

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

impl<T: Fn() -> i64> Lfs<MemoryBlockDevice, T> {
    /// Creates a new LFS on the given buffer.
    ///
    /// This is a convenience constructor for using a `Vec<u8>` as the backing store.
    /// The buffer must be at least 16 blocks (64KB) in size.
    ///
    /// # Arguments
    /// * `data` - The backing buffer for the filesystem.
    /// * `dev` - Device ID for this filesystem instance.
    /// * `time_source` - Function returning current time in milliseconds since UNIX epoch.
    pub fn from_vec(data: Vec<u8>, dev: DeviceId, time_source: T) -> Result<Self> {
        let total_blocks = (data.len() / BLOCK_SIZE) as u64;
        let device = MemoryBlockDevice::new(data);
        Self::new(device, total_blocks, dev, time_source)
    }

    /// Opens an existing LFS from the given buffer.
    ///
    /// This is a convenience constructor for using a `Vec<u8>` as the backing store.
    /// The tail position is read from the superblock on disk.
    ///
    /// # Arguments
    /// * `data` - The backing buffer containing an existing filesystem.
    /// * `dev` - Device ID for this filesystem instance.
    /// * `time_source` - Function returning current time in milliseconds since UNIX epoch.
    pub fn open_vec(data: Vec<u8>, dev: DeviceId, time_source: T) -> Result<Self> {
        let total_blocks = (data.len() / BLOCK_SIZE) as u64;
        let device = MemoryBlockDevice::new(data);
        Self::open(device, total_blocks, dev, time_source)
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

    fn zero_time() -> i64 {
        0
    }

    fn create_test_fs(blocks: usize) -> Lfs<MemoryBlockDevice, fn() -> i64> {
        let data = vec![0u8; blocks * BLOCK_SIZE];
        Lfs::from_vec(data, DeviceId::new(1), zero_time as fn() -> i64)
            .expect("Failed to create filesystem")
    }

    fn open_test_fs(data: Vec<u8>) -> Lfs<MemoryBlockDevice, fn() -> i64> {
        Lfs::open_vec(data, DeviceId::new(1), zero_time as fn() -> i64)
            .expect("Failed to open filesystem")
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
        let mut lfs =
            Lfs::from_vec(data, DeviceId::new(1), zero_time).expect("Failed to create filesystem");

        let fd = lfs.open_file("persist.txt").expect("Failed to open file");
        lfs.write(fd, b"Saved data").expect("Failed to write");
        lfs.close(fd).expect("Failed to close");

        println!("Tail position: {}", lfs.tail().as_u64());

        let data = lfs.into_inner();

        let mut lfs2 = open_test_fs(data);
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
        let result = Lfs::from_vec(data, DeviceId::new(1), zero_time);
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
        let mut lfs =
            Lfs::from_vec(data, DeviceId::new(1), zero_time).expect("Failed to create filesystem");

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

        let mut lfs2 = open_test_fs(data);

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
        let lfs = Lfs::from_vec(data, DeviceId::new(1), zero_time)
            .expect("Minimum size filesystem should work");
        assert!(lfs.free_blocks() > 0);
        println!("Minimum filesystem has {} free blocks", lfs.free_blocks());
    }

    #[test]
    fn filesystem_15_blocks_fails() {
        let data = vec![0u8; 15 * BLOCK_SIZE];
        let result = Lfs::from_vec(data, DeviceId::new(1), zero_time);
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

        let result = Lfs::open_vec(data, DeviceId::new(1), zero_time);
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
        // Use a larger filesystem to allow multiple fill/recover cycles
        let mut lfs = create_test_fs(256);

        for iteration in 0..3 {
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
                "Iteration {}: wrote {} blocks before NoSpace/FileTooLarge",
                iteration, write_count
            );

            let verify_fd = lfs
                .open_file("persistent.txt")
                .expect("Failed to reopen persistent file");
            let mut buf = vec![0u8; content.len()];
            lfs.read(verify_fd, &mut buf).expect("Failed to read");
            assert_eq!(String::from_utf8_lossy(&buf), content);
            println!("Iteration {}: content verified after NoSpace", iteration);
            lfs.close(verify_fd).expect("Failed to close");

            // Run cleaner to advance head and reclaim space for next iteration
            lfs.clean().expect("Failed to clean");
        }
    }

    #[test]
    fn debug_inode_map_persist_restore() {
        // Create filesystem and write a file
        let data = vec![0u8; 64 * BLOCK_SIZE];
        let mut lfs =
            Lfs::from_vec(data, DeviceId::new(1), zero_time).expect("Failed to create filesystem");

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
        let mut lfs2 = open_test_fs(data);
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
        let mut lfs =
            Lfs::from_vec(data, DeviceId::new(1), zero_time).expect("Failed to create filesystem");

        // Write to file a.txt
        let fd = lfs.open_file("a.txt").expect("open a.txt");
        lfs.write(fd, b"Content A").expect("write");
        lfs.close(fd).expect("close");

        let data = lfs.into_inner();

        // Restore
        let mut lfs2 = open_test_fs(data);

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
        let mut lfs = Lfs::from_vec(data, DeviceId::new(1), zero_time).expect("create fs");

        let fd = lfs.open_file("large.txt").expect("open");
        let write_data: Vec<u8> = (0..10000).map(|i| (i % 256) as u8).collect();
        lfs.write(fd, &write_data).expect("write");
        lfs.close(fd).expect("close");

        println!("Before restore: inode_map = {:?}", lfs.inode_map);

        let data = lfs.into_inner();

        let mut lfs2 = open_test_fs(data);
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
        let mut lfs = Lfs::from_vec(data, DeviceId::new(1), zero_time).expect("create fs");

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

        let mut lfs2 = open_test_fs(data);
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
        let mut lfs = Lfs::from_vec(data, DeviceId::new(1), zero_time).expect("create fs");

        // Open { name: "a.txt" }
        let fd0 = lfs.open_file("a.txt").expect("open 1");
        // Open { name: "a.txt" }
        let _fd1 = lfs.open_file("a.txt").expect("open 2");
        // Write { fd_index: 0, data: [...] }
        lfs.write(fd0, &write_data).expect("write");

        let data = lfs.into_inner();

        let mut lfs_verify = open_test_fs(data);

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
        let mut lfs = Lfs::from_vec(data, DeviceId::new(1), zero_time).expect("create fs");

        let fd0 = lfs.open_file("a.txt").expect("open 1");
        let _fd1 = lfs.open_file("a.txt").expect("open 2");
        lfs.write(fd0, &write_data).expect("write");

        let data = lfs.into_inner();

        let mut lfs_verify = open_test_fs(data);

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
        let mut lfs = Lfs::from_vec(data, DeviceId::new(1), zero_time).expect("create fs");

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
        let mut lfs_verify = open_test_fs(data);

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
        let mut lfs = Lfs::from_vec(data, DeviceId::new(1), zero_time).expect("create fs");

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
        let mut lfs2 = open_test_fs(data);

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
        let mut lfs = Lfs::from_vec(data, DeviceId::new(1), zero_time).expect("create fs");

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

        let mut lfs = Lfs::new(device, 64, DeviceId::new(1), zero_time as fn() -> i64)
            .expect("create fs with sequential device");

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

        let mut lfs = Lfs::new(device, 128, DeviceId::new(1), zero_time as fn() -> i64)
            .expect("create fs with sequential device");

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

        let mut lfs = Lfs::new(device, 256, DeviceId::new(1), zero_time as fn() -> i64)
            .expect("create fs with sequential device");

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

        let mut lfs = Lfs::new(device, 64, DeviceId::new(1), zero_time as fn() -> i64)
            .expect("create fs with sequential device");

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
        let mut lfs = Lfs::from_vec(data, DeviceId::new(1), zero_time).expect("create fs");

        let fd = lfs.open_file("persist.txt").expect("create file");
        lfs.write(fd, b"Will be removed").expect("write");
        lfs.close(fd).expect("close");

        lfs.remove("persist.txt").expect("remove");

        let data = lfs.into_inner();
        let mut lfs2 = open_test_fs(data);

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

    #[test]
    fn hard_link_basic() {
        let mut lfs = create_test_fs(64);

        // Create original file
        let fd = lfs.open_file("original.txt").expect("create file");
        lfs.write(fd, b"Shared content").expect("write");
        lfs.close(fd).expect("close");

        // Create hard link
        lfs.link("original.txt", "linked.txt")
            .expect("create hard link");

        // Read from original
        let fd = lfs.open_file("original.txt").expect("open original");
        let mut buf = vec![0u8; 14];
        lfs.read(fd, &mut buf).expect("read");
        assert_eq!(&buf, b"Shared content");
        lfs.close(fd).expect("close");

        // Read from link - should have same content
        let fd = lfs.open_file("linked.txt").expect("open link");
        let mut buf = vec![0u8; 14];
        lfs.read(fd, &mut buf).expect("read");
        assert_eq!(&buf, b"Shared content");
        lfs.close(fd).expect("close");

        println!("Hard link basic test passed");
    }

    #[test]
    fn hard_link_shared_writes() {
        let mut lfs = create_test_fs(64);

        // Create original file
        let fd = lfs.open_file("original.txt").expect("create file");
        lfs.write(fd, b"Initial").expect("write");
        lfs.close(fd).expect("close");

        // Create hard link
        lfs.link("original.txt", "linked.txt")
            .expect("create hard link");

        // Write via link
        let fd = lfs.open_file("linked.txt").expect("open link");
        lfs.seek(fd, 0).expect("seek");
        lfs.write(fd, b"Updated").expect("write via link");
        lfs.close(fd).expect("close");

        // Read from original - should see the update
        let fd = lfs.open_file("original.txt").expect("open original");
        let mut buf = vec![0u8; 7];
        lfs.read(fd, &mut buf).expect("read");
        assert_eq!(&buf, b"Updated");
        lfs.close(fd).expect("close");

        println!("Hard link shared writes test passed");
    }

    #[test]
    fn hard_link_remove_one() {
        let mut lfs = create_test_fs(64);

        // Create original file
        let fd = lfs.open_file("original.txt").expect("create file");
        lfs.write(fd, b"Persistent data").expect("write");
        lfs.close(fd).expect("close");

        // Create hard link
        lfs.link("original.txt", "linked.txt")
            .expect("create hard link");

        // Remove original
        lfs.remove("original.txt").expect("remove original");

        // Link should still work
        let fd = lfs.open_file("linked.txt").expect("open link");
        let mut buf = vec![0u8; 15];
        lfs.read(fd, &mut buf).expect("read");
        assert_eq!(&buf, b"Persistent data");
        lfs.close(fd).expect("close");

        // Original is gone - opening it creates new empty file
        let fd = lfs.open_file("original.txt").expect("open creates new");
        let mut buf = vec![0u8; 10];
        let n = lfs.read(fd, &mut buf).expect("read");
        assert_eq!(n, 0, "New file should be empty");
        lfs.close(fd).expect("close");

        println!("Hard link remove one test passed");
    }

    #[test]
    fn hard_link_remove_both() {
        let mut lfs = create_test_fs(64);

        // Create original file
        let fd = lfs.open_file("original.txt").expect("create file");
        lfs.write(fd, b"Will be gone").expect("write");
        lfs.close(fd).expect("close");

        // Create hard link
        lfs.link("original.txt", "linked.txt")
            .expect("create hard link");

        // Remove both links
        lfs.remove("original.txt").expect("remove original");
        lfs.remove("linked.txt").expect("remove link");

        // Both are gone - opening creates new empty files
        let fd = lfs.open_file("original.txt").expect("open original");
        let n = lfs.read(fd, &mut [0u8; 10]).expect("read");
        assert_eq!(n, 0);
        lfs.close(fd).expect("close");

        let fd = lfs.open_file("linked.txt").expect("open link");
        let n = lfs.read(fd, &mut [0u8; 10]).expect("read");
        assert_eq!(n, 0);
        lfs.close(fd).expect("close");

        println!("Hard link remove both test passed");
    }

    #[test]
    fn hard_link_to_directory_fails() {
        let mut lfs = create_test_fs(64);

        lfs.mkdir("mydir").expect("create directory");

        let result = lfs.link("mydir", "mydir_link");
        assert_eq!(result, Err(Error::IsDirectory));

        println!("Hard link to directory correctly rejected");
    }

    #[test]
    fn hard_link_dst_exists_fails() {
        let mut lfs = create_test_fs(64);

        let fd = lfs.open_file("src.txt").expect("create src");
        lfs.write(fd, b"source").expect("write");
        lfs.close(fd).expect("close");

        let fd = lfs.open_file("dst.txt").expect("create dst");
        lfs.write(fd, b"dest").expect("write");
        lfs.close(fd).expect("close");

        let result = lfs.link("src.txt", "dst.txt");
        assert_eq!(result, Err(Error::AlreadyExists));

        println!("Hard link to existing destination correctly rejected");
    }

    #[test]
    fn hard_link_src_not_found_fails() {
        let mut lfs = create_test_fs(64);

        let result = lfs.link("nonexistent.txt", "link.txt");
        assert_eq!(result, Err(Error::NotFound));

        println!("Hard link to nonexistent source correctly rejected");
    }

    #[test]
    fn hard_link_multiple_links() {
        let mut lfs = create_test_fs(64);

        // Create original file
        let fd = lfs.open_file("original.txt").expect("create file");
        lfs.write(fd, b"Many links").expect("write");
        lfs.close(fd).expect("close");

        // Create multiple hard links
        lfs.link("original.txt", "link1.txt").expect("create link1");
        lfs.link("original.txt", "link2.txt").expect("create link2");
        lfs.link("link1.txt", "link3.txt")
            .expect("create link3 from link1");

        // Remove original and link1
        lfs.remove("original.txt").expect("remove original");
        lfs.remove("link1.txt").expect("remove link1");

        // link2 and link3 should still work
        let fd = lfs.open_file("link2.txt").expect("open link2");
        let mut buf = vec![0u8; 10];
        lfs.read(fd, &mut buf).expect("read");
        assert_eq!(&buf, b"Many links");
        lfs.close(fd).expect("close");

        let fd = lfs.open_file("link3.txt").expect("open link3");
        let mut buf = vec![0u8; 10];
        lfs.read(fd, &mut buf).expect("read");
        assert_eq!(&buf, b"Many links");
        lfs.close(fd).expect("close");

        println!("Multiple hard links test passed");
    }

    #[test]
    fn symlink_basic() {
        let mut lfs = create_test_fs(64);

        // Create a file
        let fd = lfs.open_file("target.txt").expect("create file");
        lfs.write(fd, b"Target content").expect("write");
        lfs.close(fd).expect("close");

        // Create a symlink to it
        lfs.symlink("target.txt", "link.txt")
            .expect("create symlink");

        // Read the symlink target
        let target = lfs.readlink("link.txt").expect("readlink");
        assert_eq!(target, "target.txt");

        println!("Symlink basic test passed");
    }

    #[test]
    fn symlink_follow_in_path() {
        let mut lfs = create_test_fs(64);

        // Create a directory
        lfs.mkdir("realdir").expect("create directory");

        // Create a file in it
        let fd = lfs.open_file("realdir/file.txt").expect("create file");
        lfs.write(fd, b"File in realdir").expect("write");
        lfs.close(fd).expect("close");

        // Create a symlink to the directory
        lfs.symlink("realdir", "linkdir")
            .expect("create symlink to dir");

        // Access file through the symlink
        let fd = lfs.open_file("linkdir/file.txt").expect("open via symlink");
        let mut buf = vec![0u8; 15];
        lfs.read(fd, &mut buf).expect("read");
        assert_eq!(&buf, b"File in realdir");
        lfs.close(fd).expect("close");

        println!("Symlink follow in path test passed");
    }

    #[test]
    fn symlink_to_nonexistent() {
        let mut lfs = create_test_fs(64);

        // Create a symlink to a nonexistent target (this is allowed)
        lfs.symlink("nonexistent.txt", "dangling.txt")
            .expect("create dangling symlink");

        // Read the symlink target
        let target = lfs.readlink("dangling.txt").expect("readlink");
        assert_eq!(target, "nonexistent.txt");

        // Trying to open the file through the symlink should fail
        // (once we implement symlink following in open_file)

        println!("Symlink to nonexistent test passed");
    }

    #[test]
    fn symlink_absolute_target() {
        let mut lfs = create_test_fs(64);

        // Create a file
        let fd = lfs.open_file("absolute_target.txt").expect("create file");
        lfs.write(fd, b"Absolute target").expect("write");
        lfs.close(fd).expect("close");

        // Create a symlink with absolute path
        lfs.symlink("/absolute_target.txt", "abs_link.txt")
            .expect("create symlink with absolute target");

        // Read the symlink target
        let target = lfs.readlink("abs_link.txt").expect("readlink");
        assert_eq!(target, "/absolute_target.txt");

        println!("Symlink absolute target test passed");
    }

    #[test]
    fn symlink_already_exists_fails() {
        let mut lfs = create_test_fs(64);

        // Create a file
        let fd = lfs.open_file("existing.txt").expect("create file");
        lfs.close(fd).expect("close");

        // Try to create symlink where file exists
        let result = lfs.symlink("target", "existing.txt");
        assert_eq!(result, Err(Error::AlreadyExists));

        println!("Symlink already exists test passed");
    }

    #[test]
    fn symlink_empty_target_fails() {
        let mut lfs = create_test_fs(64);

        let result = lfs.symlink("", "link.txt");
        assert_eq!(result, Err(Error::InvalidArgument));

        println!("Symlink empty target test passed");
    }

    #[test]
    fn readlink_not_symlink_fails() {
        let mut lfs = create_test_fs(64);

        // Create a regular file
        let fd = lfs.open_file("regular.txt").expect("create file");
        lfs.close(fd).expect("close");

        // Try to readlink on a regular file
        let result = lfs.readlink("regular.txt");
        assert_eq!(result, Err(Error::InvalidArgument));

        // Create a directory
        lfs.mkdir("mydir").expect("create dir");

        // Try to readlink on a directory
        let result = lfs.readlink("mydir");
        assert_eq!(result, Err(Error::InvalidArgument));

        println!("Readlink not symlink test passed");
    }

    #[test]
    fn symlink_chain() {
        let mut lfs = create_test_fs(64);

        // Create a file
        let fd = lfs.open_file("target.txt").expect("create file");
        lfs.write(fd, b"Chain end").expect("write");
        lfs.close(fd).expect("close");

        // Create a chain of symlinks
        lfs.symlink("target.txt", "link1.txt")
            .expect("create link1");
        lfs.symlink("link1.txt", "link2.txt").expect("create link2");
        lfs.symlink("link2.txt", "link3.txt").expect("create link3");

        // Verify each readlink returns the immediate target
        assert_eq!(lfs.readlink("link1.txt").unwrap(), "target.txt");
        assert_eq!(lfs.readlink("link2.txt").unwrap(), "link1.txt");
        assert_eq!(lfs.readlink("link3.txt").unwrap(), "link2.txt");

        println!("Symlink chain test passed");
    }

    #[test]
    fn open_file_follows_symlink() {
        let mut lfs = create_test_fs(64);

        // Create a target file with content
        let fd = lfs.open_file("target.txt").expect("create target");
        lfs.write(fd, b"target content").expect("write target");
        lfs.close(fd).expect("close target");

        // Create a symlink pointing to the target
        lfs.symlink("target.txt", "link.txt")
            .expect("create symlink");

        // Open the symlink - should follow it and open the target file
        let fd = lfs.open_file("link.txt").expect("open via symlink");
        let mut buf = vec![0u8; 14];
        lfs.read(fd, &mut buf).expect("read via symlink");
        lfs.close(fd).expect("close");

        // Should get the target file's content, not the symlink target path
        assert_eq!(
            &buf, b"target content",
            "open_file should follow symlink to target file"
        );
        println!("open_file follows symlink test passed");
    }

    #[test]
    fn create_directory() {
        let mut lfs = create_test_fs(64);

        lfs.mkdir("subdir").expect("create directory");
        println!("Created directory 'subdir'");

        // Creating the same directory again should fail
        let result = lfs.mkdir("subdir");
        assert_eq!(result, Err(Error::AlreadyExists));
        println!("Creating duplicate directory correctly returns AlreadyExists");
    }

    #[test]
    fn create_file_in_subdirectory() {
        let mut lfs = create_test_fs(64);

        lfs.mkdir("subdir").expect("create directory");

        let fd = lfs
            .open_file("subdir/test.txt")
            .expect("create file in subdir");
        lfs.write(fd, b"Hello from subdir!").expect("write");
        lfs.close(fd).expect("close");
        println!("Created file in subdirectory");

        // Read it back
        let fd = lfs.open_file("subdir/test.txt").expect("reopen file");
        let mut buf = vec![0u8; 18];
        let n = lfs.read(fd, &mut buf).expect("read");
        assert_eq!(n, 18);
        assert_eq!(&buf, b"Hello from subdir!");
        lfs.close(fd).expect("close");
        println!("Read file from subdirectory successfully");
    }

    #[test]
    fn nested_directories() {
        let mut lfs = create_test_fs(64);

        lfs.mkdir("a").expect("create a");
        lfs.mkdir("a/b").expect("create a/b");
        lfs.mkdir("a/b/c").expect("create a/b/c");
        println!("Created nested directories a/b/c");

        let fd = lfs.open_file("a/b/c/deep.txt").expect("create deep file");
        lfs.write(fd, b"Deep file content").expect("write");
        lfs.close(fd).expect("close");

        let fd = lfs.open_file("a/b/c/deep.txt").expect("reopen");
        let mut buf = vec![0u8; 17];
        lfs.read(fd, &mut buf).expect("read");
        assert_eq!(&buf, b"Deep file content");
        lfs.close(fd).expect("close");
        println!("Successfully wrote and read file in nested directory");
    }

    #[test]
    fn rmdir_empty_directory() {
        let mut lfs = create_test_fs(64);

        lfs.mkdir("empty").expect("create directory");
        lfs.rmdir("empty").expect("remove empty directory");
        println!("Removed empty directory");

        // Should be able to recreate it
        lfs.mkdir("empty").expect("recreate directory");
        println!("Recreated directory after removal");
    }

    #[test]
    fn rmdir_nonempty_directory_fails() {
        let mut lfs = create_test_fs(64);

        lfs.mkdir("nonempty").expect("create directory");
        let fd = lfs.open_file("nonempty/file.txt").expect("create file");
        lfs.write(fd, b"content").expect("write");
        lfs.close(fd).expect("close");

        let result = lfs.rmdir("nonempty");
        assert_eq!(result, Err(Error::DirectoryNotEmpty));
        println!("rmdir on non-empty directory correctly fails");
    }

    #[test]
    fn remove_file_in_subdirectory() {
        let mut lfs = create_test_fs(64);

        lfs.mkdir("dir").expect("create directory");
        let fd = lfs.open_file("dir/file.txt").expect("create file");
        lfs.write(fd, b"content").expect("write");
        lfs.close(fd).expect("close");

        lfs.remove("dir/file.txt").expect("remove file");
        println!("Removed file from subdirectory");

        // Now rmdir should work
        lfs.rmdir("dir").expect("remove now-empty directory");
        println!("Removed directory after removing its file");
    }

    #[test]
    fn open_directory_as_file_fails() {
        let mut lfs = create_test_fs(64);

        lfs.mkdir("mydir").expect("create directory");

        let result = lfs.open_file("mydir");
        assert_eq!(result, Err(Error::IsDirectory));
        println!("Opening directory as file correctly fails");
    }

    #[test]
    fn remove_directory_with_remove_fails() {
        let mut lfs = create_test_fs(64);

        lfs.mkdir("mydir").expect("create directory");

        let result = lfs.remove("mydir");
        assert_eq!(result, Err(Error::IsDirectory));
        println!("remove() on directory correctly returns IsDirectory");
    }

    #[test]
    fn rmdir_on_file_fails() {
        let mut lfs = create_test_fs(64);

        let fd = lfs.open_file("file.txt").expect("create file");
        lfs.close(fd).expect("close");

        let result = lfs.rmdir("file.txt");
        assert_eq!(result, Err(Error::NotADirectory));
        println!("rmdir() on file correctly returns NotADirectory");
    }

    #[test]
    fn path_with_nonexistent_parent_fails() {
        let mut lfs = create_test_fs(64);

        let result = lfs.open_file("nonexistent/file.txt");
        assert_eq!(result, Err(Error::NotFound));
        println!("Creating file in nonexistent directory fails with NotFound");
    }

    #[test]
    fn path_traversal_with_file_as_directory_fails() {
        let mut lfs = create_test_fs(64);

        let fd = lfs.open_file("file.txt").expect("create file");
        lfs.close(fd).expect("close");

        let result = lfs.open_file("file.txt/subfile.txt");
        assert_eq!(result, Err(Error::NotADirectory));
        println!("Using file as directory in path correctly fails");
    }

    #[test]
    fn subdirectory_persists_across_restore() {
        let data = vec![0u8; 64 * BLOCK_SIZE];
        let mut lfs = Lfs::from_vec(data, DeviceId::new(1), zero_time).expect("create fs");

        lfs.mkdir("persistent").expect("create directory");
        let fd = lfs.open_file("persistent/data.txt").expect("create file");
        lfs.write(fd, b"Persisted content").expect("write");
        lfs.close(fd).expect("close");

        let data = lfs.into_inner();
        let mut lfs2 = open_test_fs(data);

        let fd = lfs2
            .open_file("persistent/data.txt")
            .expect("open file after restore");
        let mut buf = vec![0u8; 17];
        lfs2.read(fd, &mut buf).expect("read");
        assert_eq!(&buf, b"Persisted content");
        lfs2.close(fd).expect("close");
        println!("Subdirectory and contents persist across restore");
    }

    #[test]
    fn multiple_subdirectories() {
        let mut lfs = create_test_fs(128);

        lfs.mkdir("dir1").expect("create dir1");
        lfs.mkdir("dir2").expect("create dir2");
        lfs.mkdir("dir1/sub1").expect("create dir1/sub1");

        let fd = lfs.open_file("dir1/file.txt").expect("create file in dir1");
        lfs.write(fd, b"Dir1 file").expect("write");
        lfs.close(fd).expect("close");

        let fd = lfs.open_file("dir2/file.txt").expect("create file in dir2");
        lfs.write(fd, b"Dir2 file").expect("write");
        lfs.close(fd).expect("close");

        let fd = lfs
            .open_file("dir1/sub1/file.txt")
            .expect("create file in dir1/sub1");
        lfs.write(fd, b"Nested file").expect("write");
        lfs.close(fd).expect("close");

        // Verify all files
        let fd = lfs.open_file("dir1/file.txt").expect("open");
        let mut buf = vec![0u8; 9];
        lfs.read(fd, &mut buf).expect("read");
        assert_eq!(&buf, b"Dir1 file");
        lfs.close(fd).expect("close");

        let fd = lfs.open_file("dir2/file.txt").expect("open");
        let mut buf = vec![0u8; 9];
        lfs.read(fd, &mut buf).expect("read");
        assert_eq!(&buf, b"Dir2 file");
        lfs.close(fd).expect("close");

        let fd = lfs.open_file("dir1/sub1/file.txt").expect("open");
        let mut buf = vec![0u8; 11];
        lfs.read(fd, &mut buf).expect("read");
        assert_eq!(&buf, b"Nested file");
        lfs.close(fd).expect("close");

        println!("Multiple subdirectories work correctly");
    }

    #[test]
    fn leading_slash_in_path() {
        let mut lfs = create_test_fs(64);

        // Paths with leading slash should work the same as without
        lfs.mkdir("/subdir").expect("create /subdir");

        let fd = lfs.open_file("/subdir/file.txt").expect("create file");
        lfs.write(fd, b"test").expect("write");
        lfs.close(fd).expect("close");

        // Access without leading slash should work too
        let fd = lfs
            .open_file("subdir/file.txt")
            .expect("open without leading slash");
        let mut buf = vec![0u8; 4];
        lfs.read(fd, &mut buf).expect("read");
        assert_eq!(&buf, b"test");
        lfs.close(fd).expect("close");
        println!("Leading slash in path handled correctly");
    }

    #[test]
    fn empty_path_fails() {
        let mut lfs = create_test_fs(64);

        let result = lfs.open_file("");
        assert_eq!(result, Err(Error::InvalidArgument));

        let result = lfs.mkdir("");
        assert_eq!(result, Err(Error::InvalidArgument));

        let result = lfs.remove("");
        assert_eq!(result, Err(Error::InvalidArgument));

        let result = lfs.rmdir("");
        assert_eq!(result, Err(Error::InvalidArgument));

        println!("Empty path correctly rejected");
    }

    #[test]
    fn stat_root_directory() {
        let lfs = create_test_fs(64);

        // stat("/") should succeed and return info about the root directory
        let stat = lfs.stat("/").expect("stat root directory");
        assert_eq!(stat.file_type, FileType::Directory);
        assert_eq!(stat.ino, InodeNumber::ROOT.as_u64());
        println!(
            "stat('/') succeeded: type={:?}, ino={}",
            stat.file_type, stat.ino
        );
    }

    #[test]
    fn read_dir_root() {
        let mut lfs = create_test_fs(64);

        // Create some files in root
        let fd = lfs.open_file("file1.txt").expect("create file1");
        lfs.close(fd).expect("close");
        let fd = lfs.open_file("file2.txt").expect("create file2");
        lfs.close(fd).expect("close");

        // read_dir("/") should list the contents
        let entries = lfs.read_dir("/").expect("read_dir root");
        let names: Vec<&str> = entries.iter().map(|(name, _)| name.as_str()).collect();
        println!("Root directory contents: {:?}", names);

        // Should contain ".", "..", "file1.txt", "file2.txt"
        assert!(names.contains(&"."), "root should contain '.'");
        assert!(names.contains(&".."), "root should contain '..'");
        assert!(
            names.contains(&"file1.txt"),
            "root should contain 'file1.txt'"
        );
        assert!(
            names.contains(&"file2.txt"),
            "root should contain 'file2.txt'"
        );
    }

    #[test]
    fn error_to_io_error_not_found() {
        use std::io::ErrorKind;
        let err = Error::NotFound;
        let io_err = err.to_io_error();
        assert_eq!(io_err.kind(), ErrorKind::NotFound);
        println!("NotFound maps to ErrorKind::NotFound");
    }

    #[test]
    fn error_to_io_error_already_exists() {
        use std::io::ErrorKind;
        let err = Error::AlreadyExists;
        let io_err = err.to_io_error();
        assert_eq!(io_err.kind(), ErrorKind::AlreadyExists);
        println!("AlreadyExists maps to ErrorKind::AlreadyExists");
    }

    #[test]
    fn error_to_io_error_is_directory() {
        use std::io::ErrorKind;
        let err = Error::IsDirectory;
        let io_err = err.to_io_error();
        assert_eq!(io_err.kind(), ErrorKind::IsADirectory);
        println!("IsDirectory maps to ErrorKind::IsADirectory");
    }

    #[test]
    fn error_to_io_error_not_a_directory() {
        use std::io::ErrorKind;
        let err = Error::NotADirectory;
        let io_err = err.to_io_error();
        assert_eq!(io_err.kind(), ErrorKind::NotADirectory);
        println!("NotADirectory maps to ErrorKind::NotADirectory");
    }

    #[test]
    fn error_to_io_error_directory_not_empty() {
        use std::io::ErrorKind;
        let err = Error::DirectoryNotEmpty;
        let io_err = err.to_io_error();
        assert_eq!(io_err.kind(), ErrorKind::DirectoryNotEmpty);
        println!("DirectoryNotEmpty maps to ErrorKind::DirectoryNotEmpty");
    }

    #[test]
    fn error_to_io_error_no_space() {
        use std::io::ErrorKind;
        let err = Error::NoSpace;
        let io_err = err.to_io_error();
        assert_eq!(io_err.kind(), ErrorKind::StorageFull);
        println!("NoSpace maps to ErrorKind::StorageFull");
    }

    #[test]
    fn error_to_io_error_file_too_large() {
        use std::io::ErrorKind;
        let err = Error::FileTooLarge;
        let io_err = err.to_io_error();
        assert_eq!(io_err.kind(), ErrorKind::FileTooLarge);
        println!("FileTooLarge maps to ErrorKind::FileTooLarge");
    }

    #[test]
    fn error_to_io_error_corrupt_filesystem() {
        use std::io::ErrorKind;
        let err = Error::CorruptFilesystem;
        let io_err = err.to_io_error();
        assert_eq!(io_err.kind(), ErrorKind::InvalidData);
        println!("CorruptFilesystem maps to ErrorKind::InvalidData");
    }

    #[test]
    fn error_to_io_error_invalid_input_variants() {
        use std::io::ErrorKind;

        // InvalidFd
        let err = Error::InvalidFd;
        let io_err = err.to_io_error();
        assert_eq!(io_err.kind(), ErrorKind::InvalidInput);
        println!("InvalidFd maps to ErrorKind::InvalidInput");

        // FilenameTooLong
        let err = Error::FilenameTooLong;
        let io_err = err.to_io_error();
        assert_eq!(io_err.kind(), ErrorKind::InvalidInput);
        println!("FilenameTooLong maps to ErrorKind::InvalidInput");

        // InvalidOffset
        let err = Error::InvalidOffset;
        let io_err = err.to_io_error();
        assert_eq!(io_err.kind(), ErrorKind::InvalidInput);
        println!("InvalidOffset maps to ErrorKind::InvalidInput");

        // InvalidArgument
        let err = Error::InvalidArgument;
        let io_err = err.to_io_error();
        assert_eq!(io_err.kind(), ErrorKind::InvalidInput);
        println!("InvalidArgument maps to ErrorKind::InvalidInput");

        // NotOpen
        let err = Error::NotOpen;
        let io_err = err.to_io_error();
        assert_eq!(io_err.kind(), ErrorKind::InvalidInput);
        println!("NotOpen maps to ErrorKind::InvalidInput");

        // BufferTooSmall
        let err = Error::BufferTooSmall;
        let io_err = err.to_io_error();
        assert_eq!(io_err.kind(), ErrorKind::InvalidInput);
        println!("BufferTooSmall maps to ErrorKind::InvalidInput");
    }

    #[test]
    fn error_from_trait_conversion() {
        use std::io::ErrorKind;

        // Test the From trait implementation
        let err: std::io::Error = Error::NotFound.into();
        assert_eq!(err.kind(), ErrorKind::NotFound);
        println!("From<Error> for std::io::Error works correctly");

        // Test in a function context that requires std::io::Error
        fn takes_io_error(_: std::io::Error) -> bool {
            true
        }
        assert!(takes_io_error(Error::AlreadyExists.into()));
        println!("Error converts to std::io::Error via Into trait");
    }

    #[test]
    fn error_message_preserved() {
        let err = Error::NotFound;
        let io_err = err.to_io_error();
        let msg = format!("{}", io_err);
        assert!(
            msg.contains("NotFound"),
            "Error message should contain variant name: {}",
            msg
        );
        println!("Error message preserved in conversion: {}", msg);
    }

    #[test]
    fn file_block_device_create_and_write() {
        let test_file = format!("test_file_block_device_{}.dat", std::process::id());
        let total_blocks = 64;

        // Create a new file block device
        let mut device =
            FileBlockDevice::create(&test_file, total_blocks).expect("create file device");
        assert_eq!(device.total_blocks(), total_blocks);
        println!(
            "Created FileBlockDevice with {} blocks",
            device.total_blocks()
        );

        // Write some data
        let mut write_buf = [0u8; BLOCK_SIZE];
        write_buf[0..5].copy_from_slice(b"Hello");
        device
            .write_block(BlockAddress::new(1), &write_buf)
            .expect("write block");
        println!("Wrote block to file block device");

        // Read it back
        let mut read_buf = [0u8; BLOCK_SIZE];
        device
            .read_block(BlockAddress::new(1), &mut read_buf)
            .expect("read block");
        assert_eq!(&read_buf[0..5], b"Hello");
        println!("Read back data from file block device");

        // Sync to ensure it's on disk
        device.sync().expect("sync");
        println!("Synced file block device");

        // Clean up
        std::fs::remove_file(&test_file).expect("remove test file");
        println!("File block device test passed");
    }

    #[test]
    fn file_block_device_open_existing() {
        let test_file = format!("test_file_block_device_open_{}.dat", std::process::id());
        let total_blocks = 32;

        // First, create a device and write some data
        {
            let mut device =
                FileBlockDevice::create(&test_file, total_blocks).expect("create file device");
            let mut write_buf = [0u8; BLOCK_SIZE];
            write_buf[0..7].copy_from_slice(b"Persist");
            device
                .write_block(BlockAddress::new(5), &write_buf)
                .expect("write block");
            device.sync().expect("sync");
        }

        // Now open the existing file
        let device = FileBlockDevice::open(&test_file).expect("open existing file");
        assert_eq!(device.total_blocks(), total_blocks);
        println!("Opened existing FileBlockDevice");

        // Read back the data
        let mut read_buf = [0u8; BLOCK_SIZE];
        device
            .read_block(BlockAddress::new(5), &mut read_buf)
            .expect("read block");
        assert_eq!(&read_buf[0..7], b"Persist");
        println!("Data persisted across open");

        // Clean up
        std::fs::remove_file(&test_file).expect("remove test file");
        println!("File block device open test passed");
    }

    #[test]
    fn file_block_device_invalid_offset() {
        let test_file = format!("test_file_block_device_invalid_{}.dat", std::process::id());
        let total_blocks = 16;

        let mut device =
            FileBlockDevice::create(&test_file, total_blocks).expect("create file device");

        // Try to read beyond the end
        let mut buf = [0u8; BLOCK_SIZE];
        let result = device.read_block(BlockAddress::new(total_blocks), &mut buf);
        assert_eq!(result, Err(Error::InvalidOffset));
        println!("Read beyond end correctly rejected");

        // Try to write beyond the end
        let result = device.write_block(BlockAddress::new(total_blocks + 1), &buf);
        assert_eq!(result, Err(Error::InvalidOffset));
        println!("Write beyond end correctly rejected");

        // Clean up
        std::fs::remove_file(&test_file).expect("remove test file");
        println!("File block device invalid offset test passed");
    }

    #[test]
    fn file_block_device_with_lfs() {
        let test_file = format!("test_lfs_file_device_{}.dat", std::process::id());
        let total_blocks = 64;

        // Create filesystem on file block device
        {
            let device =
                FileBlockDevice::create(&test_file, total_blocks).expect("create file device");
            let mut lfs = Lfs::new(
                device,
                total_blocks,
                DeviceId::new(42),
                zero_time as fn() -> i64,
            )
            .expect("create filesystem");

            let fd = lfs.open_file("hello.txt").expect("create file");
            lfs.write(fd, b"Hello from FileBlockDevice!")
                .expect("write");
            lfs.close(fd).expect("close");

            lfs.device().sync().expect("sync");
            println!("Created filesystem and wrote file on FileBlockDevice");
        }

        // Reopen and verify
        {
            let device = FileBlockDevice::open(&test_file).expect("open file device");
            let mut lfs = Lfs::open(
                device,
                total_blocks,
                DeviceId::new(42),
                zero_time as fn() -> i64,
            )
            .expect("open filesystem");

            let fd = lfs.open_file("hello.txt").expect("open file");
            let mut buf = vec![0u8; 27];
            lfs.read(fd, &mut buf).expect("read");
            assert_eq!(&buf, b"Hello from FileBlockDevice!");
            lfs.close(fd).expect("close");
            println!("Reopened filesystem and verified data");
        }

        // Clean up
        std::fs::remove_file(&test_file).expect("remove test file");
        println!("LFS with FileBlockDevice test passed");
    }

    #[test]
    fn file_block_device_create_opens_existing() {
        let test_file = format!(
            "test_file_block_device_create_open_{}.dat",
            std::process::id()
        );

        // First, create a device and write some data
        {
            let mut device = FileBlockDevice::create(&test_file, 32).expect("create file device");
            let mut write_buf = [0u8; BLOCK_SIZE];
            write_buf[0..4].copy_from_slice(b"Test");
            device
                .write_block(BlockAddress::new(0), &write_buf)
                .expect("write block");
            device.sync().expect("sync");
        }

        // Call create again - should open existing file (not truncate)
        let device = FileBlockDevice::create(&test_file, 64).expect("create on existing");
        assert_eq!(
            device.total_blocks(),
            32,
            "Should use existing file's size, not requested size"
        );
        println!("Create on existing file opens it without truncation");

        // Verify data is still there
        let mut read_buf = [0u8; BLOCK_SIZE];
        device
            .read_block(BlockAddress::new(0), &mut read_buf)
            .expect("read block");
        assert_eq!(&read_buf[0..4], b"Test");
        println!("Existing data preserved when create opens existing file");

        // Clean up
        std::fs::remove_file(&test_file).expect("remove test file");
        println!("File block device create opens existing test passed");
    }
}

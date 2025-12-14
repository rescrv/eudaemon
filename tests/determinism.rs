//! Property-based tests for LFS determinism.
//!
//! These tests generate random sequences of filesystem operations and verify:
//! 1. The LFS behaves identically to a reference implementation
//! 2. Replaying the same operations produces identical on-disk state
//! 3. Persist/restore cycles preserve all data correctly

use std::collections::BTreeMap;

use proptest::prelude::*;
use proptest::test_runner::Config;

use synfs::BlockAddress;
use synfs::Error;
use synfs::FileDescriptor;
use synfs::Lfs;
use synfs::MemoryBlockDevice;
use synfs::SequentialBlockDevice;

/// Block size must match the LFS implementation.
const BLOCK_SIZE: usize = 4096;

/// Maximum file size for generated operations.
const MAX_OP_SIZE: usize = BLOCK_SIZE * 4;

/// Maximum seek position for generated operations.
const MAX_SEEK_POS: u64 = BLOCK_SIZE as u64 * 8;

/// Number of blocks in the test filesystem.
const TEST_FS_BLOCKS: usize = 256;

/////////////////////////////////////////////// FsOp ///////////////////////////////////////////////////

/// A filesystem operation that can be applied to both LFS and the reference implementation.
#[derive(Debug, Clone)]
enum FsOp {
    /// Open or create a file.
    Open { name: String },
    /// Close a file descriptor (by index into open files list).
    Close { fd_index: usize },
    /// Write data to a file.
    Write { fd_index: usize, data: Vec<u8> },
    /// Read data from a file.
    Read { fd_index: usize, len: usize },
    /// Seek to a position.
    Seek { fd_index: usize, pos: u64 },
    /// Truncate a file.
    Truncate { fd_index: usize, size: u64 },
    /// Remove a file.
    Remove { name: String },
}

////////////////////////////////////////// ReferenceFile ///////////////////////////////////////////////

/// A reference implementation of a file's contents.
#[derive(Debug, Clone, Default)]
struct ReferenceFile {
    data: Vec<u8>,
}

impl ReferenceFile {
    fn write(&mut self, pos: usize, data: &[u8]) {
        let end = pos + data.len();
        if end > self.data.len() {
            self.data.resize(end, 0);
        }
        self.data[pos..end].copy_from_slice(data);
    }

    fn read(&self, pos: usize, len: usize) -> Vec<u8> {
        if pos >= self.data.len() {
            return vec![];
        }
        let end = (pos + len).min(self.data.len());
        self.data[pos..end].to_vec()
    }

    fn truncate(&mut self, size: usize) {
        self.data.resize(size, 0);
    }

    fn size(&self) -> usize {
        self.data.len()
    }
}

////////////////////////////////////////// ReferenceOpenFile ///////////////////////////////////////////

/// An open file handle in the reference implementation.
#[derive(Debug, Clone)]
struct ReferenceOpenFile {
    ino: u64,
    position: usize,
    closed: bool,
}

/////////////////////////////////////////// ReferenceFs ////////////////////////////////////////////////

/// A reference implementation of a filesystem using simple in-memory storage.
///
/// Uses inode-based storage to properly support UNIX unlink semantics:
/// files can be unlinked while open, and the data remains accessible
/// via existing file descriptors until they are all closed.
#[derive(Debug)]
struct ReferenceFs {
    /// Maps inode number to file contents.
    inodes: BTreeMap<u64, ReferenceFile>,
    /// Maps filename to inode number (the "directory").
    directory: BTreeMap<String, u64>,
    /// Open file descriptors.
    open_files: Vec<ReferenceOpenFile>,
    /// Next inode number to allocate.
    next_ino: u64,
    max_file_size: usize,
}

impl ReferenceFs {
    fn new(max_file_size: usize) -> Self {
        Self {
            inodes: BTreeMap::new(),
            directory: BTreeMap::new(),
            open_files: Vec::new(),
            next_ino: 1,
            max_file_size,
        }
    }

    fn open(&mut self, name: &str) -> Result<usize, Error> {
        let ino = if let Some(&ino) = self.directory.get(name) {
            // File exists, open it
            ino
        } else {
            // Create new file
            let ino = self.next_ino;
            self.next_ino += 1;
            self.inodes.insert(ino, ReferenceFile::default());
            self.directory.insert(name.to_string(), ino);
            ino
        };

        let fd_index = self.open_files.len();
        self.open_files.push(ReferenceOpenFile {
            ino,
            position: 0,
            closed: false,
        });
        Ok(fd_index)
    }

    fn close(&mut self, fd_index: usize) -> Result<(), Error> {
        if fd_index >= self.open_files.len() || self.open_files[fd_index].closed {
            return Err(Error::InvalidFd);
        }
        let ino = self.open_files[fd_index].ino;
        self.open_files[fd_index].closed = true;

        // If inode is not in directory (unlinked) and no other FDs point to it, free it
        let in_directory = self.directory.values().any(|&i| i == ino);
        if !in_directory {
            let still_open = self.open_files.iter().any(|f| !f.closed && f.ino == ino);
            if !still_open {
                self.inodes.remove(&ino);
            }
        }

        Ok(())
    }

    fn write(&mut self, fd_index: usize, data: &[u8]) -> Result<usize, Error> {
        if fd_index >= self.open_files.len() || self.open_files[fd_index].closed {
            return Err(Error::InvalidFd);
        }
        let open_file = &self.open_files[fd_index];
        let ino = open_file.ino;
        let pos = open_file.position;

        if pos + data.len() > self.max_file_size {
            return Err(Error::FileTooLarge);
        }

        let file = self.inodes.get_mut(&ino).ok_or(Error::NotFound)?;
        file.write(pos, data);
        self.open_files[fd_index].position = pos + data.len();
        Ok(data.len())
    }

    fn read(&mut self, fd_index: usize, len: usize) -> Result<Vec<u8>, Error> {
        if fd_index >= self.open_files.len() || self.open_files[fd_index].closed {
            return Err(Error::InvalidFd);
        }
        let open_file = &self.open_files[fd_index];
        let ino = open_file.ino;
        let pos = open_file.position;

        let file = self.inodes.get(&ino).ok_or(Error::NotFound)?;
        let data = file.read(pos, len);
        self.open_files[fd_index].position = pos + data.len();
        Ok(data)
    }

    fn seek(&mut self, fd_index: usize, pos: u64) -> Result<(), Error> {
        if fd_index >= self.open_files.len() || self.open_files[fd_index].closed {
            return Err(Error::InvalidFd);
        }
        self.open_files[fd_index].position = pos as usize;
        Ok(())
    }

    fn truncate(&mut self, fd_index: usize, size: u64) -> Result<(), Error> {
        if fd_index >= self.open_files.len() || self.open_files[fd_index].closed {
            return Err(Error::InvalidFd);
        }
        if size as usize > self.max_file_size {
            return Err(Error::FileTooLarge);
        }
        let ino = self.open_files[fd_index].ino;

        let file = self.inodes.get_mut(&ino).ok_or(Error::NotFound)?;
        file.truncate(size as usize);
        Ok(())
    }

    fn remove(&mut self, name: &str) -> Result<(), Error> {
        // Check if file exists in directory
        let ino = *self.directory.get(name).ok_or(Error::NotFound)?;

        // Remove from directory (unlink)
        self.directory.remove(name);

        // If no open FDs point to this inode, free it immediately
        let still_open = self.open_files.iter().any(|f| !f.closed && f.ino == ino);
        if !still_open {
            self.inodes.remove(&ino);
        }
        // Otherwise, inode stays around until last FD is closed

        Ok(())
    }

    /// Gets the file data for a file by name (for verification).
    fn get_file(&self, name: &str) -> Option<&ReferenceFile> {
        let ino = self.directory.get(name)?;
        self.inodes.get(ino)
    }
}

/////////////////////////////////////////// LfsAdapter /////////////////////////////////////////////////

/// Adapter to track open file descriptors for LFS.
///
/// Uses `SequentialBlockDevice` to enforce that all block writes happen in strictly
/// sequential order, verifying a key property of log-structured filesystems.
struct LfsAdapter {
    lfs: Lfs<SequentialBlockDevice<MemoryBlockDevice>>,
    open_fds: Vec<Option<FileDescriptor>>,
}

impl LfsAdapter {
    fn new(lfs: Lfs<SequentialBlockDevice<MemoryBlockDevice>>) -> Self {
        Self {
            lfs,
            open_fds: Vec::new(),
        }
    }

    fn open(&mut self, name: &str) -> Result<usize, Error> {
        let fd = self.lfs.open_file(name)?;
        let fd_index = self.open_fds.len();
        self.open_fds.push(Some(fd));
        Ok(fd_index)
    }

    fn close(&mut self, fd_index: usize) -> Result<(), Error> {
        if fd_index >= self.open_fds.len() {
            return Err(Error::InvalidFd);
        }
        let fd = self.open_fds[fd_index].ok_or(Error::InvalidFd)?;
        self.lfs.close(fd)?;
        self.open_fds[fd_index] = None;
        Ok(())
    }

    fn write(&mut self, fd_index: usize, data: &[u8]) -> Result<usize, Error> {
        if fd_index >= self.open_fds.len() {
            return Err(Error::InvalidFd);
        }
        let fd = self.open_fds[fd_index].ok_or(Error::InvalidFd)?;
        self.lfs.write(fd, data)
    }

    fn read(&mut self, fd_index: usize, len: usize) -> Result<Vec<u8>, Error> {
        if fd_index >= self.open_fds.len() {
            return Err(Error::InvalidFd);
        }
        let fd = self.open_fds[fd_index].ok_or(Error::InvalidFd)?;
        let mut buf = vec![0u8; len];
        let n = self.lfs.read(fd, &mut buf)?;
        buf.truncate(n);
        Ok(buf)
    }

    fn seek(&mut self, fd_index: usize, pos: u64) -> Result<(), Error> {
        if fd_index >= self.open_fds.len() {
            return Err(Error::InvalidFd);
        }
        let fd = self.open_fds[fd_index].ok_or(Error::InvalidFd)?;
        self.lfs.seek(fd, pos)
    }

    fn truncate(&mut self, fd_index: usize, size: u64) -> Result<(), Error> {
        if fd_index >= self.open_fds.len() {
            return Err(Error::InvalidFd);
        }
        let fd = self.open_fds[fd_index].ok_or(Error::InvalidFd)?;
        self.lfs.truncate(fd, size)
    }

    fn remove(&mut self, name: &str) -> Result<(), Error> {
        self.lfs.remove(name)
    }

    fn into_inner(self) -> Vec<u8> {
        self.lfs.into_device().into_inner().into_inner()
    }
}

//////////////////////////////////////// Proptest Strategies ///////////////////////////////////////////

fn filename_strategy() -> impl Strategy<Value = String> {
    prop::string::string_regex("[a-z]{1,8}\\.txt")
        .expect("valid regex")
        .prop_filter("non-empty", |s| !s.is_empty())
}

fn data_strategy() -> impl Strategy<Value = Vec<u8>> {
    prop::collection::vec(any::<u8>(), 0..MAX_OP_SIZE)
}

fn fs_op_strategy() -> impl Strategy<Value = FsOp> {
    prop_oneof![
        3 => filename_strategy().prop_map(|name| FsOp::Open { name }),
        1 => (0..10usize).prop_map(|fd_index| FsOp::Close { fd_index }),
        5 => (0..10usize, data_strategy()).prop_map(|(fd_index, data)| FsOp::Write { fd_index, data }),
        5 => (0..10usize, 1..MAX_OP_SIZE).prop_map(|(fd_index, len)| FsOp::Read { fd_index, len }),
        3 => (0..10usize, 0..MAX_SEEK_POS).prop_map(|(fd_index, pos)| FsOp::Seek { fd_index, pos }),
        2 => (0..10usize, 0..MAX_SEEK_POS).prop_map(|(fd_index, size)| FsOp::Truncate { fd_index, size }),
        1 => filename_strategy().prop_map(|name| FsOp::Remove { name }),
    ]
}

fn ops_strategy() -> impl Strategy<Value = Vec<FsOp>> {
    prop::collection::vec(fs_op_strategy(), 1..100)
}

////////////////////////////////////////// Test Execution //////////////////////////////////////////////

/// Execute a single operation on both implementations and compare results.
/// Operations are tried on LFS first; if LFS returns NoSpace, the operation becomes a NOP
/// (skipped on reference) to keep both implementations in sync. This allows testing
/// recovery from full-fs conditions.
fn execute_op(
    op: &FsOp,
    lfs: &mut LfsAdapter,
    reference: &mut ReferenceFs,
    open_count: &mut usize,
) {
    match op {
        FsOp::Open { name } => {
            let lfs_result = lfs.open(name);
            if lfs_result == Err(Error::NoSpace) {
                // NOP: LFS is full, skip on reference to stay in sync
                return;
            }
            let ref_result = reference.open(name);

            match (&lfs_result, &ref_result) {
                (Ok(_), Ok(_)) => {
                    *open_count += 1;
                }
                (Err(e1), Err(e2)) => {
                    assert_eq!(e1, e2, "Open error mismatch for {:?}", name);
                }
                _ => {
                    panic!(
                        "Open result mismatch for {:?}: lfs={:?}, ref={:?}",
                        name, lfs_result, ref_result
                    );
                }
            }
        }
        FsOp::Close { fd_index } => {
            if *fd_index < *open_count {
                let lfs_result = lfs.close(*fd_index);
                let ref_result = reference.close(*fd_index);

                match (&lfs_result, &ref_result) {
                    (Ok(()), Ok(())) => {}
                    (Err(e1), Err(e2)) => {
                        assert_eq!(e1, e2, "Close error mismatch for fd_index {}", fd_index);
                    }
                    _ => {
                        panic!(
                            "Close result mismatch for fd_index {}: lfs={:?}, ref={:?}",
                            fd_index, lfs_result, ref_result
                        );
                    }
                }
            }
        }
        FsOp::Write { fd_index, data } => {
            if *fd_index < *open_count {
                let lfs_result = lfs.write(*fd_index, data);
                if lfs_result == Err(Error::NoSpace) {
                    // NOP: LFS is full, skip on reference to stay in sync
                    return;
                }
                let ref_result = reference.write(*fd_index, data);

                match (&lfs_result, &ref_result) {
                    (Ok(n1), Ok(n2)) => {
                        assert_eq!(n1, n2, "Write length mismatch");
                    }
                    (Err(Error::FileTooLarge), Err(Error::FileTooLarge)) => {}
                    (Err(Error::InvalidFd), Err(Error::InvalidFd)) => {}
                    _ => {
                        panic!(
                            "Write result mismatch for fd_index {}: lfs={:?}, ref={:?}",
                            fd_index, lfs_result, ref_result
                        );
                    }
                }
            }
        }
        FsOp::Read { fd_index, len } => {
            if *fd_index < *open_count {
                let lfs_result = lfs.read(*fd_index, *len);
                let ref_result = reference.read(*fd_index, *len);

                match (&lfs_result, &ref_result) {
                    (Ok(d1), Ok(d2)) => {
                        assert_eq!(d1, d2, "Read data mismatch at fd_index {}", fd_index);
                    }
                    (Err(e1), Err(e2)) => {
                        assert_eq!(e1, e2, "Read error mismatch for fd_index {}", fd_index);
                    }
                    _ => {
                        panic!(
                            "Read result mismatch for fd_index {}: lfs={:?}, ref={:?}",
                            fd_index, lfs_result, ref_result
                        );
                    }
                }
            }
        }
        FsOp::Seek { fd_index, pos } => {
            if *fd_index < *open_count {
                let lfs_result = lfs.seek(*fd_index, *pos);
                let ref_result = reference.seek(*fd_index, *pos);

                match (&lfs_result, &ref_result) {
                    (Ok(()), Ok(())) => {}
                    (Err(e1), Err(e2)) => {
                        assert_eq!(e1, e2, "Seek error mismatch for fd_index {}", fd_index);
                    }
                    _ => {
                        panic!(
                            "Seek result mismatch for fd_index {}: lfs={:?}, ref={:?}",
                            fd_index, lfs_result, ref_result
                        );
                    }
                }
            }
        }
        FsOp::Truncate { fd_index, size } => {
            if *fd_index < *open_count {
                let lfs_result = lfs.truncate(*fd_index, *size);
                if lfs_result == Err(Error::NoSpace) {
                    // NOP: LFS is full, skip on reference to stay in sync
                    return;
                }
                let ref_result = reference.truncate(*fd_index, *size);

                match (&lfs_result, &ref_result) {
                    (Ok(()), Ok(())) => {}
                    (Err(Error::FileTooLarge), Err(Error::FileTooLarge)) => {}
                    (Err(Error::InvalidFd), Err(Error::InvalidFd)) => {}
                    _ => {
                        panic!(
                            "Truncate result mismatch for fd_index {}: lfs={:?}, ref={:?}",
                            fd_index, lfs_result, ref_result
                        );
                    }
                }
            }
        }
        FsOp::Remove { name } => {
            let lfs_result = lfs.remove(name);
            if lfs_result == Err(Error::NoSpace) {
                // NOP: LFS is full, skip on reference to stay in sync
                return;
            }
            let ref_result = reference.remove(name);

            match (&lfs_result, &ref_result) {
                (Ok(()), Ok(())) => {}
                (Err(Error::NotFound), Err(Error::NotFound)) => {}
                _ => {
                    panic!(
                        "Remove result mismatch for {:?}: lfs={:?}, ref={:?}",
                        name, lfs_result, ref_result
                    );
                }
            }
        }
    }
}

/// Run a sequence of operations on both implementations.
///
/// The LFS uses a `SequentialBlockDevice` wrapper to enforce that all block writes
/// happen in strictly sequential order, which is a key invariant of log-structured
/// filesystems.
fn run_ops(ops: &[FsOp]) -> (LfsAdapter, ReferenceFs) {
    let data = vec![0u8; TEST_FS_BLOCKS * BLOCK_SIZE];
    let mem_device = MemoryBlockDevice::new(data);
    let log_start = BlockAddress::new(1);
    let log_end = BlockAddress::new(TEST_FS_BLOCKS as u64);
    let seq_device = SequentialBlockDevice::new(mem_device, log_start, log_end);
    let lfs = Lfs::new(seq_device, TEST_FS_BLOCKS as u64).expect("Failed to create LFS");
    let max_file_size = TEST_FS_BLOCKS * BLOCK_SIZE / 10;

    let mut lfs_adapter = LfsAdapter::new(lfs);
    let mut reference = ReferenceFs::new(max_file_size);
    let mut open_count = 0;

    for op in ops {
        execute_op(op, &mut lfs_adapter, &mut reference, &mut open_count);
    }

    (lfs_adapter, reference)
}

///////////////////////////////////////////// Proptests ////////////////////////////////////////////////

proptest! {
    #![proptest_config(Config::with_cases(1000))]

    #[test]
    fn lfs_matches_reference(ops in ops_strategy()) {
        let (lfs, reference) = run_ops(&ops);

        // Close all open fds in lfs so we can reopen and verify
        let data = lfs.into_inner();
        let mut lfs_verify = Lfs::open_vec(data).expect("Failed to reopen LFS for verification");

        // Verify each file's contents match the reference
        // Only check files that are still in the directory (not unlinked)
        for (name, &ino) in &reference.directory {
            let ref_file = reference.inodes.get(&ino).expect("inode should exist");
            let fd = lfs_verify.open_file(name).expect("Failed to open file for verification");
            lfs_verify.seek(fd, 0).expect("Failed to seek");

            let mut buf = vec![0u8; ref_file.size()];
            let n = lfs_verify.read(fd, &mut buf).expect("Failed to read");

            prop_assert_eq!(n, ref_file.size(), "Size mismatch for {}", name);
            prop_assert_eq!(&buf[..n], &ref_file.data[..], "Data mismatch for {}", name);

            lfs_verify.close(fd).expect("Failed to close");
        }
    }

    /// Tests that removing a file allows recreating it with new content.
    #[test]
    fn remove_and_recreate(
        name in filename_strategy(),
        data1 in data_strategy(),
        data2 in data_strategy()
    ) {
        let data = vec![0u8; TEST_FS_BLOCKS * BLOCK_SIZE];
        let mem_device = MemoryBlockDevice::new(data);
        let log_start = BlockAddress::new(1);
        let log_end = BlockAddress::new(TEST_FS_BLOCKS as u64);
        let seq_device = SequentialBlockDevice::new(mem_device, log_start, log_end);
        let mut lfs = Lfs::new(seq_device, TEST_FS_BLOCKS as u64).expect("Failed to create LFS");

        // Create file and write initial data
        let fd = lfs.open_file(&name).expect("Failed to open file");
        if !data1.is_empty() {
            let _ = lfs.write(fd, &data1); // May fail with NoSpace, that's ok
        }
        lfs.close(fd).expect("Failed to close");

        // Remove the file
        let _ = lfs.remove(&name); // May fail if file wasn't created

        // Recreate file with new data
        let fd = lfs.open_file(&name).expect("Failed to recreate file");
        if !data2.is_empty() {
            let _ = lfs.write(fd, &data2); // May fail with NoSpace
        }
        lfs.seek(fd, 0).expect("Failed to seek");

        // Read back and verify
        let size = lfs.file_size(fd).expect("Failed to get size");
        let mut buf = vec![0u8; size as usize];
        let n = lfs.read(fd, &mut buf).expect("Failed to read");

        // File should contain data2 (or be empty if write failed)
        if n > 0 {
            prop_assert_eq!(&buf[..n], &data2[..n], "Data mismatch after remove and recreate");
        }

        lfs.close(fd).expect("Failed to close");
    }

    /// Tests UNIX unlink semantics: file remains accessible via open FD after remove.
    #[test]
    fn remove_while_open_then_write_read(
        name in filename_strategy(),
        data1 in data_strategy(),
        data2 in prop::collection::vec(any::<u8>(), 1..1000)
    ) {
        let data = vec![0u8; TEST_FS_BLOCKS * BLOCK_SIZE];
        let mem_device = MemoryBlockDevice::new(data);
        let log_start = BlockAddress::new(1);
        let log_end = BlockAddress::new(TEST_FS_BLOCKS as u64);
        let seq_device = SequentialBlockDevice::new(mem_device, log_start, log_end);
        let mut lfs = Lfs::new(seq_device, TEST_FS_BLOCKS as u64).expect("Failed to create LFS");

        // Create file and write initial data
        let fd = lfs.open_file(&name).expect("Failed to open file");
        let wrote_initial = if !data1.is_empty() {
            lfs.write(fd, &data1).is_ok()
        } else {
            true
        };

        // Remove the file while it's still open
        let _ = lfs.remove(&name);

        if wrote_initial {
            // Should still be able to read via the open FD
            lfs.seek(fd, 0).expect("Failed to seek");
            let mut buf = vec![0u8; data1.len()];
            let n = lfs.read(fd, &mut buf).expect("Failed to read from unlinked file");
            if n > 0 && !data1.is_empty() {
                prop_assert_eq!(&buf[..n], &data1[..n], "Data mismatch after unlink");
            }

            // Should still be able to write via the open FD
            let write_result = lfs.write(fd, &data2);
            if write_result.is_ok() {
                // Verify the write
                lfs.seek(fd, data1.len() as u64).expect("Failed to seek");
                let mut buf2 = vec![0u8; data2.len()];
                let n2 = lfs.read(fd, &mut buf2).expect("Failed to read after write");
                if n2 > 0 {
                    prop_assert_eq!(&buf2[..n2], &data2[..n2], "Data mismatch after write to unlinked");
                }
            }
        }

        lfs.close(fd).expect("Failed to close");
    }

    /// Tests that multiple FDs to a removed file all continue to work.
    #[test]
    fn remove_with_multiple_open_fds(
        name in filename_strategy(),
        data1 in prop::collection::vec(any::<u8>(), 1..1000),
        data2 in prop::collection::vec(any::<u8>(), 1..1000)
    ) {
        let data = vec![0u8; TEST_FS_BLOCKS * BLOCK_SIZE];
        let mem_device = MemoryBlockDevice::new(data);
        let log_start = BlockAddress::new(1);
        let log_end = BlockAddress::new(TEST_FS_BLOCKS as u64);
        let seq_device = SequentialBlockDevice::new(mem_device, log_start, log_end);
        let mut lfs = Lfs::new(seq_device, TEST_FS_BLOCKS as u64).expect("Failed to create LFS");

        // Open file twice
        let fd1 = lfs.open_file(&name).expect("Failed to open file first time");
        let fd2 = lfs.open_file(&name).expect("Failed to open file second time");

        // Write via first FD
        let wrote = lfs.write(fd1, &data1).is_ok();

        // Remove the file
        lfs.remove(&name).expect("Failed to remove file");

        if wrote {
            // Both FDs should still be able to read
            lfs.seek(fd1, 0).expect("Failed to seek fd1");
            let mut buf1 = vec![0u8; data1.len()];
            let n1 = lfs.read(fd1, &mut buf1).expect("Failed to read via fd1");
            prop_assert_eq!(&buf1[..n1], &data1[..n1], "Data mismatch via fd1");

            lfs.seek(fd2, 0).expect("Failed to seek fd2");
            let mut buf2 = vec![0u8; data1.len()];
            let n2 = lfs.read(fd2, &mut buf2).expect("Failed to read via fd2");
            prop_assert_eq!(&buf2[..n2], &data1[..n2], "Data mismatch via fd2");

            // Write via second FD, read via first
            if lfs.write(fd2, &data2).is_ok() {
                lfs.seek(fd1, data1.len() as u64).expect("Failed to seek fd1");
                let mut buf3 = vec![0u8; data2.len()];
                let n3 = lfs.read(fd1, &mut buf3).expect("Failed to read new data via fd1");
                if n3 > 0 {
                    prop_assert_eq!(&buf3[..n3], &data2[..n3], "Cross-FD data mismatch");
                }
            }
        }

        // Close first FD, second should still work
        lfs.close(fd1).expect("Failed to close fd1");

        if wrote {
            lfs.seek(fd2, 0).expect("Failed to seek fd2 after fd1 close");
            let mut buf = vec![0u8; data1.len()];
            let n = lfs.read(fd2, &mut buf).expect("Failed to read after fd1 close");
            prop_assert_eq!(&buf[..n], &data1[..n], "Data mismatch after closing fd1");
        }

        lfs.close(fd2).expect("Failed to close fd2");
    }

    /// Tests remove followed by immediate recreation with writes.
    #[test]
    fn remove_recreate_write_persists(
        name in filename_strategy(),
        data1 in prop::collection::vec(any::<u8>(), 1..500),
        data2 in prop::collection::vec(any::<u8>(), 1..500)
    ) {
        let data = vec![0u8; TEST_FS_BLOCKS * BLOCK_SIZE];
        let mem_device = MemoryBlockDevice::new(data);
        let log_start = BlockAddress::new(1);
        let log_end = BlockAddress::new(TEST_FS_BLOCKS as u64);
        let seq_device = SequentialBlockDevice::new(mem_device, log_start, log_end);
        let mut lfs = Lfs::new(seq_device, TEST_FS_BLOCKS as u64).expect("Failed to create LFS");

        // Create and write initial data
        let fd = lfs.open_file(&name).expect("Failed to open file");
        let _ = lfs.write(fd, &data1);
        lfs.close(fd).expect("Failed to close");

        // Remove file
        let _ = lfs.remove(&name);

        // Recreate and write new data
        let fd = lfs.open_file(&name).expect("Failed to recreate file");
        let wrote = lfs.write(fd, &data2).is_ok();
        lfs.close(fd).expect("Failed to close");

        // Persist and restore
        let raw_data = lfs.into_device().into_inner().into_inner();
        let mut lfs2 = Lfs::open_vec(raw_data).expect("Failed to restore LFS");

        // Verify the file has the new data (not the old)
        let fd = lfs2.open_file(&name).expect("Failed to open after restore");
        let size = lfs2.file_size(fd).expect("Failed to get size");
        let mut buf = vec![0u8; size as usize];
        let n = lfs2.read(fd, &mut buf).expect("Failed to read");

        if wrote && n > 0 {
            prop_assert_eq!(&buf[..n], &data2[..n], "Data should be new content after remove/recreate");
        }

        lfs2.close(fd).expect("Failed to close");
    }

    /// Tests that removing a file and then writing to other files works correctly.
    #[test]
    fn remove_then_write_other_files(
        name1 in filename_strategy(),
        name2 in "[b-z]{1,8}\\.txt",  // Different pattern to avoid collision
        data1 in prop::collection::vec(any::<u8>(), 1..500),
        data2 in prop::collection::vec(any::<u8>(), 1..500)
    ) {
        prop_assume!(name1 != name2);

        let data = vec![0u8; TEST_FS_BLOCKS * BLOCK_SIZE];
        let mem_device = MemoryBlockDevice::new(data);
        let log_start = BlockAddress::new(1);
        let log_end = BlockAddress::new(TEST_FS_BLOCKS as u64);
        let seq_device = SequentialBlockDevice::new(mem_device, log_start, log_end);
        let mut lfs = Lfs::new(seq_device, TEST_FS_BLOCKS as u64).expect("Failed to create LFS");

        // Create first file
        let fd1 = lfs.open_file(&name1).expect("Failed to open file1");
        let _ = lfs.write(fd1, &data1);
        lfs.close(fd1).expect("Failed to close file1");

        // Create second file
        let fd2 = lfs.open_file(&name2).expect("Failed to open file2");
        let wrote2 = lfs.write(fd2, &data2).is_ok();
        lfs.close(fd2).expect("Failed to close file2");

        // Remove first file
        let _ = lfs.remove(&name1);

        // Second file should still be intact
        let fd2 = lfs.open_file(&name2).expect("Failed to reopen file2");
        lfs.seek(fd2, 0).expect("Failed to seek");
        let mut buf = vec![0u8; data2.len()];
        let n = lfs.read(fd2, &mut buf).expect("Failed to read file2");

        if wrote2 && n > 0 {
            prop_assert_eq!(&buf[..n], &data2[..n], "File2 data corrupted after removing file1");
        }

        lfs.close(fd2).expect("Failed to close");
    }

    /// Tests sequence: write, remove, write to same filename, read.
    #[test]
    fn write_remove_write_read_same_file(
        name in filename_strategy(),
        data1 in prop::collection::vec(any::<u8>(), 100..500),
        data2 in prop::collection::vec(any::<u8>(), 100..500),
        data3 in prop::collection::vec(any::<u8>(), 100..500)
    ) {
        let data = vec![0u8; TEST_FS_BLOCKS * BLOCK_SIZE];
        let mem_device = MemoryBlockDevice::new(data);
        let log_start = BlockAddress::new(1);
        let log_end = BlockAddress::new(TEST_FS_BLOCKS as u64);
        let seq_device = SequentialBlockDevice::new(mem_device, log_start, log_end);
        let mut lfs = Lfs::new(seq_device, TEST_FS_BLOCKS as u64).expect("Failed to create LFS");

        // First write
        let fd = lfs.open_file(&name).expect("Failed to open");
        let _ = lfs.write(fd, &data1);
        lfs.close(fd).expect("Failed to close");

        // Remove
        let _ = lfs.remove(&name);

        // Second write (to recreated file)
        let fd = lfs.open_file(&name).expect("Failed to reopen");
        let wrote2 = lfs.write(fd, &data2).is_ok();
        lfs.close(fd).expect("Failed to close");

        // Third write (append)
        let fd = lfs.open_file(&name).expect("Failed to open again");
        lfs.seek(fd, data2.len() as u64).expect("Failed to seek");
        let wrote3 = lfs.write(fd, &data3).is_ok();
        lfs.close(fd).expect("Failed to close");

        // Read and verify
        let fd = lfs.open_file(&name).expect("Failed to open for read");
        let size = lfs.file_size(fd).expect("Failed to get size");
        let mut buf = vec![0u8; size as usize];
        let n = lfs.read(fd, &mut buf).expect("Failed to read");

        if wrote2 && wrote3 {
            // Should have data2 + data3, NOT data1
            let expected_size = data2.len() + data3.len();
            prop_assert_eq!(n, expected_size, "Size mismatch");
            prop_assert_eq!(&buf[..data2.len()], &data2[..], "First part should be data2");
            prop_assert_eq!(&buf[data2.len()..n], &data3[..], "Second part should be data3");
        }

        lfs.close(fd).expect("Failed to close");
    }

    #[test]
    fn deterministic_replay(ops in ops_strategy()) {
        let (lfs1, _) = run_ops(&ops);
        let (lfs2, _) = run_ops(&ops);

        let data1 = lfs1.into_inner();
        let data2 = lfs2.into_inner();

        prop_assert_eq!(data1, data2, "Replay produced different results");
    }

    #[test]
    fn persist_restore_preserves_data(ops in ops_strategy()) {
        let (lfs, reference) = run_ops(&ops);

        let data = lfs.into_inner();

        let mut restored_lfs = Lfs::open_vec(data).expect("Failed to restore LFS");

        // Only check files that are still in the directory (not unlinked)
        for (name, &ino) in &reference.directory {
            let ref_file = reference.inodes.get(&ino).expect("inode should exist");
            let fd = restored_lfs.open_file(name).expect("Failed to open restored file");

            restored_lfs.seek(fd, 0).expect("Failed to seek");
            let mut buf = vec![0u8; ref_file.size()];
            let n = restored_lfs.read(fd, &mut buf).expect("Failed to read");

            prop_assert_eq!(n, ref_file.size(), "Size mismatch for {}", name);
            prop_assert_eq!(&buf[..n], &ref_file.data[..], "Data mismatch for {}", name);

            restored_lfs.close(fd).expect("Failed to close");
        }
    }
}

//////////////////////////////////////////// Manual Tests //////////////////////////////////////////////

#[test]
fn specific_write_read_sequence() {
    let ops = vec![
        FsOp::Open {
            name: "test.txt".to_string(),
        },
        FsOp::Write {
            fd_index: 0,
            data: b"Hello, World!".to_vec(),
        },
        FsOp::Seek {
            fd_index: 0,
            pos: 0,
        },
        FsOp::Read {
            fd_index: 0,
            len: 13,
        },
        FsOp::Seek {
            fd_index: 0,
            pos: 7,
        },
        FsOp::Write {
            fd_index: 0,
            data: b"LFS!".to_vec(),
        },
        FsOp::Seek {
            fd_index: 0,
            pos: 0,
        },
        FsOp::Read {
            fd_index: 0,
            len: 20,
        },
    ];

    let (lfs, reference) = run_ops(&ops);

    let ref_file = reference.get_file("test.txt").expect("File should exist");
    // "Hello, World!" (13 chars), then write "LFS!" at position 7 overwrites indices 7-10
    // H(0) e(1) l(2) l(3) o(4) ,(5) (6) W(7) o(8) r(9) l(10) d(11) !(12)
    // becomes: H e l l o ,   L F S ! d ! = "Hello, LFS!d!"
    assert_eq!(ref_file.data, b"Hello, LFS!d!");
    println!(
        "Reference file contents: {:?}",
        String::from_utf8_lossy(&ref_file.data)
    );

    drop(lfs);
}

#[test]
fn multiple_files_interleaved() {
    let ops = vec![
        FsOp::Open {
            name: "a.txt".to_string(),
        },
        FsOp::Open {
            name: "b.txt".to_string(),
        },
        FsOp::Write {
            fd_index: 0,
            data: b"AAA".to_vec(),
        },
        FsOp::Write {
            fd_index: 1,
            data: b"BBB".to_vec(),
        },
        FsOp::Write {
            fd_index: 0,
            data: b"aaa".to_vec(),
        },
        FsOp::Write {
            fd_index: 1,
            data: b"bbb".to_vec(),
        },
        FsOp::Seek {
            fd_index: 0,
            pos: 0,
        },
        FsOp::Seek {
            fd_index: 1,
            pos: 0,
        },
        FsOp::Read {
            fd_index: 0,
            len: 10,
        },
        FsOp::Read {
            fd_index: 1,
            len: 10,
        },
    ];

    let (lfs, reference) = run_ops(&ops);

    assert_eq!(
        reference.get_file("a.txt").unwrap().data,
        b"AAAaaa".to_vec()
    );
    assert_eq!(
        reference.get_file("b.txt").unwrap().data,
        b"BBBbbb".to_vec()
    );
    println!("Multiple interleaved files work correctly");

    drop(lfs);
}

#[test]
fn truncate_and_rewrite() {
    let ops = vec![
        FsOp::Open {
            name: "trunc.txt".to_string(),
        },
        FsOp::Write {
            fd_index: 0,
            data: b"0123456789".to_vec(),
        },
        FsOp::Truncate {
            fd_index: 0,
            size: 5,
        },
        FsOp::Seek {
            fd_index: 0,
            pos: 5,
        },
        FsOp::Write {
            fd_index: 0,
            data: b"ABCDE".to_vec(),
        },
        FsOp::Seek {
            fd_index: 0,
            pos: 0,
        },
        FsOp::Read {
            fd_index: 0,
            len: 20,
        },
    ];

    let (lfs, reference) = run_ops(&ops);

    assert_eq!(
        reference.get_file("trunc.txt").unwrap().data,
        b"01234ABCDE".to_vec()
    );
    println!("Truncate and rewrite works correctly");

    drop(lfs);
}

#[test]
fn sparse_file() {
    let ops = vec![
        FsOp::Open {
            name: "sparse.txt".to_string(),
        },
        FsOp::Write {
            fd_index: 0,
            data: b"START".to_vec(),
        },
        FsOp::Seek {
            fd_index: 0,
            pos: 1000,
        },
        FsOp::Write {
            fd_index: 0,
            data: b"END".to_vec(),
        },
        FsOp::Seek {
            fd_index: 0,
            pos: 0,
        },
        FsOp::Read {
            fd_index: 0,
            len: 1003,
        },
    ];

    let (lfs, reference) = run_ops(&ops);

    let ref_file = reference.get_file("sparse.txt").unwrap();
    assert_eq!(ref_file.size(), 1003);
    assert_eq!(&ref_file.data[0..5], b"START");
    assert_eq!(&ref_file.data[5..1000], &vec![0u8; 995][..]);
    assert_eq!(&ref_file.data[1000..1003], b"END");
    println!("Sparse file works correctly");

    drop(lfs);
}

#[test]
fn replay_produces_identical_state() {
    let ops = vec![
        FsOp::Open {
            name: "file1.txt".to_string(),
        },
        FsOp::Open {
            name: "file2.txt".to_string(),
        },
        FsOp::Write {
            fd_index: 0,
            data: vec![0xAA; 5000],
        },
        FsOp::Write {
            fd_index: 1,
            data: vec![0xBB; 3000],
        },
        FsOp::Seek {
            fd_index: 0,
            pos: 2500,
        },
        FsOp::Write {
            fd_index: 0,
            data: vec![0xCC; 1000],
        },
        FsOp::Truncate {
            fd_index: 1,
            size: 1500,
        },
    ];

    let (lfs1, _) = run_ops(&ops);
    let (lfs2, _) = run_ops(&ops);

    let data1 = lfs1.into_inner();
    let data2 = lfs2.into_inner();

    assert_eq!(data1, data2, "Replay must produce identical on-disk state");
    println!("Replay produces identical state verified");
}

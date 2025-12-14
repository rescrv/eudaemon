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
    name: String,
    position: usize,
    closed: bool,
}

/////////////////////////////////////////// ReferenceFs ////////////////////////////////////////////////

/// A reference implementation of a filesystem using simple in-memory storage.
#[derive(Debug)]
struct ReferenceFs {
    files: BTreeMap<String, ReferenceFile>,
    open_files: Vec<ReferenceOpenFile>,
    max_file_size: usize,
}

impl ReferenceFs {
    fn new(max_file_size: usize) -> Self {
        Self {
            files: BTreeMap::new(),
            open_files: Vec::new(),
            max_file_size,
        }
    }

    fn open(&mut self, name: &str) -> Result<usize, Error> {
        self.files.entry(name.to_string()).or_default();
        let fd_index = self.open_files.len();
        self.open_files.push(ReferenceOpenFile {
            name: name.to_string(),
            position: 0,
            closed: false,
        });
        Ok(fd_index)
    }

    fn close(&mut self, fd_index: usize) -> Result<(), Error> {
        if fd_index >= self.open_files.len() || self.open_files[fd_index].closed {
            return Err(Error::InvalidFd);
        }
        self.open_files[fd_index].closed = true;
        Ok(())
    }

    fn write(&mut self, fd_index: usize, data: &[u8]) -> Result<usize, Error> {
        if fd_index >= self.open_files.len() || self.open_files[fd_index].closed {
            return Err(Error::InvalidFd);
        }
        let open_file = &self.open_files[fd_index];
        let name = open_file.name.clone();
        let pos = open_file.position;

        if pos + data.len() > self.max_file_size {
            return Err(Error::FileTooLarge);
        }

        let file = self.files.get_mut(&name).ok_or(Error::NotFound)?;
        file.write(pos, data);
        self.open_files[fd_index].position = pos + data.len();
        Ok(data.len())
    }

    fn read(&mut self, fd_index: usize, len: usize) -> Result<Vec<u8>, Error> {
        if fd_index >= self.open_files.len() || self.open_files[fd_index].closed {
            return Err(Error::InvalidFd);
        }
        let open_file = &self.open_files[fd_index];
        let name = open_file.name.clone();
        let pos = open_file.position;

        let file = self.files.get(&name).ok_or(Error::NotFound)?;
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
        let name = self.open_files[fd_index].name.clone();

        let file = self.files.get_mut(&name).ok_or(Error::NotFound)?;
        file.truncate(size as usize);
        Ok(())
    }
}

/////////////////////////////////////////// LfsAdapter /////////////////////////////////////////////////

/// Adapter to track open file descriptors for LFS.
struct LfsAdapter {
    lfs: Lfs<MemoryBlockDevice>,
    open_fds: Vec<Option<FileDescriptor>>,
}

impl LfsAdapter {
    fn new(lfs: Lfs<MemoryBlockDevice>) -> Self {
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

    fn tail(&self) -> BlockAddress {
        self.lfs.tail()
    }

    fn into_inner(self) -> Vec<u8> {
        self.lfs.into_inner()
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
    }
}

/// Run a sequence of operations on both implementations.
fn run_ops(ops: &[FsOp]) -> (LfsAdapter, ReferenceFs) {
    let data = vec![0u8; TEST_FS_BLOCKS * BLOCK_SIZE];
    let lfs = Lfs::from_vec(data).expect("Failed to create LFS");
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
        let tail = lfs.tail();
        let data = lfs.into_inner();
        let mut lfs_verify = Lfs::open_vec(data, tail).expect("Failed to reopen LFS for verification");

        // Verify each file's contents match the reference
        for (name, ref_file) in &reference.files {
            let fd = lfs_verify.open_file(name).expect("Failed to open file for verification");
            lfs_verify.seek(fd, 0).expect("Failed to seek");

            let mut buf = vec![0u8; ref_file.size()];
            let n = lfs_verify.read(fd, &mut buf).expect("Failed to read");

            prop_assert_eq!(n, ref_file.size(), "Size mismatch for {}", name);
            prop_assert_eq!(&buf[..n], &ref_file.data[..], "Data mismatch for {}", name);

            lfs_verify.close(fd).expect("Failed to close");
        }
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

        let tail = lfs.tail();
        let data = lfs.into_inner();

        let mut restored_lfs = Lfs::open_vec(data, tail).expect("Failed to restore LFS");

        for (name, ref_file) in &reference.files {
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

    let ref_file = reference.files.get("test.txt").expect("File should exist");
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
        reference.files.get("a.txt").unwrap().data,
        b"AAAaaa".to_vec()
    );
    assert_eq!(
        reference.files.get("b.txt").unwrap().data,
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
        reference.files.get("trunc.txt").unwrap().data,
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

    let ref_file = reference.files.get("sparse.txt").unwrap();
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

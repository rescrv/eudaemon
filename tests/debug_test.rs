//! Debug tests to investigate determinism issues.

use std::collections::BTreeMap;

use synfs::BlockAddress;
use synfs::Error;
use synfs::FileDescriptor;
use synfs::Lfs;
use synfs::MemoryBlockDevice;

const BLOCK_SIZE: usize = 4096;
const TEST_FS_BLOCKS: usize = 256;

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

/// An open file handle in the reference implementation.
#[derive(Debug, Clone)]
struct ReferenceOpenFile {
    name: String,
    position: usize,
    closed: bool,
}

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

#[test]
fn regression_test_big_write() {
    // This tests large writes with two open fds pointing to same file
    let max_file_size = TEST_FS_BLOCKS * BLOCK_SIZE / 10;

    let data = vec![0u8; TEST_FS_BLOCKS * BLOCK_SIZE];
    let lfs = Lfs::from_vec(data).expect("Failed to create LFS");
    let mut lfs_adapter = LfsAdapter::new(lfs);
    let mut reference = ReferenceFs::new(max_file_size);

    // Open file twice
    let _ = lfs_adapter.open("a.txt");
    let _ = reference.open("a.txt");

    let _ = lfs_adapter.open("a.txt");
    let _ = reference.open("a.txt");

    // Write large data through fd_index 1
    let write_data: Vec<u8> = (0..500).map(|i| (i % 256) as u8).collect();
    let lfs_write = lfs_adapter.write(1, &write_data);
    let ref_write = reference.write(1, &write_data);
    println!(
        "Write result: lfs={:?}, ref={:?}",
        lfs_write.as_ref().map(|n| *n),
        ref_write.as_ref().map(|n| *n)
    );

    // Read through fd_index 0 after seeking
    let _ = lfs_adapter.seek(0, 0);
    let _ = reference.seek(0, 0);

    let lfs_read = lfs_adapter.read(0, 500);
    let ref_read = reference.read(0, 500);

    match (&lfs_read, &ref_read) {
        (Ok(lfs_data), Ok(ref_data)) => {
            println!(
                "Read length: lfs={}, ref={}",
                lfs_data.len(),
                ref_data.len()
            );
            if lfs_data != ref_data {
                println!("MISMATCH!");
                println!("First 10 lfs: {:?}", &lfs_data[..10.min(lfs_data.len())]);
                println!("First 10 ref: {:?}", &ref_data[..10.min(ref_data.len())]);
            }
            assert_eq!(lfs_data, ref_data, "Data should match");
        }
        _ => {
            println!("Read error: lfs={:?}, ref={:?}", lfs_read, ref_read);
        }
    }

    // Now persist and restore
    let tail = lfs_adapter.tail();
    let disk_data = lfs_adapter.into_inner();

    let mut lfs2 = Lfs::open_vec(disk_data, tail).expect("Reopen LFS");
    let fd = lfs2.open_file("a.txt").expect("Open after restore");
    lfs2.seek(fd, 0).expect("Seek after restore");

    let mut buf = vec![0u8; 500];
    let n = lfs2.read(fd, &mut buf).expect("Read after restore");
    println!("After restore: read {} bytes", n);

    // Compare with reference
    let ref_file = reference.files.get("a.txt").unwrap();
    println!("Reference file size: {}", ref_file.size());

    if buf[..n] != ref_file.data[..] {
        println!("After restore MISMATCH!");
        println!("Read: {:?}", &buf[..10.min(n)]);
        println!("Ref:  {:?}", &ref_file.data[..10.min(ref_file.size())]);
    }

    assert_eq!(n, ref_file.size(), "Size should match");
    assert_eq!(
        &buf[..n],
        &ref_file.data[..],
        "Data should match after restore"
    );
}

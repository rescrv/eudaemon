//! Eudaemonfs implementation of the Filesystem trait.

use std::sync::Arc;
use std::sync::Mutex;

use eudaemonfs::DeviceId;
use eudaemonfs::Lfs;
use eudaemonfs::MemoryBlockDevice;

use crate::Error;

use super::DirEntry;
use super::FileMetadata;
use super::FileType;
use super::Filesystem;
use super::TimeSpec;

/// A filesystem backed by eudaemonfs.
#[derive(Clone)]
pub struct EudaemonFilesystem<T: Fn() -> i64 + Clone + Send + 'static> {
    inner: Arc<Mutex<Lfs<MemoryBlockDevice, T>>>,
}

impl<T: Fn() -> i64 + Clone + Send + 'static> EudaemonFilesystem<T> {
    /// Creates a new eudaemonfs with the given size in bytes.
    ///
    /// # Errors
    ///
    /// Returns an error if the size is too small (less than 64KB).
    pub fn new(size: usize, dev: DeviceId, time_source: T) -> Result<Self, Error> {
        let data = vec![0u8; size];
        let lfs = Lfs::from_vec(data, dev, time_source).map_err(|e| Error::Io(e.into()))?;
        Ok(Self {
            inner: Arc::new(Mutex::new(lfs)),
        })
    }

    /// Opens an existing eudaemonfs from a byte buffer.
    ///
    /// # Errors
    ///
    /// Returns an error if the buffer does not contain a valid filesystem.
    pub fn open(data: Vec<u8>, dev: DeviceId, time_source: T) -> Result<Self, Error> {
        let lfs = Lfs::open_vec(data, dev, time_source).map_err(|e| Error::Io(e.into()))?;
        Ok(Self {
            inner: Arc::new(Mutex::new(lfs)),
        })
    }

    /// Consumes the filesystem and returns the underlying data buffer.
    pub fn into_inner(self) -> Result<Vec<u8>, Error> {
        let lfs = Arc::try_unwrap(self.inner)
            .map_err(|_| Error::Io(std::io::Error::other("filesystem has multiple references")))?
            .into_inner()
            .map_err(|_| Error::Io(std::io::Error::other("filesystem mutex poisoned")))?;
        Ok(lfs.into_inner())
    }

    /// Returns the number of free blocks available.
    pub fn free_blocks(&self) -> u64 {
        self.inner.lock().unwrap().free_blocks()
    }

    /// Returns the current usage percentage of the filesystem (0-100).
    pub fn usage_percent(&self) -> u64 {
        self.inner.lock().unwrap().usage_percent()
    }

    /// Performs log cleaning (garbage collection).
    ///
    /// Returns the number of blocks reclaimed.
    pub fn clean(&self) -> Result<usize, Error> {
        self.inner
            .lock()
            .unwrap()
            .clean()
            .map_err(|e| Error::Io(e.into()))
    }
}

/// Converts an eudaemonfs FileType to our FileType.
fn convert_file_type(ft: eudaemonfs::FileType) -> FileType {
    match ft {
        eudaemonfs::FileType::RegularFile => FileType::RegularFile,
        eudaemonfs::FileType::Directory => FileType::Directory,
        eudaemonfs::FileType::Symlink => FileType::Symlink,
        eudaemonfs::FileType::Other => FileType::Other,
    }
}

/// Converts our TimeSpec to eudaemonfs TimeSpec.
fn convert_timespec(ts: TimeSpec) -> eudaemonfs::TimeSpec {
    match ts {
        TimeSpec::Now => eudaemonfs::TimeSpec::Now,
        TimeSpec::Omit => eudaemonfs::TimeSpec::Omit,
        TimeSpec::Time(t) => eudaemonfs::TimeSpec::Time(t),
    }
}

impl<T: Fn() -> i64 + Clone + Send + 'static> Filesystem for EudaemonFilesystem<T> {
    fn dup(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }

    fn read_to_string(&self, path: &str) -> Result<String, Error> {
        let mut lfs = self.inner.lock().unwrap();
        let bytes = lfs.read_file(path).map_err(|e| Error::Io(e.into()))?;
        String::from_utf8(bytes).map_err(|e| {
            Error::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                e.to_string(),
            ))
        })
    }

    fn exists(&self, path: &str) -> bool {
        self.inner.lock().unwrap().exists(path)
    }

    fn metadata(&self, path: &str) -> Result<FileMetadata, Error> {
        let lfs = self.inner.lock().unwrap();
        let stat = lfs.stat(path).map_err(|e| Error::Io(e.into()))?;
        Ok(FileMetadata { size: stat.size })
    }

    fn truncate(&self, path: &str, size: u64) -> Result<(), Error> {
        let mut lfs = self.inner.lock().unwrap();
        lfs.truncate_path(path, size)
            .map_err(|e| Error::Io(e.into()))
    }

    fn truncate_existing(&self, path: &str, size: u64) -> Result<bool, Error> {
        let mut lfs = self.inner.lock().unwrap();
        lfs.truncate_existing(path, size)
            .map_err(|e| Error::Io(e.into()))
    }

    fn punch_hole(&self, path: &str, offset: u64, length: u64) -> Result<(), Error> {
        let mut lfs = self.inner.lock().unwrap();
        lfs.punch_hole(path, offset, length)
            .map_err(|e| Error::Io(e.into()))
    }

    fn write_string(&self, path: &str, contents: &str) -> Result<(), Error> {
        let mut lfs = self.inner.lock().unwrap();
        lfs.write_file(path, contents.as_bytes())
            .map_err(|e| Error::Io(e.into()))
    }

    fn append_string(&self, path: &str, contents: &str) -> Result<(), Error> {
        let mut lfs = self.inner.lock().unwrap();
        lfs.append_file(path, contents.as_bytes())
            .map_err(|e| Error::Io(e.into()))
    }

    fn mkdir(&self, path: &str) -> Result<(), Error> {
        let mut lfs = self.inner.lock().unwrap();
        lfs.mkdir(path).map_err(|e| Error::Io(e.into()))
    }

    fn mkdir_all(&self, path: &str) -> Result<(), Error> {
        let mut lfs = self.inner.lock().unwrap();
        lfs.mkdir_all(path).map_err(|e| Error::Io(e.into()))
    }

    fn is_dir(&self, path: &str) -> bool {
        self.inner.lock().unwrap().is_dir(path)
    }

    fn read_dir(&self, path: &str) -> Result<Vec<(String, DirEntry)>, Error> {
        let lfs = self.inner.lock().unwrap();
        let entries = lfs.read_dir(path).map_err(|e| Error::Io(e.into()))?;
        Ok(entries
            .into_iter()
            .map(|(name, stat)| {
                (
                    name,
                    DirEntry {
                        file_type: convert_file_type(stat.file_type),
                        size: stat.size,
                        atime_ms: stat.atime_ms,
                        mtime_ms: stat.mtime_ms,
                        dev: stat.dev,
                        ino: stat.ino,
                    },
                )
            })
            .collect())
    }

    fn stat(&self, path: &str) -> Result<DirEntry, Error> {
        let lfs = self.inner.lock().unwrap();
        let stat = lfs.stat(path).map_err(|e| Error::Io(e.into()))?;
        Ok(DirEntry {
            file_type: convert_file_type(stat.file_type),
            size: stat.size,
            atime_ms: stat.atime_ms,
            mtime_ms: stat.mtime_ms,
            dev: stat.dev,
            ino: stat.ino,
        })
    }

    fn lstat(&self, path: &str) -> Result<DirEntry, Error> {
        let lfs = self.inner.lock().unwrap();
        let stat = lfs.lstat(path).map_err(|e| Error::Io(e.into()))?;
        Ok(DirEntry {
            file_type: convert_file_type(stat.file_type),
            size: stat.size,
            atime_ms: stat.atime_ms,
            mtime_ms: stat.mtime_ms,
            dev: stat.dev,
            ino: stat.ino,
        })
    }

    fn symlink(&self, target: &str, linkpath: &str) -> Result<(), Error> {
        let mut lfs = self.inner.lock().unwrap();
        lfs.symlink(target, linkpath)
            .map_err(|e| Error::Io(e.into()))
    }

    fn link(&self, src: &str, dst: &str) -> Result<(), Error> {
        let mut lfs = self.inner.lock().unwrap();
        lfs.link(src, dst).map_err(|e| Error::Io(e.into()))
    }

    fn unlink(&self, path: &str) -> Result<(), Error> {
        let mut lfs = self.inner.lock().unwrap();
        lfs.remove(path).map_err(|e| Error::Io(e.into()))
    }

    fn rmdir(&self, path: &str) -> Result<(), Error> {
        let mut lfs = self.inner.lock().unwrap();
        lfs.rmdir(path).map_err(|e| Error::Io(e.into()))
    }

    fn readlink(&self, path: &str) -> Result<String, Error> {
        let lfs = self.inner.lock().unwrap();
        lfs.readlink(path).map_err(|e| Error::Io(e.into()))
    }

    fn set_times(&self, path: &str, atime: TimeSpec, mtime: TimeSpec) -> Result<(), Error> {
        let mut lfs = self.inner.lock().unwrap();
        lfs.set_times(path, convert_timespec(atime), convert_timespec(mtime))
            .map_err(|e| Error::Io(e.into()))
    }

    fn lset_times(&self, path: &str, atime: TimeSpec, mtime: TimeSpec) -> Result<(), Error> {
        let mut lfs = self.inner.lock().unwrap();
        lfs.lset_times(path, convert_timespec(atime), convert_timespec(mtime))
            .map_err(|e| Error::Io(e.into()))
    }

    fn create_file(&self, path: &str) -> Result<bool, Error> {
        let mut lfs = self.inner.lock().unwrap();
        lfs.create_file(path).map_err(|e| Error::Io(e.into()))
    }

    fn rename(&self, src: &str, dst: &str) -> Result<(), Error> {
        let mut lfs = self.inner.lock().unwrap();
        lfs.rename(src, dst).map_err(|e| Error::Io(e.into()))
    }

    fn mkstemp(&self, template: &str) -> Result<String, Error> {
        use std::time::SystemTime;
        use std::time::UNIX_EPOCH;

        let chars: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";

        let seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0)
            ^ (std::process::id() as u64);

        let mut state = seed;

        for attempt in 0..100u64 {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(attempt);

            let mut path = String::new();
            let mut s = state;
            for c in template.chars() {
                if c == 'X' {
                    path.push(chars[(s % 62) as usize] as char);
                    s = s.wrapping_mul(6364136223846793005).wrapping_add(1);
                } else {
                    path.push(c);
                }
            }

            let mut lfs = self.inner.lock().unwrap();
            match lfs.create_file(&path) {
                Ok(true) => return Ok(path),
                Ok(false) => continue,
                Err(eudaemonfs::Error::AlreadyExists) => continue,
                Err(e) => return Err(Error::Io(e.into())),
            }
        }

        Err(Error::Io(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "could not create unique temporary file",
        )))
    }

    fn mkdtemp(&self, template: &str) -> Result<String, Error> {
        use std::time::SystemTime;
        use std::time::UNIX_EPOCH;

        let chars: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";

        let seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0)
            ^ (std::process::id() as u64);

        let mut state = seed;

        for attempt in 0..100u64 {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(attempt);

            let mut path = String::new();
            let mut s = state;
            for c in template.chars() {
                if c == 'X' {
                    path.push(chars[(s % 62) as usize] as char);
                    s = s.wrapping_mul(6364136223846793005).wrapping_add(1);
                } else {
                    path.push(c);
                }
            }

            let mut lfs = self.inner.lock().unwrap();
            match lfs.mkdir(&path) {
                Ok(()) => return Ok(path),
                Err(eudaemonfs::Error::AlreadyExists) => continue,
                Err(e) => return Err(Error::Io(e.into())),
            }
        }

        Err(Error::Io(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "could not create unique temporary directory",
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn zero_time() -> i64 {
        0
    }

    fn create_test_fs() -> EudaemonFilesystem<fn() -> i64> {
        EudaemonFilesystem::new(256 * 4096, DeviceId::new(1), zero_time as fn() -> i64)
            .expect("Failed to create filesystem")
    }

    #[test]
    fn basic_file_operations() {
        let fs = create_test_fs();

        fs.write_string("/test.txt", "Hello, World!")
            .expect("Failed to write");

        assert!(fs.exists("/test.txt"));

        let content = fs.read_to_string("/test.txt").expect("Failed to read");
        assert_eq!(content, "Hello, World!");
        println!("Read content: {}", content);
    }

    #[test]
    fn directory_operations() {
        let fs = create_test_fs();

        fs.mkdir("/testdir").expect("Failed to mkdir");
        assert!(fs.is_dir("/testdir"));

        fs.write_string("/testdir/file.txt", "content")
            .expect("Failed to write");

        let entries = fs.read_dir("/testdir").expect("Failed to read_dir");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].0, "file.txt");
        println!("Directory entries: {:?}", entries);
    }

    #[test]
    fn symlink_operations() {
        let fs = create_test_fs();

        fs.write_string("/target.txt", "target content")
            .expect("Failed to write target");
        fs.symlink("/target.txt", "/link.txt")
            .expect("Failed to create symlink");

        let target = fs.readlink("/link.txt").expect("Failed to readlink");
        assert_eq!(target, "/target.txt");
        println!("Symlink target: {}", target);

        let content = fs
            .read_to_string("/link.txt")
            .expect("Failed to read via symlink");
        assert_eq!(content, "target content");
        println!("Content via symlink: {}", content);
    }

    #[test]
    fn truncate_operations() {
        let fs = create_test_fs();

        fs.write_string("/truncate.txt", "Hello, World!")
            .expect("Failed to write");

        fs.truncate("/truncate.txt", 5).expect("Failed to truncate");

        let content = fs.read_to_string("/truncate.txt").expect("Failed to read");
        assert_eq!(content, "Hello");
        println!("Truncated content: {}", content);
    }

    #[test]
    fn dup_shares_state() {
        let fs1 = create_test_fs();
        let fs2 = fs1.dup();

        fs1.write_string("/shared.txt", "shared data")
            .expect("Failed to write");

        let content = fs2
            .read_to_string("/shared.txt")
            .expect("Failed to read from dup");
        assert_eq!(content, "shared data");
        println!("Dup shares state correctly");
    }

    #[test]
    fn mkstemp_creates_unique_files() {
        let fs = create_test_fs();

        let path1 = fs.mkstemp("/tmp.XXXXXX").expect("Failed to mkstemp");
        let path2 = fs.mkstemp("/tmp.XXXXXX").expect("Failed to mkstemp");

        assert_ne!(path1, path2);
        assert!(fs.exists(&path1));
        assert!(fs.exists(&path2));
        println!("Created temp files: {} and {}", path1, path2);
    }

    #[test]
    fn mkdtemp_creates_unique_directories() {
        let fs = create_test_fs();

        let path1 = fs.mkdtemp("/tmpdir.XXXXXX").expect("Failed to mkdtemp");
        let path2 = fs.mkdtemp("/tmpdir.XXXXXX").expect("Failed to mkdtemp");

        assert_ne!(path1, path2);
        assert!(fs.is_dir(&path1));
        assert!(fs.is_dir(&path2));
        println!("Created temp dirs: {} and {}", path1, path2);
    }
}

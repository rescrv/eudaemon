use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;

use crate::Error;

/// Metadata about a file.
#[derive(Clone, Copy, Debug)]
pub struct FileMetadata {
    /// The size of the file in bytes.
    pub size: u64,
}

/// The type of a file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileType {
    /// A regular file.
    RegularFile,
    /// A directory.
    Directory,
    /// A symbolic link.
    Symlink,
    /// Some other type of file.
    Other,
}

/// A directory entry with metadata.
#[derive(Clone, Debug)]
pub struct DirEntry {
    /// The type of the file.
    pub file_type: FileType,
    /// The size of the file in bytes.
    pub size: u64,
    /// The access time in milliseconds since UNIX epoch.
    pub atime_ms: i64,
    /// The modification time in milliseconds since UNIX epoch.
    pub mtime_ms: i64,
    /// The device ID containing this file.
    pub dev: u64,
    /// The inode number.
    pub ino: u64,
}

/// Timestamp specification for setting file times.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimeSpec {
    /// Set to the current time.
    Now,
    /// Leave the time unchanged.
    Omit,
    /// Set to a specific time in milliseconds since UNIX epoch.
    Time(i64),
}

/// A trait for filesystem operations.
pub trait Filesystem {
    /// Duplicate the filesystem handle.
    fn dup(&self) -> Self;
    /// Read a file and return its contents as a string.
    fn read_to_string(&self, path: &str) -> Result<String, Error>;
    /// Check if a file exists.
    fn exists(&self, path: &str) -> bool;
    /// Get metadata about a file.
    fn metadata(&self, path: &str) -> Result<FileMetadata, Error>;
    /// Truncate or extend a file to the specified size.
    /// Creates the file if it does not exist.
    fn truncate(&self, path: &str, size: u64) -> Result<(), Error>;
    /// Truncate or extend a file to the specified size, but only if it exists.
    /// Returns Ok(false) if the file does not exist, Ok(true) if successful.
    fn truncate_existing(&self, path: &str, size: u64) -> Result<bool, Error>;
    /// Punch a hole in a file by writing spaces at the given offset for the given length.
    /// The file must exist. If offset + length exceeds file size, extends the file.
    fn punch_hole(&self, path: &str, offset: u64, length: u64) -> Result<(), Error>;
    /// Write a string to a file, creating or overwriting as needed.
    fn write_string(&self, path: &str, contents: &str) -> Result<(), Error>;
    /// Append a string to a file, creating if it does not exist.
    fn append_string(&self, path: &str, contents: &str) -> Result<(), Error>;
    /// Create a directory. Returns an error if the directory already exists
    /// or if the parent directory does not exist.
    fn mkdir(&self, path: &str) -> Result<(), Error>;
    /// Create a directory and all parent directories as needed.
    /// Returns Ok(()) if the directory already exists.
    fn mkdir_all(&self, path: &str) -> Result<(), Error>;
    /// Check if a path is a directory.
    fn is_dir(&self, path: &str) -> bool;
    /// Read the contents of a directory.
    fn read_dir(&self, path: &str) -> Result<Vec<(String, DirEntry)>, Error>;
    /// Get detailed information about a file or directory.
    fn stat(&self, path: &str) -> Result<DirEntry, Error>;
    /// Get detailed information about a file or directory without following symlinks.
    fn lstat(&self, path: &str) -> Result<DirEntry, Error>;
    /// Create a symbolic link at linkpath pointing to target.
    fn symlink(&self, target: &str, linkpath: &str) -> Result<(), Error>;
    /// Create a hard link at dst pointing to src.
    fn link(&self, src: &str, dst: &str) -> Result<(), Error>;
    /// Remove a file or symbolic link.
    fn unlink(&self, path: &str) -> Result<(), Error>;
    /// Remove an empty directory.
    fn rmdir(&self, path: &str) -> Result<(), Error>;
    /// Read the target of a symbolic link.
    fn readlink(&self, path: &str) -> Result<String, Error>;
    /// Set the access and modification times of a file.
    /// TimeSpec::Now sets to current time, TimeSpec::Omit leaves unchanged.
    fn set_times(&self, path: &str, atime: TimeSpec, mtime: TimeSpec) -> Result<(), Error>;
    /// Set the access and modification times of a file without following symlinks.
    fn lset_times(&self, path: &str, atime: TimeSpec, mtime: TimeSpec) -> Result<(), Error>;
    /// Create an empty file if it does not exist, without changing times if it does.
    /// Returns true if the file was created, false if it already existed.
    fn create_file(&self, path: &str) -> Result<bool, Error>;
    /// Rename a file or directory from src to dst.
    fn rename(&self, src: &str, dst: &str) -> Result<(), Error>;
    /// Create a unique temporary file using a template (Xs are replaced).
    /// Returns the actual path created.
    fn mkstemp(&self, template: &str) -> Result<String, Error>;
    /// Create a unique temporary directory using a template (Xs are replaced).
    /// Returns the actual path created.
    fn mkdtemp(&self, template: &str) -> Result<String, Error>;
}

/// A real filesystem that reads from disk.
#[derive(Clone, Copy)]
pub struct RealFilesystem;

/// Extract device and inode from metadata on Unix systems.
#[cfg(unix)]
fn dev_ino_from_metadata(metadata: &std::fs::Metadata) -> (u64, u64) {
    use std::os::unix::fs::MetadataExt;
    (metadata.dev(), metadata.ino())
}

/// Return placeholder values on non-Unix systems.
#[cfg(not(unix))]
fn dev_ino_from_metadata(_metadata: &std::fs::Metadata) -> (u64, u64) {
    (0, 0)
}

impl Filesystem for RealFilesystem {
    fn dup(&self) -> Self {
        *self
    }

    fn read_to_string(&self, path: &str) -> Result<String, Error> {
        std::fs::read_to_string(path).map_err(Error::Io)
    }

    fn exists(&self, path: &str) -> bool {
        std::path::Path::new(path).exists()
    }

    fn metadata(&self, path: &str) -> Result<FileMetadata, Error> {
        let meta = std::fs::metadata(path).map_err(Error::Io)?;
        Ok(FileMetadata { size: meta.len() })
    }

    fn truncate(&self, path: &str, size: u64) -> Result<(), Error> {
        use std::fs::OpenOptions;
        let file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)
            .map_err(Error::Io)?;
        file.set_len(size).map_err(Error::Io)
    }

    fn truncate_existing(&self, path: &str, size: u64) -> Result<bool, Error> {
        use std::fs::OpenOptions;
        match OpenOptions::new().write(true).truncate(false).open(path) {
            Ok(file) => {
                file.set_len(size).map_err(Error::Io)?;
                Ok(true)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(Error::Io(e)),
        }
    }

    fn punch_hole(&self, path: &str, offset: u64, length: u64) -> Result<(), Error> {
        use std::fs::OpenOptions;
        use std::io::Seek;
        use std::io::SeekFrom;
        use std::io::Write;

        let mut file = OpenOptions::new()
            .write(true)
            .read(true)
            .open(path)
            .map_err(Error::Io)?;

        let file_len = file.metadata().map_err(Error::Io)?.len();
        let end = offset.saturating_add(length);

        // Extend file if necessary
        if end > file_len {
            file.set_len(end).map_err(Error::Io)?;
        }

        // Seek to offset and write NUL bytes
        file.seek(SeekFrom::Start(offset)).map_err(Error::Io)?;
        let nulls = vec![b'\0'; length as usize];
        file.write_all(&nulls).map_err(Error::Io)?;

        Ok(())
    }

    fn write_string(&self, path: &str, contents: &str) -> Result<(), Error> {
        std::fs::write(path, contents).map_err(Error::Io)
    }

    fn append_string(&self, path: &str, contents: &str) -> Result<(), Error> {
        use std::fs::OpenOptions;
        use std::io::Write;

        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .map_err(Error::Io)?;
        file.write_all(contents.as_bytes()).map_err(Error::Io)
    }

    fn mkdir(&self, path: &str) -> Result<(), Error> {
        std::fs::create_dir(path).map_err(Error::Io)
    }

    fn mkdir_all(&self, path: &str) -> Result<(), Error> {
        std::fs::create_dir_all(path).map_err(Error::Io)
    }

    fn is_dir(&self, path: &str) -> bool {
        std::path::Path::new(path).is_dir()
    }

    fn read_dir(&self, path: &str) -> Result<Vec<(String, DirEntry)>, Error> {
        let mut entries = Vec::new();
        for entry in std::fs::read_dir(path).map_err(Error::Io)? {
            let entry = entry.map_err(Error::Io)?;
            let name = entry.file_name().to_string_lossy().to_string();
            let metadata = entry.metadata().map_err(Error::Io)?;
            let file_type = if metadata.is_dir() {
                FileType::Directory
            } else if metadata.is_symlink() {
                FileType::Symlink
            } else if metadata.is_file() {
                FileType::RegularFile
            } else {
                FileType::Other
            };
            let (dev, ino) = dev_ino_from_metadata(&metadata);
            let dir_entry = DirEntry {
                file_type,
                size: metadata.len(),
                atime_ms: metadata
                    .accessed()
                    .map(|t| {
                        t.duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_millis() as i64)
                            .unwrap_or(0)
                    })
                    .unwrap_or(0),
                mtime_ms: metadata
                    .modified()
                    .map(|t| {
                        t.duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_millis() as i64)
                            .unwrap_or(0)
                    })
                    .unwrap_or(0),
                dev,
                ino,
            };
            entries.push((name, dir_entry));
        }
        Ok(entries)
    }

    fn stat(&self, path: &str) -> Result<DirEntry, Error> {
        let metadata = std::fs::metadata(path).map_err(Error::Io)?;
        let file_type = if metadata.is_dir() {
            FileType::Directory
        } else if metadata.is_symlink() {
            FileType::Symlink
        } else if metadata.is_file() {
            FileType::RegularFile
        } else {
            FileType::Other
        };
        let (dev, ino) = dev_ino_from_metadata(&metadata);
        Ok(DirEntry {
            file_type,
            size: metadata.len(),
            atime_ms: metadata
                .accessed()
                .map(|t| {
                    t.duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_millis() as i64)
                        .unwrap_or(0)
                })
                .unwrap_or(0),
            mtime_ms: metadata
                .modified()
                .map(|t| {
                    t.duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_millis() as i64)
                        .unwrap_or(0)
                })
                .unwrap_or(0),
            dev,
            ino,
        })
    }

    fn lstat(&self, path: &str) -> Result<DirEntry, Error> {
        let metadata = std::fs::symlink_metadata(path).map_err(Error::Io)?;
        let file_type = if metadata.is_symlink() {
            FileType::Symlink
        } else if metadata.is_dir() {
            FileType::Directory
        } else if metadata.is_file() {
            FileType::RegularFile
        } else {
            FileType::Other
        };
        let (dev, ino) = dev_ino_from_metadata(&metadata);
        Ok(DirEntry {
            file_type,
            size: metadata.len(),
            atime_ms: metadata
                .accessed()
                .map(|t| {
                    t.duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_millis() as i64)
                        .unwrap_or(0)
                })
                .unwrap_or(0),
            mtime_ms: metadata
                .modified()
                .map(|t| {
                    t.duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_millis() as i64)
                        .unwrap_or(0)
                })
                .unwrap_or(0),
            dev,
            ino,
        })
    }

    fn symlink(&self, target: &str, linkpath: &str) -> Result<(), Error> {
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(target, linkpath).map_err(Error::Io)
        }
        #[cfg(not(unix))]
        {
            Err(Error::Io(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "symlinks not supported on this platform",
            )))
        }
    }

    fn link(&self, src: &str, dst: &str) -> Result<(), Error> {
        std::fs::hard_link(src, dst).map_err(Error::Io)
    }

    fn unlink(&self, path: &str) -> Result<(), Error> {
        std::fs::remove_file(path).map_err(Error::Io)
    }

    fn rmdir(&self, path: &str) -> Result<(), Error> {
        std::fs::remove_dir(path).map_err(Error::Io)
    }

    fn readlink(&self, path: &str) -> Result<String, Error> {
        std::fs::read_link(path)
            .map_err(Error::Io)?
            .to_str()
            .map(|s| s.to_string())
            .ok_or_else(|| {
                Error::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "symlink target is not valid UTF-8",
                ))
            })
    }

    fn set_times(&self, path: &str, atime: TimeSpec, mtime: TimeSpec) -> Result<(), Error> {
        use std::fs::FileTimes;
        use std::time::Duration;
        use std::time::SystemTime;
        use std::time::UNIX_EPOCH;

        let file = std::fs::File::options()
            .write(true)
            .open(path)
            .map_err(Error::Io)?;

        let metadata = file.metadata().map_err(Error::Io)?;
        let mut times = FileTimes::new();

        let atime_val = match atime {
            TimeSpec::Now => SystemTime::now(),
            TimeSpec::Omit => metadata.accessed().map_err(Error::Io)?,
            TimeSpec::Time(ms) => {
                if ms >= 0 {
                    UNIX_EPOCH + Duration::from_millis(ms as u64)
                } else {
                    UNIX_EPOCH - Duration::from_millis((-ms) as u64)
                }
            }
        };
        times = times.set_accessed(atime_val);

        let mtime_val = match mtime {
            TimeSpec::Now => SystemTime::now(),
            TimeSpec::Omit => metadata.modified().map_err(Error::Io)?,
            TimeSpec::Time(ms) => {
                if ms >= 0 {
                    UNIX_EPOCH + Duration::from_millis(ms as u64)
                } else {
                    UNIX_EPOCH - Duration::from_millis((-ms) as u64)
                }
            }
        };
        times = times.set_modified(mtime_val);

        file.set_times(times).map_err(Error::Io)
    }

    fn lset_times(&self, path: &str, atime: TimeSpec, mtime: TimeSpec) -> Result<(), Error> {
        // For symlinks on the real filesystem, we cannot easily set times without libc.
        // The -h flag implies -c (no create), so if we can't modify symlink times,
        // we silently succeed for symlinks (matching BSD behavior when unsupported).
        let metadata = std::fs::symlink_metadata(path).map_err(Error::Io)?;
        if metadata.is_symlink() {
            // Cannot set times on symlinks without platform-specific APIs.
            // Return Ok to match the -h flag behavior (silently skip).
            return Ok(());
        }
        // For non-symlinks, fall back to regular set_times
        self.set_times(path, atime, mtime)
    }

    fn create_file(&self, path: &str) -> Result<bool, Error> {
        use std::fs::OpenOptions;
        match OpenOptions::new().write(true).create_new(true).open(path) {
            Ok(_) => Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
            Err(e) => Err(Error::Io(e)),
        }
    }

    fn rename(&self, src: &str, dst: &str) -> Result<(), Error> {
        std::fs::rename(src, dst).map_err(Error::Io)
    }

    fn mkstemp(&self, template: &str) -> Result<String, Error> {
        use std::fs::OpenOptions;
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

            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(_) => return Ok(path),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(Error::Io(e)),
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

            match std::fs::create_dir(&path) {
                Ok(()) => return Ok(path),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(Error::Io(e)),
            }
        }

        Err(Error::Io(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "could not create unique temporary directory",
        )))
    }
}

/// Timestamps for mock filesystem entries.
#[derive(Clone, Debug, Default)]
struct MockTimes {
    /// Access time in milliseconds since UNIX epoch.
    atime_ms: i64,
    /// Modification time in milliseconds since UNIX epoch.
    mtime_ms: i64,
    /// Inode number.
    ino: u64,
}

/// Internal entry for the mock filesystem.
#[derive(Clone, Debug)]
enum MockEntry {
    /// A file with contents and timestamps.
    File(String, MockTimes),
    /// A directory with timestamps.
    Directory(MockTimes),
    /// A symbolic link with target path and timestamps.
    Symlink(String, MockTimes),
}

/// Internal state for the mock filesystem.
#[derive(Clone, Debug)]
struct MockFilesystemInner {
    entries: HashMap<String, MockEntry>,
    next_ino: u64,
}

/// A mock filesystem backed by a HashMap.
#[derive(Clone)]
pub struct MockFilesystem(Arc<Mutex<MockFilesystemInner>>);

impl MockFilesystem {
    /// Create a new empty MockFilesystem.
    pub fn new() -> Self {
        Self(Arc::new(Mutex::new(MockFilesystemInner {
            entries: HashMap::new(),
            next_ino: 1,
        })))
    }

    /// Allocate a new inode number.
    fn alloc_ino(&self) -> u64 {
        let mut inner = self.0.lock().unwrap();
        let ino = inner.next_ino;
        inner.next_ino += 1;
        ino
    }

    /// Add a file to the mock filesystem.
    pub fn add_file(&self, path: &str, contents: &str) {
        let ino = self.alloc_ino();
        self.0.lock().unwrap().entries.insert(
            path.to_string(),
            MockEntry::File(
                contents.to_string(),
                MockTimes {
                    ino,
                    ..Default::default()
                },
            ),
        );
    }

    /// Add a file to the mock filesystem with specific timestamps.
    pub fn add_file_with_times(&self, path: &str, contents: &str, atime_ms: i64, mtime_ms: i64) {
        let ino = self.alloc_ino();
        self.0.lock().unwrap().entries.insert(
            path.to_string(),
            MockEntry::File(
                contents.to_string(),
                MockTimes {
                    atime_ms,
                    mtime_ms,
                    ino,
                },
            ),
        );
    }

    /// Add a directory to the mock filesystem.
    pub fn add_directory(&self, path: &str) {
        let ino = self.alloc_ino();
        self.0.lock().unwrap().entries.insert(
            path.to_string(),
            MockEntry::Directory(MockTimes {
                ino,
                ..Default::default()
            }),
        );
    }

    /// Add a symbolic link to the mock filesystem.
    pub fn add_symlink(&self, linkpath: &str, target: &str) {
        let ino = self.alloc_ino();
        self.0.lock().unwrap().entries.insert(
            linkpath.to_string(),
            MockEntry::Symlink(
                target.to_string(),
                MockTimes {
                    ino,
                    ..Default::default()
                },
            ),
        );
    }

    /// Resolve symlink components in a path (excluding the final component).
    /// This mimics how the kernel resolves paths for operations like rename.
    fn resolve_parent_path(
        &self,
        inner: &MockFilesystemInner,
        path: &str,
    ) -> Result<String, Error> {
        if path == "/" || !path.contains('/') {
            return Ok(path.to_string());
        }

        let path = path.trim_end_matches('/');
        let parts: Vec<&str> = path.split('/').collect();
        if parts.is_empty() {
            return Ok(path.to_string());
        }

        // Resolve all components except the last one
        let mut resolved = String::new();
        for (i, part) in parts.iter().enumerate() {
            if part.is_empty() {
                continue;
            }
            let current = if resolved.is_empty() {
                format!("/{}", part)
            } else {
                format!("{}/{}", resolved, part)
            };

            // For all but the last component, follow symlinks
            if i < parts.len() - 1 {
                let mut depth = 0;
                let mut check_path = current.clone();
                loop {
                    if depth > 40 {
                        return Err(Error::Io(std::io::Error::other(
                            "too many levels of symbolic links",
                        )));
                    }
                    match inner.entries.get(&check_path) {
                        Some(MockEntry::Symlink(target, _)) => {
                            check_path = target.clone();
                            depth += 1;
                        }
                        Some(MockEntry::Directory(_)) => {
                            resolved = check_path;
                            break;
                        }
                        _ => {
                            resolved = check_path;
                            break;
                        }
                    }
                }
            } else {
                // Last component - don't follow symlinks
                resolved = current;
            }
        }

        Ok(resolved)
    }
}

impl Default for MockFilesystem {
    fn default() -> Self {
        Self::new()
    }
}

impl Filesystem for MockFilesystem {
    fn dup(&self) -> Self {
        Self(Arc::clone(&self.0))
    }

    fn read_to_string(&self, path: &str) -> Result<String, Error> {
        let inner = self.0.lock().unwrap();
        let mut current_path = path.to_string();
        let mut depth = 0;
        loop {
            if depth > 40 {
                return Err(Error::Io(std::io::Error::other(
                    "too many levels of symbolic links",
                )));
            }
            match inner.entries.get(&current_path) {
                Some(MockEntry::File(contents, _)) => return Ok(contents.clone()),
                Some(MockEntry::Directory(_)) => {
                    return Err(Error::Io(std::io::Error::new(
                        std::io::ErrorKind::IsADirectory,
                        path,
                    )));
                }
                Some(MockEntry::Symlink(target, _)) => {
                    current_path = target.clone();
                    depth += 1;
                }
                None => {
                    return Err(Error::Io(std::io::Error::new(
                        std::io::ErrorKind::NotFound,
                        path,
                    )));
                }
            }
        }
    }

    fn exists(&self, path: &str) -> bool {
        self.0.lock().unwrap().entries.contains_key(path)
    }

    fn metadata(&self, path: &str) -> Result<FileMetadata, Error> {
        self.0
            .lock()
            .unwrap()
            .entries
            .get(path)
            .map(|entry| FileMetadata {
                size: match entry {
                    MockEntry::File(contents, _) => contents.len() as u64,
                    MockEntry::Directory(_) => 0,
                    MockEntry::Symlink(target, _) => target.len() as u64,
                },
            })
            .ok_or_else(|| Error::Io(std::io::Error::new(std::io::ErrorKind::NotFound, path)))
    }

    fn truncate(&self, path: &str, size: u64) -> Result<(), Error> {
        let ino = self.alloc_ino();
        let mut inner = self.0.lock().unwrap();
        let entry = inner.entries.entry(path.to_string()).or_insert_with(|| {
            MockEntry::File(
                String::new(),
                MockTimes {
                    ino,
                    ..Default::default()
                },
            )
        });
        if let MockEntry::File(contents, _) = entry {
            let size = size as usize;
            if contents.len() > size {
                contents.truncate(size);
            } else {
                contents.extend(std::iter::repeat_n('\0', size - contents.len()));
            }
        }
        Ok(())
    }

    fn truncate_existing(&self, path: &str, size: u64) -> Result<bool, Error> {
        let mut inner = self.0.lock().unwrap();
        if let Some(MockEntry::File(contents, _)) = inner.entries.get_mut(path) {
            let size = size as usize;
            if contents.len() > size {
                contents.truncate(size);
            } else {
                contents.extend(std::iter::repeat_n('\0', size - contents.len()));
            }
            Ok(true)
        } else {
            Ok(false)
        }
    }

    fn punch_hole(&self, path: &str, offset: u64, length: u64) -> Result<(), Error> {
        let mut inner = self.0.lock().unwrap();
        let contents = match inner.entries.get_mut(path) {
            Some(MockEntry::File(contents, _)) => contents,
            _ => {
                return Err(Error::Io(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    path,
                )));
            }
        };

        let offset = offset as usize;
        let length = length as usize;
        let end = offset.saturating_add(length);

        // Extend file if necessary
        if contents.len() < end {
            contents.extend(std::iter::repeat_n('\0', end - contents.len()));
        }

        // Replace characters at offset..end with NUL bytes
        let bytes = unsafe { contents.as_bytes_mut() };
        for byte in bytes.iter_mut().skip(offset).take(length) {
            *byte = b'\0';
        }

        Ok(())
    }

    fn write_string(&self, path: &str, contents: &str) -> Result<(), Error> {
        let ino = self.alloc_ino();
        self.0.lock().unwrap().entries.insert(
            path.to_string(),
            MockEntry::File(
                contents.to_string(),
                MockTimes {
                    ino,
                    ..Default::default()
                },
            ),
        );
        Ok(())
    }

    fn append_string(&self, path: &str, contents: &str) -> Result<(), Error> {
        let ino = self.alloc_ino();
        let mut inner = self.0.lock().unwrap();
        match inner.entries.entry(path.to_string()).or_insert_with(|| {
            MockEntry::File(
                String::new(),
                MockTimes {
                    ino,
                    ..Default::default()
                },
            )
        }) {
            MockEntry::File(existing, _) => existing.push_str(contents),
            MockEntry::Directory(_) => {
                return Err(Error::Io(std::io::Error::new(
                    std::io::ErrorKind::IsADirectory,
                    path,
                )));
            }
            MockEntry::Symlink(_, _) => {
                return Err(Error::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "cannot append to a symbolic link",
                )));
            }
        }
        Ok(())
    }

    fn mkdir(&self, path: &str) -> Result<(), Error> {
        let ino = self.alloc_ino();
        let mut inner = self.0.lock().unwrap();
        // Check if it already exists
        if inner.entries.contains_key(path) {
            return Err(Error::Io(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                path,
            )));
        }
        // Check if parent exists (unless root or single component)
        let path_obj = std::path::Path::new(path);
        if let Some(parent) = path_obj.parent() {
            let parent_str = parent.to_str().unwrap_or("");
            // Parent must exist and be a directory (or be empty/root)
            if !parent_str.is_empty() && parent_str != "/" {
                match inner.entries.get(parent_str) {
                    Some(MockEntry::Directory(_)) => {}
                    Some(MockEntry::File(_, _) | MockEntry::Symlink(_, _)) => {
                        return Err(Error::Io(std::io::Error::new(
                            std::io::ErrorKind::NotADirectory,
                            parent_str,
                        )));
                    }
                    None => {
                        return Err(Error::Io(std::io::Error::new(
                            std::io::ErrorKind::NotFound,
                            parent_str,
                        )));
                    }
                }
            }
        }
        inner.entries.insert(
            path.to_string(),
            MockEntry::Directory(MockTimes {
                ino,
                ..Default::default()
            }),
        );
        Ok(())
    }

    fn mkdir_all(&self, path: &str) -> Result<(), Error> {
        let mut inner = self.0.lock().unwrap();
        // If it already exists as a directory, success
        if let Some(entry) = inner.entries.get(path) {
            match entry {
                MockEntry::Directory(_) => return Ok(()),
                MockEntry::File(_, _) | MockEntry::Symlink(_, _) => {
                    return Err(Error::Io(std::io::Error::new(
                        std::io::ErrorKind::AlreadyExists,
                        path,
                    )));
                }
            }
        }
        // Create all parent directories
        let path_obj = std::path::Path::new(path);
        let mut ancestors: Vec<_> = path_obj.ancestors().collect();
        ancestors.reverse();
        for ancestor in ancestors {
            let ancestor_str = ancestor.to_str().unwrap_or("");
            if ancestor_str.is_empty() || ancestor_str == "/" {
                continue;
            }
            if let Some(entry) = inner.entries.get(ancestor_str) {
                match entry {
                    MockEntry::Directory(_) => {}
                    MockEntry::File(_, _) | MockEntry::Symlink(_, _) => {
                        return Err(Error::Io(std::io::Error::new(
                            std::io::ErrorKind::NotADirectory,
                            ancestor_str,
                        )));
                    }
                }
            } else {
                let ino = inner.next_ino;
                inner.next_ino += 1;
                inner.entries.insert(
                    ancestor_str.to_string(),
                    MockEntry::Directory(MockTimes {
                        ino,
                        ..Default::default()
                    }),
                );
            }
        }
        Ok(())
    }

    fn is_dir(&self, path: &str) -> bool {
        self.0
            .lock()
            .unwrap()
            .entries
            .get(path)
            .map(|entry| matches!(entry, MockEntry::Directory(_)))
            .unwrap_or(false)
    }

    fn read_dir(&self, path: &str) -> Result<Vec<(String, DirEntry)>, Error> {
        let inner = self.0.lock().unwrap();

        // Check if path is a directory
        match inner.entries.get(path) {
            Some(MockEntry::Directory(_)) => {}
            Some(MockEntry::File(_, _) | MockEntry::Symlink(_, _)) => {
                return Err(Error::Io(std::io::Error::new(
                    std::io::ErrorKind::NotADirectory,
                    path,
                )));
            }
            None => {
                return Err(Error::Io(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    path,
                )));
            }
        }

        let prefix = if path == "/" {
            "/".to_string()
        } else {
            format!("{}/", path.trim_end_matches('/'))
        };

        let mut entries = Vec::new();
        for (file_path, entry) in inner.entries.iter() {
            if file_path == path {
                continue;
            }
            if let Some(rest) = file_path.strip_prefix(&prefix) {
                // Only include direct children (no slashes in the rest)
                if !rest.contains('/') && !rest.is_empty() {
                    let (file_type, size, times) = match entry {
                        MockEntry::File(contents, times) => {
                            (FileType::RegularFile, contents.len() as u64, times)
                        }
                        MockEntry::Directory(times) => (FileType::Directory, 0, times),
                        MockEntry::Symlink(target, times) => {
                            (FileType::Symlink, target.len() as u64, times)
                        }
                    };
                    entries.push((
                        rest.to_string(),
                        DirEntry {
                            file_type,
                            size,
                            atime_ms: times.atime_ms,
                            mtime_ms: times.mtime_ms,
                            dev: 1,
                            ino: times.ino,
                        },
                    ));
                }
            }
        }
        Ok(entries)
    }

    fn stat(&self, path: &str) -> Result<DirEntry, Error> {
        let inner = self.0.lock().unwrap();
        let has_trailing_slash = path.len() > 1 && path.ends_with('/');
        let normalized = path.trim_end_matches('/');
        let mut current_path = if normalized.is_empty() {
            "/".to_string()
        } else {
            normalized.to_string()
        };
        let mut depth = 0;
        loop {
            if depth > 40 {
                return Err(Error::Io(std::io::Error::other(
                    "too many levels of symbolic links",
                )));
            }
            match inner.entries.get(&current_path) {
                Some(MockEntry::File(contents, times)) => {
                    if has_trailing_slash {
                        return Err(Error::Io(std::io::Error::new(
                            std::io::ErrorKind::NotADirectory,
                            path,
                        )));
                    }
                    return Ok(DirEntry {
                        file_type: FileType::RegularFile,
                        size: contents.len() as u64,
                        atime_ms: times.atime_ms,
                        mtime_ms: times.mtime_ms,
                        dev: 1,
                        ino: times.ino,
                    });
                }
                Some(MockEntry::Directory(times)) => {
                    return Ok(DirEntry {
                        file_type: FileType::Directory,
                        size: 0,
                        atime_ms: times.atime_ms,
                        mtime_ms: times.mtime_ms,
                        dev: 1,
                        ino: times.ino,
                    });
                }
                Some(MockEntry::Symlink(target, _)) => {
                    current_path = target.clone();
                    depth += 1;
                }
                None => {
                    return Err(Error::Io(std::io::Error::new(
                        std::io::ErrorKind::NotFound,
                        path,
                    )));
                }
            }
        }
    }

    fn lstat(&self, path: &str) -> Result<DirEntry, Error> {
        let inner = self.0.lock().unwrap();
        match inner.entries.get(path) {
            Some(MockEntry::File(contents, times)) => Ok(DirEntry {
                file_type: FileType::RegularFile,
                size: contents.len() as u64,
                atime_ms: times.atime_ms,
                mtime_ms: times.mtime_ms,
                dev: 1,
                ino: times.ino,
            }),
            Some(MockEntry::Directory(times)) => Ok(DirEntry {
                file_type: FileType::Directory,
                size: 0,
                atime_ms: times.atime_ms,
                mtime_ms: times.mtime_ms,
                dev: 1,
                ino: times.ino,
            }),
            Some(MockEntry::Symlink(target, times)) => Ok(DirEntry {
                file_type: FileType::Symlink,
                size: target.len() as u64,
                atime_ms: times.atime_ms,
                mtime_ms: times.mtime_ms,
                dev: 1,
                ino: times.ino,
            }),
            None => Err(Error::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                path,
            ))),
        }
    }

    fn symlink(&self, target: &str, linkpath: &str) -> Result<(), Error> {
        let ino = self.alloc_ino();
        let mut inner = self.0.lock().unwrap();
        if inner.entries.contains_key(linkpath) {
            return Err(Error::Io(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                linkpath,
            )));
        }
        inner.entries.insert(
            linkpath.to_string(),
            MockEntry::Symlink(
                target.to_string(),
                MockTimes {
                    ino,
                    ..Default::default()
                },
            ),
        );
        Ok(())
    }

    fn link(&self, src: &str, dst: &str) -> Result<(), Error> {
        let mut inner = self.0.lock().unwrap();
        if inner.entries.contains_key(dst) {
            return Err(Error::Io(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                dst,
            )));
        }
        let entry = inner
            .entries
            .get(src)
            .cloned()
            .ok_or_else(|| Error::Io(std::io::Error::new(std::io::ErrorKind::NotFound, src)))?;
        match entry {
            MockEntry::Directory(_) => {
                return Err(Error::Io(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "hard links to directories are not allowed",
                )));
            }
            MockEntry::File(_, _) | MockEntry::Symlink(_, _) => {
                // Hard links share the same inode
                inner.entries.insert(dst.to_string(), entry);
            }
        }
        Ok(())
    }

    fn unlink(&self, path: &str) -> Result<(), Error> {
        let mut inner = self.0.lock().unwrap();
        match inner.entries.get(path) {
            Some(MockEntry::Directory(_)) => Err(Error::Io(std::io::Error::new(
                std::io::ErrorKind::IsADirectory,
                path,
            ))),
            Some(_) => {
                inner.entries.remove(path);
                Ok(())
            }
            None => Err(Error::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                path,
            ))),
        }
    }

    fn rmdir(&self, path: &str) -> Result<(), Error> {
        let mut inner = self.0.lock().unwrap();
        match inner.entries.get(path) {
            Some(MockEntry::Directory(_)) => {
                let prefix = format!("{}/", path.trim_end_matches('/'));
                let has_children = inner.entries.keys().any(|k| k.starts_with(&prefix));
                if has_children {
                    return Err(Error::Io(std::io::Error::new(
                        std::io::ErrorKind::DirectoryNotEmpty,
                        path,
                    )));
                }
                inner.entries.remove(path);
                Ok(())
            }
            Some(_) => Err(Error::Io(std::io::Error::new(
                std::io::ErrorKind::NotADirectory,
                path,
            ))),
            None => Err(Error::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                path,
            ))),
        }
    }

    fn readlink(&self, path: &str) -> Result<String, Error> {
        let inner = self.0.lock().unwrap();
        match inner.entries.get(path) {
            Some(MockEntry::Symlink(target, _)) => Ok(target.clone()),
            Some(_) => Err(Error::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "not a symbolic link",
            ))),
            None => Err(Error::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                path,
            ))),
        }
    }

    fn set_times(&self, path: &str, atime: TimeSpec, mtime: TimeSpec) -> Result<(), Error> {
        let mut inner = self.0.lock().unwrap();
        let mut current_path = path.to_string();
        let mut depth = 0;

        // Follow symlinks to find the target
        loop {
            if depth > 40 {
                return Err(Error::Io(std::io::Error::other(
                    "too many levels of symbolic links",
                )));
            }
            match inner.entries.get(&current_path) {
                Some(MockEntry::Symlink(target, _)) => {
                    current_path = target.clone();
                    depth += 1;
                }
                Some(_) => break,
                None => {
                    return Err(Error::Io(std::io::Error::new(
                        std::io::ErrorKind::NotFound,
                        path,
                    )));
                }
            }
        }

        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);

        let entry = inner.entries.get_mut(&current_path).unwrap();
        let times = match entry {
            MockEntry::File(_, times) => times,
            MockEntry::Directory(times) => times,
            MockEntry::Symlink(_, times) => times,
        };

        match atime {
            TimeSpec::Now => times.atime_ms = now_ms,
            TimeSpec::Omit => {}
            TimeSpec::Time(t) => times.atime_ms = t,
        }

        match mtime {
            TimeSpec::Now => times.mtime_ms = now_ms,
            TimeSpec::Omit => {}
            TimeSpec::Time(t) => times.mtime_ms = t,
        }

        Ok(())
    }

    fn lset_times(&self, path: &str, atime: TimeSpec, mtime: TimeSpec) -> Result<(), Error> {
        let mut inner = self.0.lock().unwrap();

        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);

        let entry = inner
            .entries
            .get_mut(path)
            .ok_or_else(|| Error::Io(std::io::Error::new(std::io::ErrorKind::NotFound, path)))?;

        let times = match entry {
            MockEntry::File(_, times) => times,
            MockEntry::Directory(times) => times,
            MockEntry::Symlink(_, times) => times,
        };

        match atime {
            TimeSpec::Now => times.atime_ms = now_ms,
            TimeSpec::Omit => {}
            TimeSpec::Time(t) => times.atime_ms = t,
        }

        match mtime {
            TimeSpec::Now => times.mtime_ms = now_ms,
            TimeSpec::Omit => {}
            TimeSpec::Time(t) => times.mtime_ms = t,
        }

        Ok(())
    }

    fn create_file(&self, path: &str) -> Result<bool, Error> {
        let ino = self.alloc_ino();
        let mut inner = self.0.lock().unwrap();
        if inner.entries.contains_key(path) {
            return Ok(false);
        }
        inner.entries.insert(
            path.to_string(),
            MockEntry::File(
                String::new(),
                MockTimes {
                    ino,
                    ..Default::default()
                },
            ),
        );
        Ok(true)
    }

    fn rename(&self, src: &str, dst: &str) -> Result<(), Error> {
        // First resolve the destination path (following symlinks in parent directories)
        let dst_resolved = {
            let inner = self.0.lock().unwrap();
            self.resolve_parent_path(&inner, dst)?
        };

        let mut inner = self.0.lock().unwrap();

        // Check if we're trying to move a directory into itself
        let src_trimmed = src.trim_end_matches('/');
        let dst_trimmed = dst_resolved.trim_end_matches('/');
        let src_prefix = format!("{}/", src_trimmed);
        if dst_trimmed.starts_with(&src_prefix) || dst_trimmed == src_trimmed {
            return Err(Error::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "cannot move directory into itself",
            )));
        }

        // Check if source exists
        let entry = inner
            .entries
            .remove(src)
            .ok_or_else(|| Error::Io(std::io::Error::new(std::io::ErrorKind::NotFound, src)))?;

        // Check if destination exists
        if inner.entries.contains_key(&dst_resolved) {
            // If source is a directory and dest exists, it's an error unless dest is also
            // an empty directory
            if let MockEntry::Directory(_) = &entry {
                match inner.entries.get(&dst_resolved) {
                    Some(MockEntry::Directory(_)) => {
                        // Check if dest directory is empty
                        let prefix = format!("{}/", dst_resolved.trim_end_matches('/'));
                        let has_children = inner.entries.keys().any(|k| k.starts_with(&prefix));
                        if has_children {
                            // Put source back and return error
                            inner.entries.insert(src.to_string(), entry);
                            return Err(Error::Io(std::io::Error::new(
                                std::io::ErrorKind::DirectoryNotEmpty,
                                dst_resolved,
                            )));
                        }
                        // Remove empty destination directory
                        inner.entries.remove(&dst_resolved);
                    }
                    Some(_) => {
                        // Can't replace a file with a directory
                        inner.entries.insert(src.to_string(), entry);
                        return Err(Error::Io(std::io::Error::new(
                            std::io::ErrorKind::NotADirectory,
                            dst_resolved,
                        )));
                    }
                    None => unreachable!(),
                }
            } else {
                // Source is a file, check if dest is a directory
                if let Some(MockEntry::Directory(_)) = inner.entries.get(&dst_resolved) {
                    inner.entries.insert(src.to_string(), entry);
                    return Err(Error::Io(std::io::Error::new(
                        std::io::ErrorKind::IsADirectory,
                        dst_resolved,
                    )));
                }
                // Remove existing file/symlink at destination
                inner.entries.remove(&dst_resolved);
            }
        }

        // For directories, we also need to move all children
        if let MockEntry::Directory(_) = &entry {
            let src_prefix = format!("{}/", src.trim_end_matches('/'));
            let dst_prefix = format!("{}/", dst_resolved.trim_end_matches('/'));

            // Collect all children to move
            let children_to_move: Vec<(String, MockEntry)> = inner
                .entries
                .iter()
                .filter(|(k, _)| k.starts_with(&src_prefix))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();

            // Remove old children and insert with new paths
            for (old_path, child_entry) in children_to_move {
                inner.entries.remove(&old_path);
                let new_path = format!("{}{}", dst_prefix, &old_path[src_prefix.len()..]);
                inner.entries.insert(new_path, child_entry);
            }
        }

        inner.entries.insert(dst_resolved, entry);
        Ok(())
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

            let ino = self.alloc_ino();
            let mut inner = self.0.lock().unwrap();

            let path_obj = std::path::Path::new(&path);
            if let Some(parent) = path_obj.parent() {
                let parent_str = parent.to_str().unwrap_or("");
                if !parent_str.is_empty() && parent_str != "/" {
                    match inner.entries.get(parent_str) {
                        Some(MockEntry::Directory(_)) => {}
                        Some(_) => {
                            return Err(Error::Io(std::io::Error::new(
                                std::io::ErrorKind::NotADirectory,
                                parent_str,
                            )));
                        }
                        None => {
                            return Err(Error::Io(std::io::Error::new(
                                std::io::ErrorKind::NotFound,
                                parent_str,
                            )));
                        }
                    }
                }
            }

            if !inner.entries.contains_key(&path) {
                inner.entries.insert(
                    path.clone(),
                    MockEntry::File(
                        String::new(),
                        MockTimes {
                            ino,
                            ..Default::default()
                        },
                    ),
                );
                return Ok(path);
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

            let ino = self.alloc_ino();
            let mut inner = self.0.lock().unwrap();

            let path_obj = std::path::Path::new(&path);
            if let Some(parent) = path_obj.parent() {
                let parent_str = parent.to_str().unwrap_or("");
                if !parent_str.is_empty() && parent_str != "/" {
                    match inner.entries.get(parent_str) {
                        Some(MockEntry::Directory(_)) => {}
                        Some(_) => {
                            return Err(Error::Io(std::io::Error::new(
                                std::io::ErrorKind::NotADirectory,
                                parent_str,
                            )));
                        }
                        None => {
                            return Err(Error::Io(std::io::Error::new(
                                std::io::ErrorKind::NotFound,
                                parent_str,
                            )));
                        }
                    }
                }
            }

            if !inner.entries.contains_key(&path) {
                inner.entries.insert(
                    path.clone(),
                    MockEntry::Directory(MockTimes {
                        ino,
                        ..Default::default()
                    }),
                );
                return Ok(path);
            }
        }

        Err(Error::Io(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "could not create unique temporary directory",
        )))
    }
}

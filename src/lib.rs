use std::cell::RefCell;
use std::collections::HashMap;
use std::io::BufRead;
use std::io::Write;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;

use utf8path::Path;

mod builtins;

pub use builtins::lookup_bin;
pub use builtins::sh;

/// Errors that can occur during shell operations.
#[derive(Debug)]
pub enum Error {
    /// An error from shvar parsing.
    Shvar(shvar::Error),
    /// An I/O error.
    Io(std::io::Error),
    /// The command string was empty.
    EmptyCommand,
    /// The requested binary was not found.
    UnknownBinary(String),
}

impl From<shvar::Error> for Error {
    fn from(err: shvar::Error) -> Self {
        Self::Shvar(err)
    }
}

/// A trait for types that can serve as standard input.
pub trait Stdin {
    /// Duplicate the stdin handle.
    fn dup(&self) -> Self;
    /// Read a line from stdin, returning None at EOF.
    fn read_line(&self) -> Result<Option<String>, Error>;
}

impl Stdin for () {
    fn dup(&self) -> Self {
        *self
    }

    fn read_line(&self) -> Result<Option<String>, Error> {
        Ok(None)
    }
}

/// A stdin backed by a vector of lines.
#[derive(Clone)]
pub struct StringStdin(Rc<RefCell<Vec<String>>>);

impl StringStdin {
    /// Create a new StringStdin from a string, splitting on newlines.
    pub fn new(input: &str) -> Self {
        let lines: Vec<String> = input.lines().rev().map(|s| s.to_string()).collect();
        Self(Rc::new(RefCell::new(lines)))
    }
}

impl Stdin for StringStdin {
    fn dup(&self) -> Self {
        Self(Rc::clone(&self.0))
    }

    fn read_line(&self) -> Result<Option<String>, Error> {
        Ok(self.0.borrow_mut().pop())
    }
}

impl Stdin for std::io::Stdin {
    fn dup(&self) -> Self {
        std::io::stdin()
    }

    fn read_line(&self) -> Result<Option<String>, Error> {
        let mut buf = String::new();
        let n = self.lock().read_line(&mut buf).map_err(Error::Io)?;
        if n == 0 {
            Ok(None)
        } else {
            if buf.ends_with('\n') {
                buf.pop();
            }
            Ok(Some(buf))
        }
    }
}

/// A trait for types that can serve as standard output.
pub trait Stdout {
    /// Duplicate the stdout handle.
    fn dup(&self) -> Self;
    /// Write a string to stdout.
    fn write_str(&self, s: &str) -> Result<(), Error>;
    /// Write a string followed by a newline to stdout.
    fn write_line(&self, s: &str) -> Result<(), Error> {
        self.write_str(s)?;
        self.write_str("\n")
    }
}

impl Stdout for () {
    fn dup(&self) -> Self {
        *self
    }

    fn write_str(&self, _s: &str) -> Result<(), Error> {
        Ok(())
    }
}

/// A stdout that collects output into a string.
#[derive(Clone)]
pub struct StringStdout(Rc<RefCell<String>>);

impl StringStdout {
    /// Create a new empty StringStdout.
    pub fn new() -> Self {
        Self(Rc::new(RefCell::new(String::new())))
    }

    /// Get the collected output as a string.
    pub fn into_string(&self) -> String {
        self.0.borrow().clone()
    }
}

impl Default for StringStdout {
    fn default() -> Self {
        Self::new()
    }
}

impl Stdout for StringStdout {
    fn dup(&self) -> Self {
        Self(Rc::clone(&self.0))
    }

    fn write_str(&self, s: &str) -> Result<(), Error> {
        self.0.borrow_mut().push_str(s);
        Ok(())
    }
}

impl Stdout for std::io::Stdout {
    fn dup(&self) -> Self {
        std::io::stdout()
    }

    fn write_str(&self, s: &str) -> Result<(), Error> {
        self.lock().write_all(s.as_bytes()).map_err(Error::Io)
    }
}

/// A trait for types that can serve as standard error.
pub trait Stderr {
    /// Duplicate the stderr handle.
    fn dup(&self) -> Self;
    /// Write a string to stderr.
    fn write_str(&self, s: &str) -> Result<(), Error>;
    /// Write a string followed by a newline to stderr.
    fn write_line(&self, s: &str) -> Result<(), Error> {
        self.write_str(s)?;
        self.write_str("\n")
    }
}

impl Stderr for () {
    fn dup(&self) -> Self {
        *self
    }

    fn write_str(&self, _s: &str) -> Result<(), Error> {
        Ok(())
    }
}

/// A stderr that collects output into a string.
#[derive(Clone)]
pub struct StringStderr(Rc<RefCell<String>>);

impl StringStderr {
    /// Create a new empty StringStderr.
    pub fn new() -> Self {
        Self(Rc::new(RefCell::new(String::new())))
    }

    /// Get the collected output as a string.
    pub fn into_string(&self) -> String {
        self.0.borrow().clone()
    }
}

impl Default for StringStderr {
    fn default() -> Self {
        Self::new()
    }
}

impl Stderr for StringStderr {
    fn dup(&self) -> Self {
        Self(Rc::clone(&self.0))
    }

    fn write_str(&self, s: &str) -> Result<(), Error> {
        self.0.borrow_mut().push_str(s);
        Ok(())
    }
}

impl Stderr for std::io::Stderr {
    fn dup(&self) -> Self {
        std::io::stderr()
    }

    fn write_str(&self, s: &str) -> Result<(), Error> {
        self.lock().write_all(s.as_bytes()).map_err(Error::Io)
    }
}

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
    /// The modification time in milliseconds since UNIX epoch.
    pub mtime_ms: i64,
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

        // Seek to offset and write spaces
        file.seek(SeekFrom::Start(offset)).map_err(Error::Io)?;
        let spaces = vec![b' '; length as usize];
        file.write_all(&spaces).map_err(Error::Io)?;

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
            let dir_entry = DirEntry {
                file_type,
                size: metadata.len(),
                mtime_ms: metadata
                    .modified()
                    .map(|t| {
                        t.duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_millis() as i64)
                            .unwrap_or(0)
                    })
                    .unwrap_or(0),
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
        Ok(DirEntry {
            file_type,
            size: metadata.len(),
            mtime_ms: metadata
                .modified()
                .map(|t| {
                    t.duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_millis() as i64)
                        .unwrap_or(0)
                })
                .unwrap_or(0),
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
        Ok(DirEntry {
            file_type,
            size: metadata.len(),
            mtime_ms: metadata
                .modified()
                .map(|t| {
                    t.duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_millis() as i64)
                        .unwrap_or(0)
                })
                .unwrap_or(0),
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

/// Internal entry for the mock filesystem.
#[derive(Clone, Debug)]
enum MockEntry {
    /// A file with contents.
    File(String),
    /// A directory.
    Directory,
    /// A symbolic link with target path.
    Symlink(String),
}

/// A mock filesystem backed by a HashMap.
#[derive(Clone)]
pub struct MockFilesystem(Rc<RefCell<HashMap<String, MockEntry>>>);

impl MockFilesystem {
    /// Create a new empty MockFilesystem.
    pub fn new() -> Self {
        Self(Rc::new(RefCell::new(HashMap::new())))
    }

    /// Add a file to the mock filesystem.
    pub fn add_file(&self, path: &str, contents: &str) {
        self.0
            .borrow_mut()
            .insert(path.to_string(), MockEntry::File(contents.to_string()));
    }

    /// Add a directory to the mock filesystem.
    pub fn add_directory(&self, path: &str) {
        self.0
            .borrow_mut()
            .insert(path.to_string(), MockEntry::Directory);
    }
}

impl Default for MockFilesystem {
    fn default() -> Self {
        Self::new()
    }
}

impl Filesystem for MockFilesystem {
    fn dup(&self) -> Self {
        Self(Rc::clone(&self.0))
    }

    fn read_to_string(&self, path: &str) -> Result<String, Error> {
        self.0
            .borrow()
            .get(path)
            .and_then(|entry| match entry {
                MockEntry::File(contents) => Some(contents.clone()),
                MockEntry::Directory | MockEntry::Symlink(_) => None,
            })
            .ok_or_else(|| Error::Io(std::io::Error::new(std::io::ErrorKind::NotFound, path)))
    }

    fn exists(&self, path: &str) -> bool {
        self.0.borrow().contains_key(path)
    }

    fn metadata(&self, path: &str) -> Result<FileMetadata, Error> {
        self.0
            .borrow()
            .get(path)
            .map(|entry| FileMetadata {
                size: match entry {
                    MockEntry::File(contents) => contents.len() as u64,
                    MockEntry::Directory => 0,
                    MockEntry::Symlink(target) => target.len() as u64,
                },
            })
            .ok_or_else(|| Error::Io(std::io::Error::new(std::io::ErrorKind::NotFound, path)))
    }

    fn truncate(&self, path: &str, size: u64) -> Result<(), Error> {
        let mut files = self.0.borrow_mut();
        let entry = files
            .entry(path.to_string())
            .or_insert_with(|| MockEntry::File(String::new()));
        if let MockEntry::File(contents) = entry {
            let size = size as usize;
            if contents.len() > size {
                contents.truncate(size);
            } else {
                contents.extend(std::iter::repeat_n(' ', size - contents.len()));
            }
        }
        Ok(())
    }

    fn truncate_existing(&self, path: &str, size: u64) -> Result<bool, Error> {
        let mut files = self.0.borrow_mut();
        if let Some(MockEntry::File(contents)) = files.get_mut(path) {
            let size = size as usize;
            if contents.len() > size {
                contents.truncate(size);
            } else {
                contents.extend(std::iter::repeat_n(' ', size - contents.len()));
            }
            Ok(true)
        } else {
            Ok(false)
        }
    }

    fn punch_hole(&self, path: &str, offset: u64, length: u64) -> Result<(), Error> {
        let mut files = self.0.borrow_mut();
        let contents = match files.get_mut(path) {
            Some(MockEntry::File(contents)) => contents,
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
            contents.extend(std::iter::repeat_n(' ', end - contents.len()));
        }

        // Replace characters at offset..end with spaces
        let bytes = unsafe { contents.as_bytes_mut() };
        for byte in bytes.iter_mut().skip(offset).take(length) {
            *byte = b' ';
        }

        Ok(())
    }

    fn write_string(&self, path: &str, contents: &str) -> Result<(), Error> {
        self.0
            .borrow_mut()
            .insert(path.to_string(), MockEntry::File(contents.to_string()));
        Ok(())
    }

    fn append_string(&self, path: &str, contents: &str) -> Result<(), Error> {
        let mut files = self.0.borrow_mut();
        match files
            .entry(path.to_string())
            .or_insert_with(|| MockEntry::File(String::new()))
        {
            MockEntry::File(existing) => existing.push_str(contents),
            MockEntry::Directory => {
                return Err(Error::Io(std::io::Error::new(
                    std::io::ErrorKind::IsADirectory,
                    path,
                )));
            }
            MockEntry::Symlink(_) => {
                return Err(Error::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "cannot append to a symbolic link",
                )));
            }
        }
        Ok(())
    }

    fn mkdir(&self, path: &str) -> Result<(), Error> {
        let mut files = self.0.borrow_mut();
        // Check if it already exists
        if files.contains_key(path) {
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
                match files.get(parent_str) {
                    Some(MockEntry::Directory) => {}
                    Some(MockEntry::File(_) | MockEntry::Symlink(_)) => {
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
        files.insert(path.to_string(), MockEntry::Directory);
        Ok(())
    }

    fn mkdir_all(&self, path: &str) -> Result<(), Error> {
        let mut files = self.0.borrow_mut();
        // If it already exists as a directory, success
        if let Some(entry) = files.get(path) {
            match entry {
                MockEntry::Directory => return Ok(()),
                MockEntry::File(_) | MockEntry::Symlink(_) => {
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
            if let Some(entry) = files.get(ancestor_str) {
                match entry {
                    MockEntry::Directory => {}
                    MockEntry::File(_) | MockEntry::Symlink(_) => {
                        return Err(Error::Io(std::io::Error::new(
                            std::io::ErrorKind::NotADirectory,
                            ancestor_str,
                        )));
                    }
                }
            } else {
                files.insert(ancestor_str.to_string(), MockEntry::Directory);
            }
        }
        Ok(())
    }

    fn is_dir(&self, path: &str) -> bool {
        self.0
            .borrow()
            .get(path)
            .map(|entry| matches!(entry, MockEntry::Directory))
            .unwrap_or(false)
    }

    fn read_dir(&self, path: &str) -> Result<Vec<(String, DirEntry)>, Error> {
        let files = self.0.borrow();

        // Check if path is a directory
        match files.get(path) {
            Some(MockEntry::Directory) => {}
            Some(MockEntry::File(_) | MockEntry::Symlink(_)) => {
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
        for (file_path, entry) in files.iter() {
            if file_path == path {
                continue;
            }
            if let Some(rest) = file_path.strip_prefix(&prefix) {
                // Only include direct children (no slashes in the rest)
                if !rest.contains('/') && !rest.is_empty() {
                    let (file_type, size) = match entry {
                        MockEntry::File(contents) => (FileType::RegularFile, contents.len() as u64),
                        MockEntry::Directory => (FileType::Directory, 0),
                        MockEntry::Symlink(target) => (FileType::Symlink, target.len() as u64),
                    };
                    entries.push((
                        rest.to_string(),
                        DirEntry {
                            file_type,
                            size,
                            mtime_ms: 0,
                        },
                    ));
                }
            }
        }
        Ok(entries)
    }

    fn stat(&self, path: &str) -> Result<DirEntry, Error> {
        let files = self.0.borrow();
        let mut current_path = path.to_string();
        let mut depth = 0;
        loop {
            if depth > 40 {
                return Err(Error::Io(std::io::Error::other(
                    "too many levels of symbolic links",
                )));
            }
            match files.get(&current_path) {
                Some(MockEntry::File(contents)) => {
                    return Ok(DirEntry {
                        file_type: FileType::RegularFile,
                        size: contents.len() as u64,
                        mtime_ms: 0,
                    });
                }
                Some(MockEntry::Directory) => {
                    return Ok(DirEntry {
                        file_type: FileType::Directory,
                        size: 0,
                        mtime_ms: 0,
                    });
                }
                Some(MockEntry::Symlink(target)) => {
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
        let files = self.0.borrow();
        match files.get(path) {
            Some(MockEntry::File(contents)) => Ok(DirEntry {
                file_type: FileType::RegularFile,
                size: contents.len() as u64,
                mtime_ms: 0,
            }),
            Some(MockEntry::Directory) => Ok(DirEntry {
                file_type: FileType::Directory,
                size: 0,
                mtime_ms: 0,
            }),
            Some(MockEntry::Symlink(target)) => Ok(DirEntry {
                file_type: FileType::Symlink,
                size: target.len() as u64,
                mtime_ms: 0,
            }),
            None => Err(Error::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                path,
            ))),
        }
    }

    fn symlink(&self, target: &str, linkpath: &str) -> Result<(), Error> {
        let mut files = self.0.borrow_mut();
        if files.contains_key(linkpath) {
            return Err(Error::Io(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                linkpath,
            )));
        }
        files.insert(linkpath.to_string(), MockEntry::Symlink(target.to_string()));
        Ok(())
    }

    fn link(&self, src: &str, dst: &str) -> Result<(), Error> {
        let mut files = self.0.borrow_mut();
        if files.contains_key(dst) {
            return Err(Error::Io(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                dst,
            )));
        }
        let entry = files
            .get(src)
            .cloned()
            .ok_or_else(|| Error::Io(std::io::Error::new(std::io::ErrorKind::NotFound, src)))?;
        match entry {
            MockEntry::Directory => {
                return Err(Error::Io(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "hard links to directories are not allowed",
                )));
            }
            MockEntry::File(_) | MockEntry::Symlink(_) => {
                files.insert(dst.to_string(), entry);
            }
        }
        Ok(())
    }

    fn unlink(&self, path: &str) -> Result<(), Error> {
        let mut files = self.0.borrow_mut();
        match files.get(path) {
            Some(MockEntry::Directory) => Err(Error::Io(std::io::Error::new(
                std::io::ErrorKind::IsADirectory,
                path,
            ))),
            Some(_) => {
                files.remove(path);
                Ok(())
            }
            None => Err(Error::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                path,
            ))),
        }
    }

    fn rmdir(&self, path: &str) -> Result<(), Error> {
        let mut files = self.0.borrow_mut();
        match files.get(path) {
            Some(MockEntry::Directory) => {
                let prefix = format!("{}/", path.trim_end_matches('/'));
                let has_children = files.keys().any(|k| k.starts_with(&prefix));
                if has_children {
                    return Err(Error::Io(std::io::Error::new(
                        std::io::ErrorKind::DirectoryNotEmpty,
                        path,
                    )));
                }
                files.remove(path);
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
        let files = self.0.borrow();
        match files.get(path) {
            Some(MockEntry::Symlink(target)) => Ok(target.clone()),
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

            let mut files = self.0.borrow_mut();

            let path_obj = std::path::Path::new(&path);
            if let Some(parent) = path_obj.parent() {
                let parent_str = parent.to_str().unwrap_or("");
                if !parent_str.is_empty() && parent_str != "/" {
                    match files.get(parent_str) {
                        Some(MockEntry::Directory) => {}
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

            if !files.contains_key(&path) {
                files.insert(path.clone(), MockEntry::File(String::new()));
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

            let mut files = self.0.borrow_mut();

            let path_obj = std::path::Path::new(&path);
            if let Some(parent) = path_obj.parent() {
                let parent_str = parent.to_str().unwrap_or("");
                if !parent_str.is_empty() && parent_str != "/" {
                    match files.get(parent_str) {
                        Some(MockEntry::Directory) => {}
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

            if !files.contains_key(&path) {
                files.insert(path.clone(), MockEntry::Directory);
                return Ok(path);
            }
        }

        Err(Error::Io(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "could not create unique temporary directory",
        )))
    }
}

/// The execution environment for a command.
pub struct Environment<SI, SO, SE, FS = RealFilesystem>
where
    SI: Stdin,
    SO: Stdout,
    SE: Stderr,
    FS: Filesystem,
{
    /// Standard input.
    pub stdin: SI,
    /// Standard output.
    pub stdout: SO,
    /// Standard error.
    pub stderr: SE,
    /// Filesystem access.
    pub fs: FS,
    /// Environment variables.
    pub env: HashMap<String, String>,
    /// Command-line arguments.
    pub args: Vec<String>,
    /// Current working directory.
    pub cwd: Path<'static>,
    /// Whether an exit has been signaled by the `exit` builtin.
    pub exit_signaled: Arc<AtomicBool>,
}

impl Default for Environment<std::io::Stdin, std::io::Stdout, std::io::Stderr, RealFilesystem> {
    fn default() -> Self {
        Self {
            stdin: std::io::stdin(),
            stdout: std::io::stdout(),
            stderr: std::io::stderr(),
            fs: RealFilesystem,
            env: HashMap::from_iter([
                ("COLUMNS".to_string(), "120".to_string()),
                ("HOME".to_string(), "/home/assistant".to_string()),
                ("PATH".to_string(), "/usr/bin:/bin".to_string()),
                ("PWD".to_string(), "/".to_string()),
                ("SHELL".to_string(), "synshell".to_string()),
                ("TMPDIR".to_string(), "/tmp".to_string()),
                ("USER".to_string(), "assistant".to_string()),
            ]),
            args: vec!["/bin/synshell".to_string()],
            cwd: Path::from("/home/assistant"),
            exit_signaled: Arc::new(AtomicBool::new(false)),
        }
    }
}

impl<SI, SO, SE, FS> Environment<SI, SO, SE, FS>
where
    SI: Stdin,
    SO: Stdout,
    SE: Stderr,
    FS: Filesystem,
{
    /// Duplicate the environment.
    pub fn dup(&self) -> Self {
        Self {
            stdin: self.stdin.dup(),
            stdout: self.stdout.dup(),
            stderr: self.stderr.dup(),
            fs: self.fs.dup(),
            env: self.env.clone(),
            args: self.args.clone(),
            cwd: self.cwd.clone(),
            exit_signaled: Arc::clone(&self.exit_signaled),
        }
    }

    /// Signal that an exit has been requested.
    pub fn signal_exit(&self) {
        self.exit_signaled.store(true, Ordering::SeqCst);
    }

    /// Check if an exit has been signaled.
    pub fn is_exit_signaled(&self) -> bool {
        self.exit_signaled.load(Ordering::SeqCst)
    }

    /// Set the arguments for the environment.
    pub fn with_args(mut self, args: Vec<String>) -> Self {
        self.args = args;
        self
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ExitCode(i8);

impl ExitCode {
    pub fn code(self) -> i8 {
        self.0
    }
}

impl From<i8> for ExitCode {
    fn from(code: i8) -> Self {
        Self(code)
    }
}

impl std::fmt::Display for ExitCode {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// A command to be executed.
#[allow(clippy::type_complexity)]
pub struct Command<SI, SO, SE, FS = RealFilesystem>
where
    SI: Stdin,
    SO: Stdout,
    SE: Stderr,
    FS: Filesystem,
{
    env: Environment<SI, SO, SE, FS>,
    bin: fn(&Environment<SI, SO, SE, FS>) -> Result<ExitCode, Error>,
}

impl<SI, SO, SE, FS> Command<SI, SO, SE, FS>
where
    SI: Stdin,
    SO: Stdout,
    SE: Stderr,
    FS: Filesystem,
{
    /// Create a new command from a binary name and environment.
    pub fn new(bin: &str, env: Environment<SI, SO, SE, FS>) -> Result<Self, Error> {
        let bin = builtins::lookup_bin(bin)?;
        Ok(Self { env, bin })
    }

    /// Duplicate the command.
    pub fn dup(&self) -> Self {
        Self {
            env: self.env.dup(),
            bin: self.bin,
        }
    }

    /// Run the command and return its exit code.
    pub fn run(self) -> Result<ExitCode, Error> {
        (self.bin)(&self.env)
    }
}

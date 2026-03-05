//! Eudaemon types: shared types for the eudaemon ecosystem.
//!
//! This crate provides the core abstractions used across eudaemon crates:
//! - [`Filesystem`] trait for filesystem operations
//! - [`StdioIn`], [`StdioOut`] traits for I/O
//! - Common types like [`FileType`], [`DirEntry`], [`TimeSpec`]

#![deny(missing_docs)]

use std::cell::RefCell;
use std::io::BufRead;
use std::io::Write;
use std::rc::Rc;

use utf8path::Path;

mod directory;

pub use directory::DirectoryFilesystem;

////////////////////////////////////////////// Error ///////////////////////////////////////////////

/// Errors that can occur during eudaemonty operations.
#[derive(Debug)]
pub enum Error {
    /// An I/O error.
    Io(std::io::Error),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Io(e) => write!(f, "{}", e),
        }
    }
}

impl std::error::Error for Error {}

////////////////////////////////////////////// StdioIn //////////////////////////////////////////////

/// A trait for types that can serve as standard input.
pub trait StdioIn {
    /// Duplicate the handle.
    fn dup(&self) -> Self;
    /// Read a line, returning None at EOF.
    fn read_line(&self) -> Result<Option<String>, Error>;
}

impl StdioIn for () {
    fn dup(&self) -> Self {}

    fn read_line(&self) -> Result<Option<String>, Error> {
        Ok(None)
    }
}

/// An input backed by a vector of lines.
#[derive(Clone)]
pub struct StringStdioIn(Rc<RefCell<Vec<String>>>);

impl StringStdioIn {
    /// Create a new StringStdioIn from a string, splitting on newlines.
    pub fn new(input: &str) -> Self {
        let lines: Vec<String> = input.lines().rev().map(|s| s.to_string()).collect();
        Self(Rc::new(RefCell::new(lines)))
    }
}

impl StdioIn for StringStdioIn {
    fn dup(&self) -> Self {
        Self(Rc::clone(&self.0))
    }

    fn read_line(&self) -> Result<Option<String>, Error> {
        Ok(self.0.borrow_mut().pop())
    }
}

impl StdioIn for std::io::Stdin {
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

////////////////////////////////////////////// StdioOut /////////////////////////////////////////////

/// A trait for types that can serve as output (stdout or stderr).
pub trait StdioOut {
    /// Duplicate the handle.
    fn dup(&self) -> Self;
    /// Write a string.
    fn write_str(&self, s: &str) -> Result<(), Error>;
    /// Write a string followed by a newline.
    fn write_line(&self, s: &str) -> Result<(), Error> {
        self.write_str(s)?;
        self.write_str("\n")
    }
}

impl StdioOut for () {
    fn dup(&self) -> Self {}

    fn write_str(&self, _s: &str) -> Result<(), Error> {
        Ok(())
    }
}

/// An output that collects into a string.
#[derive(Clone)]
pub struct StringStdioOut(Rc<RefCell<String>>);

impl StringStdioOut {
    /// Create a new empty StringStdioOut.
    pub fn new() -> Self {
        Self(Rc::new(RefCell::new(String::new())))
    }

    /// Get the collected output as a string.
    pub fn into_string(&self) -> String {
        self.0.borrow().clone()
    }
}

impl Default for StringStdioOut {
    fn default() -> Self {
        Self::new()
    }
}

impl StdioOut for StringStdioOut {
    fn dup(&self) -> Self {
        Self(Rc::clone(&self.0))
    }

    fn write_str(&self, s: &str) -> Result<(), Error> {
        self.0.borrow_mut().push_str(s);
        Ok(())
    }
}

impl StdioOut for std::io::Stdout {
    fn dup(&self) -> Self {
        std::io::stdout()
    }

    fn write_str(&self, s: &str) -> Result<(), Error> {
        self.lock().write_all(s.as_bytes()).map_err(Error::Io)
    }
}

impl StdioOut for std::io::Stderr {
    fn dup(&self) -> Self {
        std::io::stderr()
    }

    fn write_str(&self, s: &str) -> Result<(), Error> {
        self.lock().write_all(s.as_bytes()).map_err(Error::Io)
    }
}

/// An output that writes to a file via a filesystem (truncating).
pub struct FileStdioOut<FS: Filesystem> {
    fs: FS,
    path: String,
    buffer: Arc<Mutex<String>>,
}

impl<FS: Filesystem> FileStdioOut<FS> {
    /// Create a new FileStdioOut that writes to the given path.
    ///
    /// The file is truncated when created and all output is buffered.
    /// The buffer is flushed to the file when the last reference is dropped.
    pub fn new(fs: FS, path: String) -> Self {
        Self {
            fs,
            path,
            buffer: Arc::new(Mutex::new(String::new())),
        }
    }

    /// Flush the buffer to the file.
    pub fn flush(&self) -> Result<(), Error> {
        let buffer = self.buffer.lock().unwrap();
        self.fs.write_string(&self.path, &buffer)
    }
}

impl<FS: Filesystem> StdioOut for FileStdioOut<FS> {
    fn dup(&self) -> Self {
        Self {
            fs: self.fs.dup(),
            path: self.path.clone(),
            buffer: Arc::clone(&self.buffer),
        }
    }

    fn write_str(&self, s: &str) -> Result<(), Error> {
        self.buffer.lock().unwrap().push_str(s);
        Ok(())
    }
}

impl<FS: Filesystem> Drop for FileStdioOut<FS> {
    fn drop(&mut self) {
        // Only flush if this is the last reference to the buffer
        if Arc::strong_count(&self.buffer) == 1 {
            let _ = self.flush();
        }
    }
}

/// An output that appends to a file via a filesystem.
///
/// Unlike `FileStdioOut`, this writes immediately on each `write_str` call
/// using `append_string`, so output is appended to the file incrementally.
pub struct FileAppendStdioOut<FS: Filesystem> {
    fs: FS,
    path: String,
}

impl<FS: Filesystem> FileAppendStdioOut<FS> {
    /// Create a new FileAppendStdioOut that appends to the given path.
    pub fn new(fs: FS, path: String) -> Self {
        Self { fs, path }
    }
}

impl<FS: Filesystem> StdioOut for FileAppendStdioOut<FS> {
    fn dup(&self) -> Self {
        Self {
            fs: self.fs.dup(),
            path: self.path.clone(),
        }
    }

    fn write_str(&self, s: &str) -> Result<(), Error> {
        self.fs.append_string(&self.path, s)
    }
}

/////////////////////////////////////////////// Pipe /////////////////////////////////////////////////

use std::collections::VecDeque;
use std::sync::Condvar;

/// Internal state for a pipe.
struct PipeInner {
    /// Lines waiting to be read.
    lines: VecDeque<String>,
    /// Partial line being accumulated (no newline yet).
    partial: String,
    /// Number of active writers.
    writer_count: usize,
}

/// Shared state between pipe reader and writer, including condvar for blocking.
struct PipeShared {
    /// The pipe's internal state, protected by a mutex.
    inner: Mutex<PipeInner>,
    /// Condition variable to wake up blocked readers.
    condvar: Condvar,
}

/// Create a new pipe, returning the read and write ends.
///
/// The write end (`PipeWriter`) implements `StdioOut`.
/// The read end (`PipeReader`) implements `StdioIn`.
///
/// When all `PipeWriter` handles are closed, the reader will receive EOF
/// after draining any remaining buffered data.
pub fn mkpipe() -> (PipeReader, PipeWriter) {
    let shared = Arc::new(PipeShared {
        inner: Mutex::new(PipeInner {
            lines: VecDeque::new(),
            partial: String::new(),
            writer_count: 1,
        }),
        condvar: Condvar::new(),
    });
    (
        PipeReader {
            shared: Arc::clone(&shared),
        },
        PipeWriter { shared },
    )
}

/// The read end of a pipe.
///
/// Implements `StdioIn` for reading data written to the corresponding `PipeWriter`.
/// Returns EOF (None) when the buffer is empty and all writers have been closed.
/// Blocks if no data is available but writers are still active.
#[derive(Clone)]
pub struct PipeReader {
    shared: Arc<PipeShared>,
}

impl StdioIn for PipeReader {
    fn dup(&self) -> Self {
        Self {
            shared: Arc::clone(&self.shared),
        }
    }

    fn read_line(&self) -> Result<Option<String>, Error> {
        let mut inner = self.shared.inner.lock().unwrap();
        loop {
            // Try to return a complete line first
            if let Some(line) = inner.lines.pop_front() {
                return Ok(Some(line));
            }
            // No complete lines; check if pipe is closed
            if inner.writer_count == 0 {
                // Return any remaining partial content as the last line
                if inner.partial.is_empty() {
                    return Ok(None);
                } else {
                    return Ok(Some(std::mem::take(&mut inner.partial)));
                }
            }
            // Writers still active but no data available - block until notified
            inner = self.shared.condvar.wait(inner).unwrap();
        }
    }
}

/// The write end of a pipe.
///
/// Implements `StdioOut` for writing data to be read by the corresponding `PipeReader`.
/// When all `PipeWriter` handles are closed (via `close()` or dropped), the reader receives EOF.
pub struct PipeWriter {
    shared: Arc<PipeShared>,
}

impl Clone for PipeWriter {
    fn clone(&self) -> Self {
        let mut inner = self.shared.inner.lock().unwrap();
        inner.writer_count += 1;
        drop(inner);
        Self {
            shared: Arc::clone(&self.shared),
        }
    }
}

impl Drop for PipeWriter {
    fn drop(&mut self) {
        let mut inner = self.shared.inner.lock().unwrap();
        inner.writer_count = inner.writer_count.saturating_sub(1);
        // Notify readers that a writer has closed (they may now see EOF)
        drop(inner);
        self.shared.condvar.notify_all();
    }
}

impl StdioOut for PipeWriter {
    fn dup(&self) -> Self {
        self.clone()
    }

    fn write_str(&self, s: &str) -> Result<(), Error> {
        let mut inner = self.shared.inner.lock().unwrap();
        let mut had_newline = false;
        for ch in s.chars() {
            if ch == '\n' {
                let line = std::mem::take(&mut inner.partial);
                inner.lines.push_back(line);
                had_newline = true;
            } else {
                inner.partial.push(ch);
            }
        }
        // Notify readers if we added complete lines
        if had_newline {
            drop(inner);
            self.shared.condvar.notify_one();
        }
        Ok(())
    }
}

/////////////////////////////////////////// FileMetadata ///////////////////////////////////////////

/// Metadata about a file.
#[derive(Clone, Copy, Debug)]
pub struct FileMetadata {
    /// The size of the file in bytes.
    pub size: u64,
}

///////////////////////////////////////////// FileType /////////////////////////////////////////////

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

///////////////////////////////////////////// DirEntry /////////////////////////////////////////////

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

///////////////////////////////////////////// TimeSpec /////////////////////////////////////////////

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

/////////////////////////////////////////// Filesystem /////////////////////////////////////////////

/// A trait for filesystem operations.
pub trait Filesystem {
    /// Returns the root directory path (for display purposes).
    fn root(&self) -> Path<'_>;
    /// Duplicate the filesystem handle.
    fn dup(&self) -> Self
    where
        Self: Sized;
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
    /// Punch a hole in a file by writing NUL bytes at the given offset for the given length.
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
    /// Lists markdown files, returns paths relative to root.
    fn list_markdown_files(&self) -> Result<Vec<String>, Error>;

    // =========================================================================
    // Debug/introspection methods for filesystem debugging
    // =========================================================================

    /// Returns the number of free blocks in the filesystem.
    ///
    /// Returns `None` if the filesystem does not support this operation.
    fn free_blocks(&self) -> Option<u64> {
        None
    }

    /// Returns the total number of blocks in the filesystem's log.
    ///
    /// Returns `None` if the filesystem does not support this operation.
    fn total_blocks(&self) -> Option<u64> {
        None
    }

    /// Returns the filesystem usage as a percentage (0-100).
    ///
    /// Returns `None` if the filesystem does not support this operation.
    fn usage_percent(&self) -> Option<u64> {
        None
    }

    /// Triggers garbage collection/cleaning and returns the number of blocks reclaimed.
    ///
    /// Returns `None` if the filesystem does not support this operation.
    fn clean(&self) -> Option<Result<usize, Error>> {
        None
    }

    /// Returns the entire filesystem tree as a formatted string.
    ///
    /// Returns `None` if the filesystem does not support this operation.
    fn tree(&self) -> Option<String> {
        None
    }
}

///////////////////////////////////////// MutFilesystem ////////////////////////////////////////////

/// A trait for filesystem operations that require mutable access.
///
/// This trait mirrors [`Filesystem`] but uses `&mut self` instead of `&self`,
/// making it suitable for filesystems that need interior mutability without
/// synchronization primitives. Use [`Filesystem`] for thread-safe access patterns.
pub trait MutFilesystem {
    /// Returns the root directory path (for display purposes).
    fn root(&self) -> Path<'_>;
    /// Read a file and return its contents as a string.
    fn read_to_string(&mut self, path: &str) -> Result<String, Error>;
    /// Check if a file exists.
    fn exists(&self, path: &str) -> bool;
    /// Get metadata about a file.
    fn metadata(&self, path: &str) -> Result<FileMetadata, Error>;
    /// Truncate or extend a file to the specified size.
    /// Creates the file if it does not exist.
    fn truncate(&mut self, path: &str, size: u64) -> Result<(), Error>;
    /// Truncate or extend a file to the specified size, but only if it exists.
    /// Returns Ok(false) if the file does not exist, Ok(true) if successful.
    fn truncate_existing(&mut self, path: &str, size: u64) -> Result<bool, Error>;
    /// Punch a hole in a file by writing NUL bytes at the given offset for the given length.
    /// The file must exist. If offset + length exceeds file size, extends the file.
    fn punch_hole(&mut self, path: &str, offset: u64, length: u64) -> Result<(), Error>;
    /// Write a string to a file, creating or overwriting as needed.
    fn write_string(&mut self, path: &str, contents: &str) -> Result<(), Error>;
    /// Append a string to a file, creating if it does not exist.
    fn append_string(&mut self, path: &str, contents: &str) -> Result<(), Error>;
    /// Create a directory. Returns an error if the directory already exists
    /// or if the parent directory does not exist.
    fn mkdir(&mut self, path: &str) -> Result<(), Error>;
    /// Create a directory and all parent directories as needed.
    /// Returns Ok(()) if the directory already exists.
    fn mkdir_all(&mut self, path: &str) -> Result<(), Error>;
    /// Check if a path is a directory.
    fn is_dir(&self, path: &str) -> bool;
    /// Read the contents of a directory.
    fn read_dir(&self, path: &str) -> Result<Vec<(String, DirEntry)>, Error>;
    /// Get detailed information about a file or directory.
    fn stat(&self, path: &str) -> Result<DirEntry, Error>;
    /// Get detailed information about a file or directory without following symlinks.
    fn lstat(&self, path: &str) -> Result<DirEntry, Error>;
    /// Create a symbolic link at linkpath pointing to target.
    fn symlink(&mut self, target: &str, linkpath: &str) -> Result<(), Error>;
    /// Create a hard link at dst pointing to src.
    fn link(&mut self, src: &str, dst: &str) -> Result<(), Error>;
    /// Remove a file or symbolic link.
    fn unlink(&mut self, path: &str) -> Result<(), Error>;
    /// Remove an empty directory.
    fn rmdir(&mut self, path: &str) -> Result<(), Error>;
    /// Read the target of a symbolic link.
    fn readlink(&self, path: &str) -> Result<String, Error>;
    /// Set the access and modification times of a file.
    /// TimeSpec::Now sets to current time, TimeSpec::Omit leaves unchanged.
    fn set_times(&mut self, path: &str, atime: TimeSpec, mtime: TimeSpec) -> Result<(), Error>;
    /// Set the access and modification times of a file without following symlinks.
    fn lset_times(&mut self, path: &str, atime: TimeSpec, mtime: TimeSpec) -> Result<(), Error>;
    /// Create an empty file if it does not exist, without changing times if it does.
    /// Returns true if the file was created, false if it already existed.
    fn create_file(&mut self, path: &str) -> Result<bool, Error>;
    /// Rename a file or directory from src to dst.
    fn rename(&mut self, src: &str, dst: &str) -> Result<(), Error>;
    /// Create a unique temporary file using a template (Xs are replaced).
    /// Returns the actual path created.
    fn mkstemp(&mut self, template: &str) -> Result<String, Error>;
    /// Create a unique temporary directory using a template (Xs are replaced).
    /// Returns the actual path created.
    fn mkdtemp(&mut self, template: &str) -> Result<String, Error>;
    /// Lists markdown files, returns paths relative to root.
    fn list_markdown_files(&self) -> Result<Vec<String>, Error>;

    // =========================================================================
    // Debug/introspection methods for filesystem debugging
    // =========================================================================

    /// Returns the number of free blocks in the filesystem.
    ///
    /// Returns `None` if the filesystem does not support this operation.
    fn free_blocks(&self) -> Option<u64> {
        None
    }

    /// Returns the total number of blocks in the filesystem's log.
    ///
    /// Returns `None` if the filesystem does not support this operation.
    fn total_blocks(&self) -> Option<u64> {
        None
    }

    /// Returns the filesystem usage as a percentage (0-100).
    ///
    /// Returns `None` if the filesystem does not support this operation.
    fn usage_percent(&self) -> Option<u64> {
        None
    }

    /// Triggers garbage collection/cleaning and returns the number of blocks reclaimed.
    ///
    /// Returns `None` if the filesystem does not support this operation.
    fn clean(&mut self) -> Option<Result<usize, Error>> {
        None
    }

    /// Returns the entire filesystem tree as a formatted string.
    ///
    /// Returns `None` if the filesystem does not support this operation.
    fn tree(&self) -> Option<String> {
        None
    }
}

/////////////////////////////////////// SyncMutFilesystem //////////////////////////////////////////

use std::sync::Arc;
use std::sync::Mutex;

/// A wrapper that provides [`Filesystem`] (thread-safe, `&self`) access to a [`MutFilesystem`].
///
/// This type wraps a `MutFilesystem` in `Arc<Mutex<_>>` to provide interior mutability
/// with synchronization, allowing the wrapped filesystem to implement the `Filesystem` trait.
pub struct SyncMutFilesystem<F: MutFilesystem> {
    inner: Arc<Mutex<F>>,
}

impl<F: MutFilesystem> SyncMutFilesystem<F> {
    /// Creates a new `SyncMutFilesystem` wrapping the given filesystem.
    pub fn new(fs: F) -> Self {
        Self {
            inner: Arc::new(Mutex::new(fs)),
        }
    }

    /// Creates a new `SyncMutFilesystem` from an existing `Arc<Mutex<F>>`.
    pub fn from_arc(inner: Arc<Mutex<F>>) -> Self {
        Self { inner }
    }

    /// Returns a clone of the inner `Arc<Mutex<F>>`.
    pub fn inner(&self) -> Arc<Mutex<F>> {
        Arc::clone(&self.inner)
    }

    /// Consumes the wrapper and returns the inner filesystem, if this is the last reference.
    ///
    /// Returns `Err(self)` if there are other references to the inner `Arc`.
    pub fn try_into_inner(self) -> Result<F, Self> {
        match Arc::try_unwrap(self.inner) {
            Ok(mutex) => Ok(mutex.into_inner().expect("mutex poisoned")),
            Err(arc) => Err(Self { inner: arc }),
        }
    }
}

impl<F: MutFilesystem> Clone for SyncMutFilesystem<F> {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

/// The root path for SyncMutFilesystem.
static SYNC_MUT_FS_ROOT: &str = "/";

impl<F: MutFilesystem> Filesystem for SyncMutFilesystem<F> {
    fn root(&self) -> Path<'_> {
        Path::new(SYNC_MUT_FS_ROOT)
    }

    fn dup(&self) -> Self {
        self.clone()
    }

    fn read_to_string(&self, path: &str) -> Result<String, Error> {
        self.inner.lock().unwrap().read_to_string(path)
    }

    fn exists(&self, path: &str) -> bool {
        self.inner.lock().unwrap().exists(path)
    }

    fn metadata(&self, path: &str) -> Result<FileMetadata, Error> {
        self.inner.lock().unwrap().metadata(path)
    }

    fn truncate(&self, path: &str, size: u64) -> Result<(), Error> {
        self.inner.lock().unwrap().truncate(path, size)
    }

    fn truncate_existing(&self, path: &str, size: u64) -> Result<bool, Error> {
        self.inner.lock().unwrap().truncate_existing(path, size)
    }

    fn punch_hole(&self, path: &str, offset: u64, length: u64) -> Result<(), Error> {
        self.inner.lock().unwrap().punch_hole(path, offset, length)
    }

    fn write_string(&self, path: &str, contents: &str) -> Result<(), Error> {
        self.inner.lock().unwrap().write_string(path, contents)
    }

    fn append_string(&self, path: &str, contents: &str) -> Result<(), Error> {
        self.inner.lock().unwrap().append_string(path, contents)
    }

    fn mkdir(&self, path: &str) -> Result<(), Error> {
        self.inner.lock().unwrap().mkdir(path)
    }

    fn mkdir_all(&self, path: &str) -> Result<(), Error> {
        self.inner.lock().unwrap().mkdir_all(path)
    }

    fn is_dir(&self, path: &str) -> bool {
        self.inner.lock().unwrap().is_dir(path)
    }

    fn read_dir(&self, path: &str) -> Result<Vec<(String, DirEntry)>, Error> {
        self.inner.lock().unwrap().read_dir(path)
    }

    fn stat(&self, path: &str) -> Result<DirEntry, Error> {
        self.inner.lock().unwrap().stat(path)
    }

    fn lstat(&self, path: &str) -> Result<DirEntry, Error> {
        self.inner.lock().unwrap().lstat(path)
    }

    fn symlink(&self, target: &str, linkpath: &str) -> Result<(), Error> {
        self.inner.lock().unwrap().symlink(target, linkpath)
    }

    fn link(&self, src: &str, dst: &str) -> Result<(), Error> {
        self.inner.lock().unwrap().link(src, dst)
    }

    fn unlink(&self, path: &str) -> Result<(), Error> {
        self.inner.lock().unwrap().unlink(path)
    }

    fn rmdir(&self, path: &str) -> Result<(), Error> {
        self.inner.lock().unwrap().rmdir(path)
    }

    fn readlink(&self, path: &str) -> Result<String, Error> {
        self.inner.lock().unwrap().readlink(path)
    }

    fn set_times(&self, path: &str, atime: TimeSpec, mtime: TimeSpec) -> Result<(), Error> {
        self.inner.lock().unwrap().set_times(path, atime, mtime)
    }

    fn lset_times(&self, path: &str, atime: TimeSpec, mtime: TimeSpec) -> Result<(), Error> {
        self.inner.lock().unwrap().lset_times(path, atime, mtime)
    }

    fn create_file(&self, path: &str) -> Result<bool, Error> {
        self.inner.lock().unwrap().create_file(path)
    }

    fn rename(&self, src: &str, dst: &str) -> Result<(), Error> {
        self.inner.lock().unwrap().rename(src, dst)
    }

    fn mkstemp(&self, template: &str) -> Result<String, Error> {
        self.inner.lock().unwrap().mkstemp(template)
    }

    fn mkdtemp(&self, template: &str) -> Result<String, Error> {
        self.inner.lock().unwrap().mkdtemp(template)
    }

    fn list_markdown_files(&self) -> Result<Vec<String>, Error> {
        self.inner.lock().unwrap().list_markdown_files()
    }

    fn free_blocks(&self) -> Option<u64> {
        self.inner.lock().unwrap().free_blocks()
    }

    fn total_blocks(&self) -> Option<u64> {
        self.inner.lock().unwrap().total_blocks()
    }

    fn usage_percent(&self) -> Option<u64> {
        self.inner.lock().unwrap().usage_percent()
    }

    fn clean(&self) -> Option<Result<usize, Error>> {
        self.inner.lock().unwrap().clean()
    }

    fn tree(&self) -> Option<String> {
        self.inner.lock().unwrap().tree()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn string_stdio_in_reads_lines() {
        let input = StringStdioIn::new("line1\nline2\nline3");
        assert_eq!(input.read_line().unwrap(), Some("line1".to_string()));
        assert_eq!(input.read_line().unwrap(), Some("line2".to_string()));
        assert_eq!(input.read_line().unwrap(), Some("line3".to_string()));
        assert_eq!(input.read_line().unwrap(), None);
        println!("DEBUG: StringStdioIn reads lines correctly");
    }

    #[test]
    fn string_stdio_out_collects_output() {
        let output = StringStdioOut::new();
        output.write_str("hello").unwrap();
        output.write_line(" world").unwrap();
        assert_eq!(output.into_string(), "hello world\n");
        println!("DEBUG: StringStdioOut collects output correctly");
    }

    #[test]
    fn unit_stdio_in_returns_none() {
        let input: () = ();
        assert_eq!(input.read_line().unwrap(), None);
        println!("DEBUG: unit StdioIn returns None");
    }

    #[test]
    fn pipe_write_then_read() {
        let (reader, writer) = mkpipe();
        StdioOut::write_line(&writer, "hello").unwrap();
        StdioOut::write_line(&writer, "world").unwrap();
        drop(writer);
        assert_eq!(reader.read_line().unwrap(), Some("hello".to_string()));
        assert_eq!(reader.read_line().unwrap(), Some("world".to_string()));
        assert_eq!(reader.read_line().unwrap(), None);
        println!("DEBUG: pipe write then read works");
    }

    #[test]
    fn pipe_partial_line_on_close() {
        let (reader, writer) = mkpipe();
        StdioOut::write_str(&writer, "no newline").unwrap();
        drop(writer);
        assert_eq!(reader.read_line().unwrap(), Some("no newline".to_string()));
        assert_eq!(reader.read_line().unwrap(), None);
        println!("DEBUG: pipe returns partial line on close");
    }

    #[test]
    fn pipe_empty_returns_none_after_close() {
        let (reader, writer) = mkpipe();
        drop(writer);
        assert_eq!(reader.read_line().unwrap(), None);
        println!("DEBUG: empty pipe returns None after writer dropped");
    }

    #[test]
    fn pipe_multiple_writers() {
        let (reader, writer1) = mkpipe();
        let writer2 = writer1.clone();
        StdioOut::write_line(&writer1, "from writer1").unwrap();
        drop(writer1);
        // Reader should not see EOF yet because writer2 is still alive
        assert_eq!(
            reader.read_line().unwrap(),
            Some("from writer1".to_string())
        );
        StdioOut::write_line(&writer2, "from writer2").unwrap();
        drop(writer2);
        assert_eq!(
            reader.read_line().unwrap(),
            Some("from writer2".to_string())
        );
        assert_eq!(reader.read_line().unwrap(), None);
        println!("DEBUG: multiple writers work correctly");
    }

    #[test]
    fn pipe_interleaved_write() {
        let (reader, writer) = mkpipe();
        StdioOut::write_str(&writer, "hel").unwrap();
        StdioOut::write_str(&writer, "lo\nwor").unwrap();
        StdioOut::write_str(&writer, "ld\n").unwrap();
        drop(writer);
        assert_eq!(reader.read_line().unwrap(), Some("hello".to_string()));
        assert_eq!(reader.read_line().unwrap(), Some("world".to_string()));
        assert_eq!(reader.read_line().unwrap(), None);
        println!("DEBUG: interleaved writes work correctly");
    }
}

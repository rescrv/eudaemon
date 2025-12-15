//! Eudaemon types: shared types for the eudaemon ecosystem.
//!
//! This crate provides the core abstractions used across eudaemon crates:
//! - [`Filesystem`] trait for filesystem operations
//! - [`Stdin`], [`Stdout`], [`Stderr`] traits for I/O
//! - Common types like [`FileType`], [`DirEntry`], [`TimeSpec`]

#![deny(missing_docs)]

use std::cell::RefCell;
use std::io::BufRead;
use std::io::Write;
use std::rc::Rc;

use lispdown::Parser;
use lispdown::Vm;

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

////////////////////////////////////////////// Stdin ///////////////////////////////////////////////

/// A trait for types that can serve as standard input.
pub trait Stdin {
    /// Duplicate the stdin handle.
    fn dup(&self) -> Self;
    /// Read a line from stdin, returning None at EOF.
    fn read_line(&self) -> Result<Option<String>, Error>;
}

impl Stdin for () {
    fn dup(&self) -> Self {}

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

////////////////////////////////////////////// Stdout //////////////////////////////////////////////

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
    fn dup(&self) -> Self {}

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

////////////////////////////////////////////// Stderr //////////////////////////////////////////////

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
    fn dup(&self) -> Self {}

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

/////////////////////////////////////// DebugReplConfig ////////////////////////////////////////////

/// Configuration for the debug REPL.
#[derive(Default)]
pub struct DebugReplConfig {
    /// Whether to operate in read-only mode.
    pub read_only: bool,
    /// An optional function to call when the user requests a sync.
    pub sync_fn: Option<Box<dyn Fn()>>,
    /// An optional banner to display at startup.
    pub banner: Option<String>,
}

/////////////////////////////////////////// Filesystem /////////////////////////////////////////////

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

    /// Run an interactive debug REPL for this filesystem.
    ///
    /// The REPL provides commands for exploring and manipulating the filesystem
    /// through a Lisp interface.
    fn debug_repl<SI: Stdin, SO: Stdout, SE: Stderr>(
        &self,
        stdin: SI,
        stdout: SO,
        stderr: SE,
        config: DebugReplConfig,
    ) -> Result<(), Error>
    where
        Self: Sized,
    {
        debug_repl_impl(self, stdin, stdout, stderr, config)
    }
}

////////////////////////////////////////// debug_repl_impl /////////////////////////////////////////

fn print_help<SO: Stdout>(stdout: &SO) -> Result<(), Error> {
    stdout.write_line("Commands:")?;
    stdout.write_line("  :help, :h, :?      Show this help")?;
    stdout.write_line("  :quit, :q, :exit   Exit the REPL")?;
    stdout.write_line("  :tree              Show filesystem tree")?;
    stdout.write_line("  :ls [path]         List directory (default: /)")?;
    stdout.write_line("  :cat path          Show file contents")?;
    stdout.write_line("  :stat path         Show file metadata")?;
    stdout.write_line("  :fns               List available Lisp functions")?;
    stdout.write_line("")?;
    stdout.write_line("Lisp evaluation:")?;
    stdout.write_line("  Type any S-expression to evaluate it.")?;
    Ok(())
}

fn print_functions<SO: Stdout>(stdout: &SO) -> Result<(), Error> {
    stdout.write_line("Filesystem Functions (via Lisp):")?;
    stdout.write_line("  Core functions: first, rest, cons, append, length, nth, list")?;
    stdout.write_line("  Predicates: null?, list?, atom?, empty?, eq?")?;
    stdout.write_line("  Higher-order: map, filter, reduce")?;
    stdout.write_line("  Control: quote, if, let, begin, ->, ->>")?;
    Ok(())
}

fn format_tree<FS: Filesystem>(fs: &FS, path: &str, prefix: &str, is_last: bool) -> String {
    let mut result = String::new();
    let name = if path == "/" {
        "/".to_string()
    } else {
        path.rsplit('/').next().unwrap_or(path).to_string()
    };

    let connector = if path == "/" {
        ""
    } else if is_last {
        "└── "
    } else {
        "├── "
    };

    result.push_str(&format!("{}{}{}\n", prefix, connector, name));

    if fs.is_dir(path)
        && let Ok(entries) = fs.read_dir(path)
    {
        let mut entries: Vec<_> = entries
            .into_iter()
            .filter(|(n, _)| n != "." && n != "..")
            .collect();
        entries.sort_by(|a, b| a.0.cmp(&b.0));

        let child_prefix = if path == "/" {
            String::new()
        } else {
            format!("{}{}   ", prefix, if is_last { " " } else { "│" })
        };

        for (i, (name, _)) in entries.iter().enumerate() {
            let child_path = if path == "/" {
                format!("/{}", name)
            } else {
                format!("{}/{}", path, name)
            };
            let is_last_child = i == entries.len() - 1;
            result.push_str(&format_tree(fs, &child_path, &child_prefix, is_last_child));
        }
    }

    result
}

fn format_ls<FS: Filesystem>(fs: &FS, path: &str) -> String {
    match fs.read_dir(path) {
        Ok(entries) => {
            let mut entries: Vec<_> = entries
                .into_iter()
                .filter(|(n, _)| n != "." && n != "..")
                .collect();
            entries.sort_by(|a, b| a.0.cmp(&b.0));

            let mut result = String::new();
            for (name, entry) in entries {
                let type_char = match entry.file_type {
                    FileType::Directory => 'd',
                    FileType::Symlink => 'l',
                    FileType::RegularFile => '-',
                    FileType::Other => '?',
                };
                result.push_str(&format!("{} {:>8}  {}\n", type_char, entry.size, name));
            }
            result
        }
        Err(e) => format!("Error: {}\n", e),
    }
}

fn format_stat<FS: Filesystem>(fs: &FS, path: &str) -> String {
    match fs.lstat(path) {
        Ok(entry) => {
            let type_str = match entry.file_type {
                FileType::Directory => "directory",
                FileType::Symlink => "symlink",
                FileType::RegularFile => "file",
                FileType::Other => "other",
            };
            format!(
                "type: {}\nsize: {}\natime_ms: {}\nmtime_ms: {}\ndev: {}\nino: {}\n",
                type_str, entry.size, entry.atime_ms, entry.mtime_ms, entry.dev, entry.ino
            )
        }
        Err(e) => format!("Error: {}\n", e),
    }
}

fn handle_command<FS: Filesystem, SO: Stdout>(
    fs: &FS,
    vm: &mut Vm,
    stdout: &SO,
    input: &str,
    _config: &DebugReplConfig,
) -> Result<Option<bool>, Error> {
    let input = input.trim();
    if input.is_empty() {
        return Ok(None);
    }

    if input.starts_with(':') {
        let parts: Vec<&str> = input.split_whitespace().collect();
        match parts[0] {
            ":quit" | ":q" | ":exit" => return Ok(Some(false)),
            ":help" | ":h" | ":?" => print_help(stdout)?,
            ":fns" | ":functions" => print_functions(stdout)?,
            ":tree" => stdout.write_str(&format_tree(fs, "/", "", true))?,
            ":ls" => {
                let path = if parts.len() > 1 { parts[1] } else { "/" };
                stdout.write_str(&format_ls(fs, path))?;
            }
            ":cat" => {
                if parts.len() < 2 {
                    stdout.write_line("Usage: :cat <path>")?;
                } else {
                    match fs.read_to_string(parts[1]) {
                        Ok(content) => stdout.write_str(&content)?,
                        Err(e) => stdout.write_line(&format!("Error: {}", e))?,
                    }
                }
            }
            ":stat" => {
                if parts.len() < 2 {
                    stdout.write_line("Usage: :stat <path>")?;
                } else {
                    stdout.write_str(&format_stat(fs, parts[1]))?;
                }
            }
            _ => stdout.write_line(&format!(
                "Unknown command: {}. Type :help for commands.",
                parts[0]
            ))?,
        }
        return Ok(Some(true));
    }

    // Evaluate S-expression
    let mut parser = Parser::new(input);
    match parser.parse() {
        Ok(expr) => match vm.eval(&expr) {
            Ok(result) => stdout.write_line(&result.to_string())?,
            Err(e) => stdout.write_line(&format!("Error: {}", e))?,
        },
        Err(e) => stdout.write_line(&format!("Parse error: {}", e))?,
    }

    Ok(Some(true))
}

fn debug_repl_impl<FS: Filesystem, SI: Stdin, SO: Stdout, SE: Stderr>(
    fs: &FS,
    stdin: SI,
    stdout: SO,
    _stderr: SE,
    config: DebugReplConfig,
) -> Result<(), Error> {
    let mut vm = Vm::new();
    vm.register_builtins();
    vm.register_json_builtins();

    if let Some(ref banner) = config.banner {
        stdout.write_line(banner)?;
    } else {
        stdout.write_line("eudaemon filesystem debugger")?;
    }
    if config.read_only {
        stdout.write_line("Mode: read-only")?;
    }
    stdout.write_line("Type :help for commands, :quit to exit")?;
    stdout.write_line("")?;

    loop {
        stdout.write_str("fs> ")?;
        match stdin.read_line()? {
            Some(line) => match handle_command(fs, &mut vm, &stdout, &line, &config)? {
                Some(true) => continue,
                Some(false) => break,
                None => continue,
            },
            None => {
                stdout.write_line("^D")?;
                break;
            }
        }
    }

    if !config.read_only
        && let Some(sync) = config.sync_fn
    {
        sync();
        stdout.write_line("Changes synced.")?;
    }

    Ok(())
}

///////////////////////////////////////// RealFilesystem ///////////////////////////////////////////

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

        let mut file = OpenOptions::new()
            .write(true)
            .read(true)
            .open(path)
            .map_err(Error::Io)?;

        let file_len = file.metadata().map_err(Error::Io)?.len();
        let end = offset.saturating_add(length);

        if end > file_len {
            file.set_len(end).map_err(Error::Io)?;
        }

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
        let metadata = std::fs::symlink_metadata(path).map_err(Error::Io)?;
        if metadata.is_symlink() {
            return Ok(());
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn string_stdin_reads_lines() {
        let stdin = StringStdin::new("line1\nline2\nline3");
        assert_eq!(stdin.read_line().unwrap(), Some("line1".to_string()));
        assert_eq!(stdin.read_line().unwrap(), Some("line2".to_string()));
        assert_eq!(stdin.read_line().unwrap(), Some("line3".to_string()));
        assert_eq!(stdin.read_line().unwrap(), None);
        println!("DEBUG: StringStdin reads lines correctly");
    }

    #[test]
    fn string_stdout_collects_output() {
        let stdout = StringStdout::new();
        stdout.write_str("hello").unwrap();
        stdout.write_line(" world").unwrap();
        assert_eq!(stdout.into_string(), "hello world\n");
        println!("DEBUG: StringStdout collects output correctly");
    }

    #[test]
    fn string_stderr_collects_output() {
        let stderr = StringStderr::new();
        stderr.write_str("error: ").unwrap();
        stderr.write_line("something went wrong").unwrap();
        assert_eq!(stderr.into_string(), "error: something went wrong\n");
        println!("DEBUG: StringStderr collects output correctly");
    }

    #[test]
    fn unit_stdin_returns_none() {
        let stdin = ();
        assert_eq!(stdin.read_line().unwrap(), None);
        println!("DEBUG: unit stdin returns None");
    }

    #[test]
    fn debug_repl_quit_command() {
        let fs = RealFilesystem;
        let stdin = StringStdin::new(":quit");
        let stdout = StringStdout::new();
        let stderr = StringStderr::new();
        let config = DebugReplConfig::default();

        fs.debug_repl(stdin, stdout.dup(), stderr, config).unwrap();

        let output = stdout.into_string();
        assert!(output.contains("eudaemon filesystem debugger"));
        println!("DEBUG: debug_repl handles :quit command");
    }

    #[test]
    fn debug_repl_help_command() {
        let fs = RealFilesystem;
        let stdin = StringStdin::new(":help\n:quit");
        let stdout = StringStdout::new();
        let stderr = StringStderr::new();
        let config = DebugReplConfig::default();

        fs.debug_repl(stdin, stdout.dup(), stderr, config).unwrap();

        let output = stdout.into_string();
        assert!(output.contains(":tree"));
        assert!(output.contains(":ls"));
        println!("DEBUG: debug_repl handles :help command");
    }
}

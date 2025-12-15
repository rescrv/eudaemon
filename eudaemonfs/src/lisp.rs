//! Lisp interface for the Log-structured File System.
//!
//! This module provides a homoiconic representation of the filesystem as S-expressions,
//! enabling programmatic manipulation of filesystem state through Lisp programs.
//!
//! # S-expression Representation
//!
//! The filesystem is represented as a tree of tagged S-expressions:
//!
//! ```text
//! (fs
//!   (dir "/"
//!     (file "hello.txt" (size 13) (mtime 1234567890))
//!     (dir "subdir"
//!       (file "nested.txt" (size 100) (mtime 1234567891)))
//!     (symlink "link" (target "/hello.txt"))))
//! ```
//!
//! # Available Operations
//!
//! When registered with a lispdown VM, the following builtins become available:
//!
//! - `(lfs-stat path)` - Get file metadata as an S-expression
//! - `(lfs-read path)` - Read file contents as a string
//! - `(lfs-write path content)` - Write string content to a file
//! - `(lfs-mkdir path)` - Create a directory
//! - `(lfs-remove path)` - Remove a file
//! - `(lfs-rmdir path)` - Remove an empty directory
//! - `(lfs-readdir path)` - List directory contents
//! - `(lfs-exists? path)` - Check if a path exists
//! - `(lfs-symlink target linkpath)` - Create a symbolic link
//! - `(lfs-readlink path)` - Read symlink target
//! - `(lfs-rename src dst)` - Rename a file or directory
//! - `(lfs-tree)` - Get the entire filesystem as an S-expression tree
//! - `(lfs-free-blocks)` - Get the number of free blocks
//! - `(lfs-usage-percent)` - Get the filesystem usage percentage

use std::path::Path;
use std::sync::Arc;
use std::sync::Mutex;

use lispdown::{Filesystem, SError, SExpr, SResult, Vm};

use crate::{BlockDevice, DeviceId, FileType, Lfs, MemoryBlockDevice, StatInfo};

/// Extracts a string from an S-expression atom.
///
/// Handles both quoted strings (with surrounding `"`) and unquoted atoms.
fn extract_string(expr: &SExpr) -> String {
    match expr {
        SExpr::Atom(s) => {
            if s.starts_with('"') && s.ends_with('"') && s.len() >= 2 {
                s[1..s.len() - 1].to_string()
            } else {
                s.clone()
            }
        }
        SExpr::List(_) => expr.to_string(),
    }
}

/// Creates a quoted string atom.
fn string_atom(s: &str) -> SExpr {
    SExpr::Atom(format!(
        "\"{}\"",
        s.replace('\\', "\\\\").replace('"', "\\\"")
    ))
}

/// A wrapper around Lfs that provides a Lisp-friendly interface.
///
/// This type holds the Lfs instance in a Mutex to allow mutation through
/// the `&self` interface required by lispdown's builtin function signature.
/// Using Arc<Mutex<_>> enables thread-safe sharing.
pub struct LfsLisp<D: BlockDevice, T: Fn() -> i64> {
    lfs: Mutex<Lfs<D, T>>,
}

impl<D: BlockDevice, T: Fn() -> i64> LfsLisp<D, T> {
    /// Creates a new LfsLisp wrapper around an existing Lfs instance.
    pub fn new(lfs: Lfs<D, T>) -> Self {
        Self {
            lfs: Mutex::new(lfs),
        }
    }

    /// Consumes the wrapper and returns the underlying Lfs.
    pub fn into_inner(self) -> Lfs<D, T> {
        self.lfs.into_inner().expect("mutex poisoned")
    }

    /// Gets file or directory metadata as an S-expression.
    ///
    /// Returns: `(stat (type TYPE) (size SIZE) (mtime MTIME) (atime ATIME) (ino INO) (links LINKS))`
    pub fn stat(&self, path: &str) -> SResult<SExpr> {
        let lfs = self.lfs.lock().unwrap();
        let info = lfs.stat(path).map_err(lfs_error_to_serror)?;
        Ok(stat_info_to_sexpr(&info))
    }

    /// Gets metadata without following the final symlink.
    pub fn lstat(&self, path: &str) -> SResult<SExpr> {
        let lfs = self.lfs.lock().unwrap();
        let info = lfs.lstat(path).map_err(lfs_error_to_serror)?;
        Ok(stat_info_to_sexpr(&info))
    }

    /// Reads file contents as a string.
    pub fn read(&self, path: &str) -> SResult<String> {
        let mut lfs = self.lfs.lock().unwrap();
        let bytes = lfs.read_file(path).map_err(lfs_error_to_serror)?;
        String::from_utf8(bytes).map_err(|e| {
            SError::new("lfs")
                .with_code("invalid-utf8")
                .with_message("File contains invalid UTF-8")
                .with_string_field("error", &e.to_string())
        })
    }

    /// Writes string content to a file.
    pub fn write(&self, path: &str, content: &str) -> SResult<()> {
        let mut lfs = self.lfs.lock().unwrap();
        lfs.write_file(path, content.as_bytes())
            .map_err(lfs_error_to_serror)
    }

    /// Creates a directory.
    pub fn mkdir(&self, path: &str) -> SResult<()> {
        let mut lfs = self.lfs.lock().unwrap();
        lfs.mkdir(path).map_err(lfs_error_to_serror)
    }

    /// Creates a directory and all parent directories.
    pub fn mkdir_all(&self, path: &str) -> SResult<()> {
        let mut lfs = self.lfs.lock().unwrap();
        lfs.mkdir_all(path).map_err(lfs_error_to_serror)
    }

    /// Removes a file.
    pub fn remove(&self, path: &str) -> SResult<()> {
        let mut lfs = self.lfs.lock().unwrap();
        lfs.remove(path).map_err(lfs_error_to_serror)
    }

    /// Removes an empty directory.
    pub fn rmdir(&self, path: &str) -> SResult<()> {
        let mut lfs = self.lfs.lock().unwrap();
        lfs.rmdir(path).map_err(lfs_error_to_serror)
    }

    /// Lists directory contents.
    ///
    /// Returns: `((name1 stat1) (name2 stat2) ...)`
    pub fn readdir(&self, path: &str) -> SResult<SExpr> {
        let lfs = self.lfs.lock().unwrap();
        let entries = lfs.read_dir(path).map_err(lfs_error_to_serror)?;
        let items: Vec<SExpr> = entries
            .into_iter()
            .map(|(name, info)| SExpr::List(vec![string_atom(&name), stat_info_to_sexpr(&info)]))
            .collect();
        Ok(SExpr::List(items))
    }

    /// Checks if a path exists.
    pub fn exists(&self, path: &str) -> bool {
        let lfs = self.lfs.lock().unwrap();
        lfs.exists(path)
    }

    /// Checks if a path is a directory.
    pub fn is_dir(&self, path: &str) -> bool {
        let lfs = self.lfs.lock().unwrap();
        lfs.is_dir(path)
    }

    /// Creates a symbolic link.
    pub fn symlink(&self, target: &str, linkpath: &str) -> SResult<()> {
        let mut lfs = self.lfs.lock().unwrap();
        lfs.symlink(target, linkpath).map_err(lfs_error_to_serror)
    }

    /// Reads the target of a symbolic link.
    pub fn readlink(&self, path: &str) -> SResult<String> {
        let lfs = self.lfs.lock().unwrap();
        lfs.readlink(path).map_err(lfs_error_to_serror)
    }

    /// Creates a hard link.
    pub fn link(&self, src: &str, dst: &str) -> SResult<()> {
        let mut lfs = self.lfs.lock().unwrap();
        lfs.link(src, dst).map_err(lfs_error_to_serror)
    }

    /// Renames a file or directory.
    pub fn rename(&self, src: &str, dst: &str) -> SResult<()> {
        let mut lfs = self.lfs.lock().unwrap();
        lfs.rename(src, dst).map_err(lfs_error_to_serror)
    }

    /// Appends content to a file.
    pub fn append(&self, path: &str, content: &str) -> SResult<()> {
        let mut lfs = self.lfs.lock().unwrap();
        lfs.append_file(path, content.as_bytes())
            .map_err(lfs_error_to_serror)
    }

    /// Gets the entire filesystem tree as an S-expression.
    ///
    /// Returns a recursive representation of the filesystem starting from root.
    pub fn tree(&self) -> SResult<SExpr> {
        let lfs = self.lfs.lock().unwrap();
        let root_tree = tree_recursive(&lfs, "/")?;
        Ok(SExpr::List(vec![SExpr::Atom("fs".to_string()), root_tree]))
    }

    /// Returns the number of free blocks.
    pub fn free_blocks(&self) -> u64 {
        self.lfs.lock().unwrap().free_blocks()
    }

    /// Returns the total number of log blocks.
    pub fn total_log_blocks(&self) -> u64 {
        self.lfs.lock().unwrap().total_log_blocks()
    }

    /// Returns the usage percentage (0-100).
    pub fn usage_percent(&self) -> u64 {
        self.lfs.lock().unwrap().usage_percent()
    }

    /// Performs garbage collection.
    pub fn clean(&self) -> SResult<usize> {
        let mut lfs = self.lfs.lock().unwrap();
        lfs.clean().map_err(lfs_error_to_serror)
    }
}

impl<T: Fn() -> i64> LfsLisp<MemoryBlockDevice, T> {
    /// Creates a new LfsLisp with an in-memory filesystem.
    pub fn from_vec(data: Vec<u8>, dev: DeviceId, time_source: T) -> crate::Result<Self> {
        let lfs = Lfs::from_vec(data, dev, time_source)?;
        Ok(Self::new(lfs))
    }

    /// Opens an existing LfsLisp from an in-memory buffer.
    pub fn open_vec(data: Vec<u8>, dev: DeviceId, time_source: T) -> crate::Result<Self> {
        let lfs = Lfs::open_vec(data, dev, time_source)?;
        Ok(Self::new(lfs))
    }

    /// Returns the underlying data buffer.
    pub fn data(&self) -> Vec<u8> {
        self.lfs.lock().unwrap().data().to_vec()
    }

    /// Consumes the wrapper and returns the underlying data buffer.
    pub fn into_data(self) -> Vec<u8> {
        self.lfs.into_inner().expect("mutex poisoned").into_inner()
    }
}

/// Recursively builds the filesystem tree starting from a path.
fn tree_recursive<D: BlockDevice, T: Fn() -> i64>(lfs: &Lfs<D, T>, path: &str) -> SResult<SExpr> {
    let info = lfs.stat(path).map_err(lfs_error_to_serror)?;
    let name = if path == "/" {
        "/".to_string()
    } else {
        Path::new(path)
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| path.to_string())
    };

    match info.file_type {
        FileType::Directory => {
            let entries = lfs.read_dir(path).map_err(lfs_error_to_serror)?;
            let mut children: Vec<SExpr> = Vec::new();
            children.push(SExpr::Atom("dir".to_string()));
            children.push(string_atom(&name));

            for (child_name, child_info) in entries {
                // Skip . and .. entries
                if child_name == "." || child_name == ".." {
                    continue;
                }
                let child_path = if path == "/" {
                    format!("/{}", child_name)
                } else {
                    format!("{}/{}", path, child_name)
                };
                match child_info.file_type {
                    FileType::Directory => {
                        children.push(tree_recursive(lfs, &child_path)?);
                    }
                    FileType::RegularFile => {
                        children.push(SExpr::List(vec![
                            SExpr::Atom("file".to_string()),
                            string_atom(&child_name),
                            SExpr::List(vec![
                                SExpr::Atom("size".to_string()),
                                SExpr::Atom(child_info.size.to_string()),
                            ]),
                            SExpr::List(vec![
                                SExpr::Atom("mtime".to_string()),
                                SExpr::Atom(child_info.mtime_ms.to_string()),
                            ]),
                        ]));
                    }
                    FileType::Symlink => {
                        let target = lfs.readlink(&child_path).unwrap_or_default();
                        children.push(SExpr::List(vec![
                            SExpr::Atom("symlink".to_string()),
                            string_atom(&child_name),
                            SExpr::List(vec![
                                SExpr::Atom("target".to_string()),
                                string_atom(&target),
                            ]),
                        ]));
                    }
                    FileType::Other => {
                        children.push(SExpr::List(vec![
                            SExpr::Atom("other".to_string()),
                            string_atom(&child_name),
                        ]));
                    }
                }
            }
            Ok(SExpr::List(children))
        }
        FileType::RegularFile => Ok(SExpr::List(vec![
            SExpr::Atom("file".to_string()),
            string_atom(&name),
            SExpr::List(vec![
                SExpr::Atom("size".to_string()),
                SExpr::Atom(info.size.to_string()),
            ]),
            SExpr::List(vec![
                SExpr::Atom("mtime".to_string()),
                SExpr::Atom(info.mtime_ms.to_string()),
            ]),
        ])),
        FileType::Symlink => {
            let target = lfs.readlink(path).unwrap_or_default();
            Ok(SExpr::List(vec![
                SExpr::Atom("symlink".to_string()),
                string_atom(&name),
                SExpr::List(vec![
                    SExpr::Atom("target".to_string()),
                    string_atom(&target),
                ]),
            ]))
        }
        FileType::Other => Ok(SExpr::List(vec![
            SExpr::Atom("other".to_string()),
            string_atom(&name),
        ])),
    }
}

/// Converts an Lfs error to an SError.
fn lfs_error_to_serror(err: crate::Error) -> SError {
    let code = match err {
        crate::Error::CorruptFilesystem => "corrupt-filesystem",
        crate::Error::NoSpace => "no-space",
        crate::Error::NotFound => "not-found",
        crate::Error::InvalidFd => "invalid-fd",
        crate::Error::FilenameTooLong => "filename-too-long",
        crate::Error::AlreadyExists => "already-exists",
        crate::Error::InvalidOffset => "invalid-offset",
        crate::Error::FileTooLarge => "file-too-large",
        crate::Error::BufferTooSmall => "buffer-too-small",
        crate::Error::InvalidArgument => "invalid-argument",
        crate::Error::NotOpen => "not-open",
        crate::Error::IsDirectory => "is-directory",
        crate::Error::NotADirectory => "not-a-directory",
        crate::Error::DirectoryNotEmpty => "directory-not-empty",
    };
    SError::new("lfs")
        .with_code(code)
        .with_message(&format!("{:?}", err))
}

/// Converts StatInfo to an S-expression.
fn stat_info_to_sexpr(info: &StatInfo) -> SExpr {
    let type_str = match info.file_type {
        FileType::RegularFile => "file",
        FileType::Directory => "dir",
        FileType::Symlink => "symlink",
        FileType::Other => "other",
    };

    SExpr::List(vec![
        SExpr::Atom("stat".to_string()),
        SExpr::List(vec![
            SExpr::Atom("type".to_string()),
            SExpr::Atom(type_str.to_string()),
        ]),
        SExpr::List(vec![
            SExpr::Atom("size".to_string()),
            SExpr::Atom(info.size.to_string()),
        ]),
        SExpr::List(vec![
            SExpr::Atom("mtime".to_string()),
            SExpr::Atom(info.mtime_ms.to_string()),
        ]),
        SExpr::List(vec![
            SExpr::Atom("atime".to_string()),
            SExpr::Atom(info.atime_ms.to_string()),
        ]),
        SExpr::List(vec![
            SExpr::Atom("ino".to_string()),
            SExpr::Atom(info.ino.to_string()),
        ]),
        SExpr::List(vec![
            SExpr::Atom("links".to_string()),
            SExpr::Atom(info.link_count.to_string()),
        ]),
    ])
}

/// Shared state for LfsLisp builtins.
///
/// This type wraps LfsLisp in an Arc so it can be shared across multiple builtin
/// function closures while still providing interior mutability and thread safety.
pub struct LfsLispState<D: BlockDevice, T: Fn() -> i64> {
    lfs: Arc<LfsLisp<D, T>>,
}

impl<D: BlockDevice, T: Fn() -> i64> LfsLispState<D, T> {
    /// Creates a new LfsLispState.
    pub fn new(lfs: LfsLisp<D, T>) -> Self {
        Self { lfs: Arc::new(lfs) }
    }

    /// Returns a clone of the inner Arc for sharing.
    pub fn share(&self) -> Arc<LfsLisp<D, T>> {
        Arc::clone(&self.lfs)
    }
}

impl<D: BlockDevice + Send + Sync + 'static, T: Fn() -> i64 + Send + Sync + 'static>
    LfsLispState<D, T>
{
    /// Registers all LFS builtins with the given VM.
    ///
    /// This registers the following functions:
    /// - `lfs-stat`, `lfs-lstat`, `lfs-read`, `lfs-write`
    /// - `lfs-mkdir`, `lfs-mkdir-all`, `lfs-remove`, `lfs-rmdir`
    /// - `lfs-readdir`, `lfs-exists?`, `lfs-is-dir?`
    /// - `lfs-symlink`, `lfs-readlink`, `lfs-link`, `lfs-rename`
    /// - `lfs-append`, `lfs-tree`
    /// - `lfs-free-blocks`, `lfs-total-blocks`, `lfs-usage-percent`, `lfs-clean`
    pub fn register_builtins(&self, vm: &mut Vm) {
        // We use a thread_local to store the LfsLisp reference since BuiltinFn
        // is a function pointer that can't capture state. This is a workaround
        // for the lispdown API design.
        //
        // TODO(claude): A better approach would be to modify lispdown to support
        // stateful builtins, but that's outside the scope of this change.
        let lfs = self.share();
        set_thread_local_lfs(lfs);

        // Register all builtins
        vm.def_fn("lfs-stat", builtin_lfs_stat);
        vm.def_fn("lfs-lstat", builtin_lfs_lstat);
        vm.def_fn("lfs-read", builtin_lfs_read);
        vm.def_fn("lfs-write", builtin_lfs_write);
        vm.def_fn("lfs-mkdir", builtin_lfs_mkdir);
        vm.def_fn("lfs-mkdir-all", builtin_lfs_mkdir_all);
        vm.def_fn("lfs-remove", builtin_lfs_remove);
        vm.def_fn("lfs-rmdir", builtin_lfs_rmdir);
        vm.def_fn("lfs-readdir", builtin_lfs_readdir);
        vm.def_fn("lfs-exists?", builtin_lfs_exists);
        vm.def_fn("lfs-is-dir?", builtin_lfs_is_dir);
        vm.def_fn("lfs-symlink", builtin_lfs_symlink);
        vm.def_fn("lfs-readlink", builtin_lfs_readlink);
        vm.def_fn("lfs-link", builtin_lfs_link);
        vm.def_fn("lfs-rename", builtin_lfs_rename);
        vm.def_fn("lfs-append", builtin_lfs_append);
        vm.def_fn("lfs-tree", builtin_lfs_tree);
        vm.def_fn("lfs-free-blocks", builtin_lfs_free_blocks);
        vm.def_fn("lfs-total-blocks", builtin_lfs_total_blocks);
        vm.def_fn("lfs-usage-percent", builtin_lfs_usage_percent);
        vm.def_fn("lfs-clean", builtin_lfs_clean);
    }
}

// Thread-local storage for the LfsLisp instance.
// This is necessary because lispdown's BuiltinFn is a function pointer
// that cannot capture state.
thread_local! {
    static THREAD_LFS: std::cell::RefCell<Option<Arc<dyn LfsLispOps + Send + Sync>>> = const { std::cell::RefCell::new(None) };
}

/// Trait for type-erased LfsLisp operations.
trait LfsLispOps {
    fn stat(&self, path: &str) -> SResult<SExpr>;
    fn lstat(&self, path: &str) -> SResult<SExpr>;
    fn read(&self, path: &str) -> SResult<String>;
    fn write(&self, path: &str, content: &str) -> SResult<()>;
    fn mkdir(&self, path: &str) -> SResult<()>;
    fn mkdir_all(&self, path: &str) -> SResult<()>;
    fn remove(&self, path: &str) -> SResult<()>;
    fn rmdir(&self, path: &str) -> SResult<()>;
    fn readdir(&self, path: &str) -> SResult<SExpr>;
    fn exists(&self, path: &str) -> bool;
    fn is_dir(&self, path: &str) -> bool;
    fn symlink(&self, target: &str, linkpath: &str) -> SResult<()>;
    fn readlink(&self, path: &str) -> SResult<String>;
    fn link(&self, src: &str, dst: &str) -> SResult<()>;
    fn rename(&self, src: &str, dst: &str) -> SResult<()>;
    fn append(&self, path: &str, content: &str) -> SResult<()>;
    fn tree(&self) -> SResult<SExpr>;
    fn free_blocks(&self) -> u64;
    fn total_log_blocks(&self) -> u64;
    fn usage_percent(&self) -> u64;
    fn clean(&self) -> SResult<usize>;
}

impl<D: BlockDevice, T: Fn() -> i64> LfsLispOps for LfsLisp<D, T> {
    fn stat(&self, path: &str) -> SResult<SExpr> {
        LfsLisp::stat(self, path)
    }
    fn lstat(&self, path: &str) -> SResult<SExpr> {
        LfsLisp::lstat(self, path)
    }
    fn read(&self, path: &str) -> SResult<String> {
        LfsLisp::read(self, path)
    }
    fn write(&self, path: &str, content: &str) -> SResult<()> {
        LfsLisp::write(self, path, content)
    }
    fn mkdir(&self, path: &str) -> SResult<()> {
        LfsLisp::mkdir(self, path)
    }
    fn mkdir_all(&self, path: &str) -> SResult<()> {
        LfsLisp::mkdir_all(self, path)
    }
    fn remove(&self, path: &str) -> SResult<()> {
        LfsLisp::remove(self, path)
    }
    fn rmdir(&self, path: &str) -> SResult<()> {
        LfsLisp::rmdir(self, path)
    }
    fn readdir(&self, path: &str) -> SResult<SExpr> {
        LfsLisp::readdir(self, path)
    }
    fn exists(&self, path: &str) -> bool {
        LfsLisp::exists(self, path)
    }
    fn is_dir(&self, path: &str) -> bool {
        LfsLisp::is_dir(self, path)
    }
    fn symlink(&self, target: &str, linkpath: &str) -> SResult<()> {
        LfsLisp::symlink(self, target, linkpath)
    }
    fn readlink(&self, path: &str) -> SResult<String> {
        LfsLisp::readlink(self, path)
    }
    fn link(&self, src: &str, dst: &str) -> SResult<()> {
        LfsLisp::link(self, src, dst)
    }
    fn rename(&self, src: &str, dst: &str) -> SResult<()> {
        LfsLisp::rename(self, src, dst)
    }
    fn append(&self, path: &str, content: &str) -> SResult<()> {
        LfsLisp::append(self, path, content)
    }
    fn tree(&self) -> SResult<SExpr> {
        LfsLisp::tree(self)
    }
    fn free_blocks(&self) -> u64 {
        LfsLisp::free_blocks(self)
    }
    fn total_log_blocks(&self) -> u64 {
        LfsLisp::total_log_blocks(self)
    }
    fn usage_percent(&self) -> u64 {
        LfsLisp::usage_percent(self)
    }
    fn clean(&self) -> SResult<usize> {
        LfsLisp::clean(self)
    }
}

/// Sets the thread-local LfsLisp instance.
fn set_thread_local_lfs<
    D: BlockDevice + Send + Sync + 'static,
    T: Fn() -> i64 + Send + Sync + 'static,
>(
    lfs: Arc<LfsLisp<D, T>>,
) {
    THREAD_LFS.with(|cell| {
        *cell.borrow_mut() = Some(lfs as Arc<dyn LfsLispOps + Send + Sync>);
    });
}

/// Gets the thread-local LfsLisp instance.
fn get_thread_local_lfs() -> SResult<Arc<dyn LfsLispOps + Send + Sync>> {
    THREAD_LFS.with(|cell| {
        cell.borrow().clone().ok_or_else(|| {
            SError::new("lfs")
                .with_code("no-filesystem")
                .with_message("No LFS filesystem is registered with this VM")
        })
    })
}

// ============================================================================
// Builtin functions
// ============================================================================

fn builtin_lfs_stat(_vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 1 {
        return Err(SError::new("lfs-stat")
            .with_code("wrong-argument-count")
            .with_message("lfs-stat requires exactly one argument: path"));
    }
    let path = extract_string(&args[0]);
    let lfs = get_thread_local_lfs()?;
    lfs.stat(&path)
}

fn builtin_lfs_lstat(_vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 1 {
        return Err(SError::new("lfs-lstat")
            .with_code("wrong-argument-count")
            .with_message("lfs-lstat requires exactly one argument: path"));
    }
    let path = extract_string(&args[0]);
    let lfs = get_thread_local_lfs()?;
    lfs.lstat(&path)
}

fn builtin_lfs_read(_vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 1 {
        return Err(SError::new("lfs-read")
            .with_code("wrong-argument-count")
            .with_message("lfs-read requires exactly one argument: path"));
    }
    let path = extract_string(&args[0]);
    let lfs = get_thread_local_lfs()?;
    let content = lfs.read(&path)?;
    Ok(string_atom(&content))
}

fn builtin_lfs_write(_vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 2 {
        return Err(SError::new("lfs-write")
            .with_code("wrong-argument-count")
            .with_message("lfs-write requires exactly two arguments: path and content"));
    }
    let path = extract_string(&args[0]);
    let content = extract_string(&args[1]);
    let lfs = get_thread_local_lfs()?;
    lfs.write(&path, &content)?;
    Ok(string_atom(&path))
}

fn builtin_lfs_mkdir(_vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 1 {
        return Err(SError::new("lfs-mkdir")
            .with_code("wrong-argument-count")
            .with_message("lfs-mkdir requires exactly one argument: path"));
    }
    let path = extract_string(&args[0]);
    let lfs = get_thread_local_lfs()?;
    lfs.mkdir(&path)?;
    Ok(string_atom(&path))
}

fn builtin_lfs_mkdir_all(_vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 1 {
        return Err(SError::new("lfs-mkdir-all")
            .with_code("wrong-argument-count")
            .with_message("lfs-mkdir-all requires exactly one argument: path"));
    }
    let path = extract_string(&args[0]);
    let lfs = get_thread_local_lfs()?;
    lfs.mkdir_all(&path)?;
    Ok(string_atom(&path))
}

fn builtin_lfs_remove(_vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 1 {
        return Err(SError::new("lfs-remove")
            .with_code("wrong-argument-count")
            .with_message("lfs-remove requires exactly one argument: path"));
    }
    let path = extract_string(&args[0]);
    let lfs = get_thread_local_lfs()?;
    lfs.remove(&path)?;
    Ok(SExpr::Atom("#t".to_string()))
}

fn builtin_lfs_rmdir(_vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 1 {
        return Err(SError::new("lfs-rmdir")
            .with_code("wrong-argument-count")
            .with_message("lfs-rmdir requires exactly one argument: path"));
    }
    let path = extract_string(&args[0]);
    let lfs = get_thread_local_lfs()?;
    lfs.rmdir(&path)?;
    Ok(SExpr::Atom("#t".to_string()))
}

fn builtin_lfs_readdir(_vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 1 {
        return Err(SError::new("lfs-readdir")
            .with_code("wrong-argument-count")
            .with_message("lfs-readdir requires exactly one argument: path"));
    }
    let path = extract_string(&args[0]);
    let lfs = get_thread_local_lfs()?;
    lfs.readdir(&path)
}

fn builtin_lfs_exists(_vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 1 {
        return Err(SError::new("lfs-exists?")
            .with_code("wrong-argument-count")
            .with_message("lfs-exists? requires exactly one argument: path"));
    }
    let path = extract_string(&args[0]);
    let lfs = get_thread_local_lfs()?;
    let exists = lfs.exists(&path);
    Ok(SExpr::Atom(if exists { "#t" } else { "#f" }.to_string()))
}

fn builtin_lfs_is_dir(_vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 1 {
        return Err(SError::new("lfs-is-dir?")
            .with_code("wrong-argument-count")
            .with_message("lfs-is-dir? requires exactly one argument: path"));
    }
    let path = extract_string(&args[0]);
    let lfs = get_thread_local_lfs()?;
    let is_dir = lfs.is_dir(&path);
    Ok(SExpr::Atom(if is_dir { "#t" } else { "#f" }.to_string()))
}

fn builtin_lfs_symlink(_vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 2 {
        return Err(SError::new("lfs-symlink")
            .with_code("wrong-argument-count")
            .with_message("lfs-symlink requires exactly two arguments: target and linkpath"));
    }
    let target = extract_string(&args[0]);
    let linkpath = extract_string(&args[1]);
    let lfs = get_thread_local_lfs()?;
    lfs.symlink(&target, &linkpath)?;
    Ok(string_atom(&linkpath))
}

fn builtin_lfs_readlink(_vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 1 {
        return Err(SError::new("lfs-readlink")
            .with_code("wrong-argument-count")
            .with_message("lfs-readlink requires exactly one argument: path"));
    }
    let path = extract_string(&args[0]);
    let lfs = get_thread_local_lfs()?;
    let target = lfs.readlink(&path)?;
    Ok(string_atom(&target))
}

fn builtin_lfs_link(_vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 2 {
        return Err(SError::new("lfs-link")
            .with_code("wrong-argument-count")
            .with_message("lfs-link requires exactly two arguments: src and dst"));
    }
    let src = extract_string(&args[0]);
    let dst = extract_string(&args[1]);
    let lfs = get_thread_local_lfs()?;
    lfs.link(&src, &dst)?;
    Ok(string_atom(&dst))
}

fn builtin_lfs_rename(_vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 2 {
        return Err(SError::new("lfs-rename")
            .with_code("wrong-argument-count")
            .with_message("lfs-rename requires exactly two arguments: src and dst"));
    }
    let src = extract_string(&args[0]);
    let dst = extract_string(&args[1]);
    let lfs = get_thread_local_lfs()?;
    lfs.rename(&src, &dst)?;
    Ok(string_atom(&dst))
}

fn builtin_lfs_append(_vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 2 {
        return Err(SError::new("lfs-append")
            .with_code("wrong-argument-count")
            .with_message("lfs-append requires exactly two arguments: path and content"));
    }
    let path = extract_string(&args[0]);
    let content = extract_string(&args[1]);
    let lfs = get_thread_local_lfs()?;
    lfs.append(&path, &content)?;
    Ok(string_atom(&path))
}

fn builtin_lfs_tree(_vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
    if !args.is_empty() {
        return Err(SError::new("lfs-tree")
            .with_code("wrong-argument-count")
            .with_message("lfs-tree takes no arguments"));
    }
    let lfs = get_thread_local_lfs()?;
    lfs.tree()
}

fn builtin_lfs_free_blocks(_vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
    if !args.is_empty() {
        return Err(SError::new("lfs-free-blocks")
            .with_code("wrong-argument-count")
            .with_message("lfs-free-blocks takes no arguments"));
    }
    let lfs = get_thread_local_lfs()?;
    Ok(SExpr::Atom(lfs.free_blocks().to_string()))
}

fn builtin_lfs_total_blocks(_vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
    if !args.is_empty() {
        return Err(SError::new("lfs-total-blocks")
            .with_code("wrong-argument-count")
            .with_message("lfs-total-blocks takes no arguments"));
    }
    let lfs = get_thread_local_lfs()?;
    Ok(SExpr::Atom(lfs.total_log_blocks().to_string()))
}

fn builtin_lfs_usage_percent(_vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
    if !args.is_empty() {
        return Err(SError::new("lfs-usage-percent")
            .with_code("wrong-argument-count")
            .with_message("lfs-usage-percent takes no arguments"));
    }
    let lfs = get_thread_local_lfs()?;
    Ok(SExpr::Atom(lfs.usage_percent().to_string()))
}

fn builtin_lfs_clean(_vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
    if !args.is_empty() {
        return Err(SError::new("lfs-clean")
            .with_code("wrong-argument-count")
            .with_message("lfs-clean takes no arguments"));
    }
    let lfs = get_thread_local_lfs()?;
    let reclaimed = lfs.clean()?;
    Ok(SExpr::Atom(reclaimed.to_string()))
}

// ============================================================================
// Filesystem trait implementation for LfsLisp
// ============================================================================

/// A thread-safe wrapper around Lfs for implementing the Filesystem trait.
///
/// This type uses `Arc<Mutex<Lfs>>` to allow the Lfs to be shared across
/// threads as required by the `Filesystem` trait's `Send + Sync` bounds.
pub struct LfsSyncFilesystem<D: BlockDevice, T: Fn() -> i64> {
    lfs: Arc<Mutex<Lfs<D, T>>>,
}

impl<D: BlockDevice, T: Fn() -> i64> LfsSyncFilesystem<D, T> {
    /// Creates a new thread-safe filesystem wrapper.
    pub fn new(lfs: Lfs<D, T>) -> Self {
        Self {
            lfs: Arc::new(Mutex::new(lfs)),
        }
    }

    /// Returns a clone of the inner Arc for sharing.
    pub fn share(&self) -> Arc<Mutex<Lfs<D, T>>> {
        Arc::clone(&self.lfs)
    }
}

impl<D: BlockDevice + Send, T: Fn() -> i64 + Send> Filesystem for LfsSyncFilesystem<D, T> {
    fn root(&self) -> &Path {
        Path::new("/")
    }

    fn list_markdown_files(&self) -> SResult<Vec<String>> {
        let lfs = self.lfs.lock().unwrap();

        // Recursively find all .md files
        fn find_md_files<D: BlockDevice, T: Fn() -> i64>(
            lfs: &Lfs<D, T>,
            path: &str,
            results: &mut Vec<String>,
        ) -> SResult<()> {
            let entries = lfs.read_dir(path).map_err(lfs_error_to_serror)?;
            for (name, info) in entries {
                if name == "." || name == ".." {
                    continue;
                }
                let full_path = if path == "/" {
                    format!("/{}", name)
                } else {
                    format!("{}/{}", path, name)
                };
                match info.file_type {
                    FileType::Directory => {
                        find_md_files(lfs, &full_path, results)?;
                    }
                    FileType::RegularFile => {
                        if name.ends_with(".md") || name.ends_with(".MD") {
                            // Return path without leading slash for compatibility
                            results.push(full_path.trim_start_matches('/').to_string());
                        }
                    }
                    _ => {}
                }
            }
            Ok(())
        }

        let mut results = Vec::new();
        find_md_files(&lfs, "/", &mut results)?;
        results.sort();
        Ok(results)
    }

    fn read(&self, path: &str) -> SResult<String> {
        let full_path = if path.starts_with('/') {
            path.to_string()
        } else {
            format!("/{}", path)
        };
        let mut lfs = self.lfs.lock().unwrap();
        let bytes = lfs.read_file(&full_path).map_err(lfs_error_to_serror)?;
        String::from_utf8(bytes).map_err(|e| {
            SError::new("lfs")
                .with_code("invalid-utf8")
                .with_message("File contains invalid UTF-8")
                .with_string_field("error", &e.to_string())
        })
    }

    fn write(&self, path: &str, content: &str) -> SResult<()> {
        let full_path = if path.starts_with('/') {
            path.to_string()
        } else {
            format!("/{}", path)
        };

        let mut lfs = self.lfs.lock().unwrap();

        // Ensure parent directory exists
        if let Some(parent) = Path::new(&full_path).parent() {
            let parent_str = parent.to_string_lossy();
            if parent_str != "/" && !parent_str.is_empty() {
                // Ignore error if directory already exists
                let _ = lfs.mkdir_all(&parent_str);
            }
        }

        lfs.write_file(&full_path, content.as_bytes())
            .map_err(lfs_error_to_serror)
    }

    fn exists(&self, path: &str) -> bool {
        let full_path = if path.starts_with('/') {
            path.to_string()
        } else {
            format!("/{}", path)
        };
        let lfs = self.lfs.lock().unwrap();
        lfs.exists(&full_path)
    }
}

impl<T: Fn() -> i64> LfsSyncFilesystem<MemoryBlockDevice, T> {
    /// Creates a new thread-safe filesystem with an in-memory backend.
    pub fn from_vec(data: Vec<u8>, dev: DeviceId, time_source: T) -> crate::Result<Self> {
        let lfs = Lfs::from_vec(data, dev, time_source)?;
        Ok(Self::new(lfs))
    }
}

// ============================================================================
// REPL Support
// ============================================================================

use std::borrow::Cow;
use std::cell::RefCell;
use std::rc::Rc;

use rustyline::completion::{Completer, Pair};
use rustyline::config::Configurer;
use rustyline::error::ReadlineError;
use rustyline::highlight::{CmdKind, Highlighter};
use rustyline::hint::Hinter;
use rustyline::validate::{ValidationContext, ValidationResult, Validator};
use rustyline::{Context, EditMode, Editor, Helper};

/// Parses a size string like "1M", "64K", "1G" into bytes.
pub fn parse_size(s: &str) -> Result<usize, String> {
    let s = s.trim();
    if s.is_empty() {
        return Err("Empty size string".to_string());
    }

    let (num_str, suffix) = if s.ends_with(|c: char| c.is_ascii_alphabetic()) {
        let idx = s.len() - 1;
        (&s[..idx], &s[idx..])
    } else {
        (s, "")
    };

    let num: usize = num_str
        .parse()
        .map_err(|_| format!("Invalid number: {}", num_str))?;

    let multiplier = match suffix.to_uppercase().as_str() {
        "" | "B" => 1,
        "K" | "KB" => 1024,
        "M" | "MB" => 1024 * 1024,
        "G" | "GB" => 1024 * 1024 * 1024,
        _ => return Err(format!("Unknown size suffix: {}", suffix)),
    };

    Ok(num * multiplier)
}

/// Checks if parentheses are balanced in the input string.
pub fn is_balanced(input: &str) -> bool {
    let mut depth = 0i32;
    let mut in_string = false;
    let mut escape_next = false;

    for c in input.chars() {
        if escape_next {
            escape_next = false;
            continue;
        }

        match c {
            '\\' if in_string => {
                escape_next = true;
            }
            '"' => {
                in_string = !in_string;
            }
            '(' if !in_string => {
                depth += 1;
            }
            ')' if !in_string => {
                depth -= 1;
            }
            _ => {}
        }
    }

    depth <= 0 && !in_string
}

/// Extracts file type from a stat S-expression.
pub fn extract_type_from_stat(stat: &SExpr) -> &str {
    if let SExpr::List(items) = stat {
        for item in items {
            if let SExpr::List(pair) = item
                && pair.len() == 2
                && let SExpr::Atom(key) = &pair[0]
                && key == "type"
                && let SExpr::Atom(val) = &pair[1]
            {
                return match val.as_str() {
                    "dir" => "d",
                    "file" => "-",
                    "symlink" => "l",
                    _ => "?",
                };
            }
        }
    }
    "?"
}

/// Formats a stat S-expression for display.
pub fn format_stat(stat: &SExpr) -> String {
    if let SExpr::List(items) = stat {
        let mut output = String::new();
        for item in items {
            if let SExpr::List(pair) = item
                && pair.len() == 2
                && let (SExpr::Atom(key), SExpr::Atom(val)) = (&pair[0], &pair[1])
            {
                output.push_str(&format!("{}: {}\n", key, val));
            }
        }
        if output.is_empty() {
            stat.to_string()
        } else {
            output.trim_end().to_string()
        }
    } else {
        stat.to_string()
    }
}

/// Helper struct for rustyline that provides multi-line input and autocomplete.
#[derive(Default)]
pub struct LfsReplHelper {
    function_names: Vec<String>,
}

impl LfsReplHelper {
    /// Creates a new helper with the standard LFS function names.
    pub fn new() -> Self {
        let names = vec![
            // LFS builtins
            "lfs-stat",
            "lfs-lstat",
            "lfs-read",
            "lfs-write",
            "lfs-mkdir",
            "lfs-mkdir-all",
            "lfs-remove",
            "lfs-rmdir",
            "lfs-readdir",
            "lfs-exists?",
            "lfs-is-dir?",
            "lfs-symlink",
            "lfs-readlink",
            "lfs-link",
            "lfs-rename",
            "lfs-append",
            "lfs-tree",
            "lfs-free-blocks",
            "lfs-total-blocks",
            "lfs-usage-percent",
            "lfs-clean",
            // Core builtins
            "null?",
            "list?",
            "atom?",
            "empty?",
            "eq?",
            "first",
            "rest",
            "cons",
            "append",
            "length",
            "nth",
            "list",
            "help",
            "quote",
            "if",
            "let",
            "begin",
            "->",
            "->>",
            "map",
            "filter",
            "reduce",
            // JSON builtins
            "obj",
            "arr",
            "get",
            "keys",
            "values",
            "assoc",
            "dissoc",
            "merge",
        ];
        LfsReplHelper {
            function_names: names.iter().map(|s| s.to_string()).collect(),
        }
    }
}

impl Validator for LfsReplHelper {
    fn validate(&self, ctx: &mut ValidationContext) -> Result<ValidationResult, ReadlineError> {
        let input = ctx.input();
        if is_balanced(input) {
            Ok(ValidationResult::Valid(None))
        } else {
            Ok(ValidationResult::Incomplete)
        }
    }
}

impl Completer for LfsReplHelper {
    type Candidate = Pair;

    fn complete(
        &self,
        line: &str,
        pos: usize,
        _ctx: &Context<'_>,
    ) -> Result<(usize, Vec<Pair>), ReadlineError> {
        let start = line[..pos]
            .rfind(|c: char| c.is_whitespace() || c == '(' || c == ')')
            .map(|i| i + 1)
            .unwrap_or(0);
        let prefix = &line[start..pos];

        if prefix.is_empty() {
            return Ok((pos, Vec::new()));
        }

        let matches: Vec<Pair> = self
            .function_names
            .iter()
            .filter(|name| name.starts_with(prefix))
            .map(|name| Pair {
                display: name.clone(),
                replacement: name.clone(),
            })
            .collect();

        Ok((start, matches))
    }
}

impl Hinter for LfsReplHelper {
    type Hint = String;

    fn hint(&self, _line: &str, _pos: usize, _ctx: &Context<'_>) -> Option<Self::Hint> {
        None
    }
}

impl Highlighter for LfsReplHelper {
    fn highlight<'l>(&self, line: &'l str, _pos: usize) -> Cow<'l, str> {
        Cow::Borrowed(line)
    }

    fn highlight_char(&self, _line: &str, _pos: usize, _kind: CmdKind) -> bool {
        false
    }
}

impl Helper for LfsReplHelper {}

/// Trait object for type-erased LfsLisp debug operations.
pub trait LfsDebugOps {
    /// Gets the filesystem tree as an S-expression string.
    fn tree(&self) -> String;
    /// Gets filesystem usage statistics as a formatted string.
    fn usage(&self) -> String;
    /// Lists directory contents in a human-readable format.
    fn ls(&self, path: &str) -> String;
    /// Gets file contents as a string.
    fn cat(&self, path: &str) -> String;
    /// Gets file stat information as a formatted string.
    fn stat(&self, path: &str) -> String;
}

impl<D: BlockDevice, T: Fn() -> i64> LfsDebugOps for LfsLisp<D, T> {
    fn tree(&self) -> String {
        match LfsLisp::tree(self) {
            Ok(expr) => expr.to_string(),
            Err(e) => format!("Error: {}", e),
        }
    }

    fn usage(&self) -> String {
        let free = self.free_blocks();
        let total = self.total_log_blocks();
        let percent = self.usage_percent();
        format!(
            "Free blocks: {}\nTotal blocks: {}\nUsage: {}%",
            free, total, percent
        )
    }

    fn ls(&self, path: &str) -> String {
        match LfsLisp::readdir(self, path) {
            Ok(SExpr::List(entries)) => {
                let mut output = String::new();
                for entry in entries {
                    if let SExpr::List(items) = entry
                        && items.len() >= 2
                    {
                        // Extract name and type from stat
                        let name = &items[0];
                        let stat = &items[1];
                        let file_type = extract_type_from_stat(stat);
                        output.push_str(&format!("{} {}\n", file_type, name));
                    }
                }
                if output.is_empty() {
                    "(empty directory)".to_string()
                } else {
                    output.trim_end().to_string()
                }
            }
            Ok(other) => other.to_string(),
            Err(e) => format!("Error: {}", e),
        }
    }

    fn cat(&self, path: &str) -> String {
        match LfsLisp::read(self, path) {
            Ok(content) => content,
            Err(e) => format!("Error: {}", e),
        }
    }

    fn stat(&self, path: &str) -> String {
        match LfsLisp::stat(self, path) {
            Ok(expr) => format_stat(&expr),
            Err(e) => format!("Error: {}", e),
        }
    }
}

/// Trait for type-erased LfsLisp operations used by the REPL builtins.
trait LfsDebugOpsInternal: LfsDebugOps {
    fn stat_internal(&self, path: &str) -> SResult<SExpr>;
    fn lstat_internal(&self, path: &str) -> SResult<SExpr>;
    fn read_internal(&self, path: &str) -> SResult<String>;
    fn write_internal(&self, path: &str, content: &str) -> SResult<()>;
    fn mkdir_internal(&self, path: &str) -> SResult<()>;
    fn mkdir_all_internal(&self, path: &str) -> SResult<()>;
    fn remove_internal(&self, path: &str) -> SResult<()>;
    fn rmdir_internal(&self, path: &str) -> SResult<()>;
    fn readdir_internal(&self, path: &str) -> SResult<SExpr>;
    fn exists_internal(&self, path: &str) -> bool;
    fn is_dir_internal(&self, path: &str) -> bool;
    fn symlink_internal(&self, target: &str, linkpath: &str) -> SResult<()>;
    fn readlink_internal(&self, path: &str) -> SResult<String>;
    fn link_internal(&self, src: &str, dst: &str) -> SResult<()>;
    fn rename_internal(&self, src: &str, dst: &str) -> SResult<()>;
    fn append_internal(&self, path: &str, content: &str) -> SResult<()>;
    fn tree_internal(&self) -> SResult<SExpr>;
    fn free_blocks_internal(&self) -> u64;
    fn total_log_blocks_internal(&self) -> u64;
    fn usage_percent_internal(&self) -> u64;
    fn clean_internal(&self) -> SResult<usize>;
}

impl<D: BlockDevice, T: Fn() -> i64> LfsDebugOpsInternal for LfsLisp<D, T> {
    fn stat_internal(&self, path: &str) -> SResult<SExpr> {
        LfsLisp::stat(self, path)
    }
    fn lstat_internal(&self, path: &str) -> SResult<SExpr> {
        LfsLisp::lstat(self, path)
    }
    fn read_internal(&self, path: &str) -> SResult<String> {
        LfsLisp::read(self, path)
    }
    fn write_internal(&self, path: &str, content: &str) -> SResult<()> {
        LfsLisp::write(self, path, content)
    }
    fn mkdir_internal(&self, path: &str) -> SResult<()> {
        LfsLisp::mkdir(self, path)
    }
    fn mkdir_all_internal(&self, path: &str) -> SResult<()> {
        LfsLisp::mkdir_all(self, path)
    }
    fn remove_internal(&self, path: &str) -> SResult<()> {
        LfsLisp::remove(self, path)
    }
    fn rmdir_internal(&self, path: &str) -> SResult<()> {
        LfsLisp::rmdir(self, path)
    }
    fn readdir_internal(&self, path: &str) -> SResult<SExpr> {
        LfsLisp::readdir(self, path)
    }
    fn exists_internal(&self, path: &str) -> bool {
        LfsLisp::exists(self, path)
    }
    fn is_dir_internal(&self, path: &str) -> bool {
        LfsLisp::is_dir(self, path)
    }
    fn symlink_internal(&self, target: &str, linkpath: &str) -> SResult<()> {
        LfsLisp::symlink(self, target, linkpath)
    }
    fn readlink_internal(&self, path: &str) -> SResult<String> {
        LfsLisp::readlink(self, path)
    }
    fn link_internal(&self, src: &str, dst: &str) -> SResult<()> {
        LfsLisp::link(self, src, dst)
    }
    fn rename_internal(&self, src: &str, dst: &str) -> SResult<()> {
        LfsLisp::rename(self, src, dst)
    }
    fn append_internal(&self, path: &str, content: &str) -> SResult<()> {
        LfsLisp::append(self, path, content)
    }
    fn tree_internal(&self) -> SResult<SExpr> {
        LfsLisp::tree(self)
    }
    fn free_blocks_internal(&self) -> u64 {
        LfsLisp::free_blocks(self)
    }
    fn total_log_blocks_internal(&self) -> u64 {
        LfsLisp::total_log_blocks(self)
    }
    fn usage_percent_internal(&self) -> u64 {
        LfsLisp::usage_percent(self)
    }
    fn clean_internal(&self) -> SResult<usize> {
        LfsLisp::clean(self)
    }
}

thread_local! {
    static THREAD_LFS_REPL_WRAPPER: RefCell<Option<Rc<dyn LfsDebugOpsInternal>>> = const { RefCell::new(None) };
}

fn set_thread_local_lfs_repl_wrapper<D: BlockDevice + 'static, T: Fn() -> i64 + 'static>(
    lfs: Rc<LfsLisp<D, T>>,
) {
    THREAD_LFS_REPL_WRAPPER.with(|cell| {
        *cell.borrow_mut() = Some(lfs as Rc<dyn LfsDebugOpsInternal>);
    });
}

fn get_thread_local_lfs_repl_wrapper() -> SResult<Rc<dyn LfsDebugOpsInternal>> {
    THREAD_LFS_REPL_WRAPPER.with(|cell| {
        cell.borrow().clone().ok_or_else(|| {
            SError::new("lfs")
                .with_code("no-filesystem")
                .with_message("No LFS filesystem is registered")
        })
    })
}

/// Wrapper to register builtins with an existing Rc<LfsLisp>.
struct LfsLispReplWrapper<D: BlockDevice, T: Fn() -> i64> {
    lfs: Rc<LfsLisp<D, T>>,
}

impl<D: BlockDevice + 'static, T: Fn() -> i64 + 'static> LfsLispReplWrapper<D, T> {
    fn register_builtins(&self, vm: &mut Vm) {
        set_thread_local_lfs_repl_wrapper(Rc::clone(&self.lfs));

        vm.def_fn("lfs-stat", repl_builtin_lfs_stat);
        vm.def_fn("lfs-lstat", repl_builtin_lfs_lstat);
        vm.def_fn("lfs-read", repl_builtin_lfs_read);
        vm.def_fn("lfs-write", repl_builtin_lfs_write);
        vm.def_fn("lfs-mkdir", repl_builtin_lfs_mkdir);
        vm.def_fn("lfs-mkdir-all", repl_builtin_lfs_mkdir_all);
        vm.def_fn("lfs-remove", repl_builtin_lfs_remove);
        vm.def_fn("lfs-rmdir", repl_builtin_lfs_rmdir);
        vm.def_fn("lfs-readdir", repl_builtin_lfs_readdir);
        vm.def_fn("lfs-exists?", repl_builtin_lfs_exists);
        vm.def_fn("lfs-is-dir?", repl_builtin_lfs_is_dir);
        vm.def_fn("lfs-symlink", repl_builtin_lfs_symlink);
        vm.def_fn("lfs-readlink", repl_builtin_lfs_readlink);
        vm.def_fn("lfs-link", repl_builtin_lfs_link);
        vm.def_fn("lfs-rename", repl_builtin_lfs_rename);
        vm.def_fn("lfs-append", repl_builtin_lfs_append);
        vm.def_fn("lfs-tree", repl_builtin_lfs_tree);
        vm.def_fn("lfs-free-blocks", repl_builtin_lfs_free_blocks);
        vm.def_fn("lfs-total-blocks", repl_builtin_lfs_total_blocks);
        vm.def_fn("lfs-usage-percent", repl_builtin_lfs_usage_percent);
        vm.def_fn("lfs-clean", repl_builtin_lfs_clean);
    }
}

// REPL builtin functions that use the thread-local wrapper

fn repl_builtin_lfs_stat(_vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 1 {
        return Err(SError::new("lfs-stat")
            .with_code("wrong-argument-count")
            .with_message("lfs-stat requires exactly one argument: path"));
    }
    let path = extract_string(&args[0]);
    let lfs = get_thread_local_lfs_repl_wrapper()?;
    lfs.stat_internal(&path)
}

fn repl_builtin_lfs_lstat(_vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 1 {
        return Err(SError::new("lfs-lstat")
            .with_code("wrong-argument-count")
            .with_message("lfs-lstat requires exactly one argument: path"));
    }
    let path = extract_string(&args[0]);
    let lfs = get_thread_local_lfs_repl_wrapper()?;
    lfs.lstat_internal(&path)
}

fn repl_builtin_lfs_read(_vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 1 {
        return Err(SError::new("lfs-read")
            .with_code("wrong-argument-count")
            .with_message("lfs-read requires exactly one argument: path"));
    }
    let path = extract_string(&args[0]);
    let lfs = get_thread_local_lfs_repl_wrapper()?;
    let content = lfs.read_internal(&path)?;
    Ok(string_atom(&content))
}

fn repl_builtin_lfs_write(_vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 2 {
        return Err(SError::new("lfs-write")
            .with_code("wrong-argument-count")
            .with_message("lfs-write requires exactly two arguments: path and content"));
    }
    let path = extract_string(&args[0]);
    let content = extract_string(&args[1]);
    let lfs = get_thread_local_lfs_repl_wrapper()?;
    lfs.write_internal(&path, &content)?;
    Ok(string_atom(&path))
}

fn repl_builtin_lfs_mkdir(_vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 1 {
        return Err(SError::new("lfs-mkdir")
            .with_code("wrong-argument-count")
            .with_message("lfs-mkdir requires exactly one argument: path"));
    }
    let path = extract_string(&args[0]);
    let lfs = get_thread_local_lfs_repl_wrapper()?;
    lfs.mkdir_internal(&path)?;
    Ok(string_atom(&path))
}

fn repl_builtin_lfs_mkdir_all(_vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 1 {
        return Err(SError::new("lfs-mkdir-all")
            .with_code("wrong-argument-count")
            .with_message("lfs-mkdir-all requires exactly one argument: path"));
    }
    let path = extract_string(&args[0]);
    let lfs = get_thread_local_lfs_repl_wrapper()?;
    lfs.mkdir_all_internal(&path)?;
    Ok(string_atom(&path))
}

fn repl_builtin_lfs_remove(_vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 1 {
        return Err(SError::new("lfs-remove")
            .with_code("wrong-argument-count")
            .with_message("lfs-remove requires exactly one argument: path"));
    }
    let path = extract_string(&args[0]);
    let lfs = get_thread_local_lfs_repl_wrapper()?;
    lfs.remove_internal(&path)?;
    Ok(SExpr::Atom("#t".to_string()))
}

fn repl_builtin_lfs_rmdir(_vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 1 {
        return Err(SError::new("lfs-rmdir")
            .with_code("wrong-argument-count")
            .with_message("lfs-rmdir requires exactly one argument: path"));
    }
    let path = extract_string(&args[0]);
    let lfs = get_thread_local_lfs_repl_wrapper()?;
    lfs.rmdir_internal(&path)?;
    Ok(SExpr::Atom("#t".to_string()))
}

fn repl_builtin_lfs_readdir(_vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 1 {
        return Err(SError::new("lfs-readdir")
            .with_code("wrong-argument-count")
            .with_message("lfs-readdir requires exactly one argument: path"));
    }
    let path = extract_string(&args[0]);
    let lfs = get_thread_local_lfs_repl_wrapper()?;
    lfs.readdir_internal(&path)
}

fn repl_builtin_lfs_exists(_vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 1 {
        return Err(SError::new("lfs-exists?")
            .with_code("wrong-argument-count")
            .with_message("lfs-exists? requires exactly one argument: path"));
    }
    let path = extract_string(&args[0]);
    let lfs = get_thread_local_lfs_repl_wrapper()?;
    let exists = lfs.exists_internal(&path);
    Ok(SExpr::Atom(if exists { "#t" } else { "#f" }.to_string()))
}

fn repl_builtin_lfs_is_dir(_vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 1 {
        return Err(SError::new("lfs-is-dir?")
            .with_code("wrong-argument-count")
            .with_message("lfs-is-dir? requires exactly one argument: path"));
    }
    let path = extract_string(&args[0]);
    let lfs = get_thread_local_lfs_repl_wrapper()?;
    let is_dir = lfs.is_dir_internal(&path);
    Ok(SExpr::Atom(if is_dir { "#t" } else { "#f" }.to_string()))
}

fn repl_builtin_lfs_symlink(_vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 2 {
        return Err(SError::new("lfs-symlink")
            .with_code("wrong-argument-count")
            .with_message("lfs-symlink requires exactly two arguments: target and linkpath"));
    }
    let target = extract_string(&args[0]);
    let linkpath = extract_string(&args[1]);
    let lfs = get_thread_local_lfs_repl_wrapper()?;
    lfs.symlink_internal(&target, &linkpath)?;
    Ok(string_atom(&linkpath))
}

fn repl_builtin_lfs_readlink(_vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 1 {
        return Err(SError::new("lfs-readlink")
            .with_code("wrong-argument-count")
            .with_message("lfs-readlink requires exactly one argument: path"));
    }
    let path = extract_string(&args[0]);
    let lfs = get_thread_local_lfs_repl_wrapper()?;
    let target = lfs.readlink_internal(&path)?;
    Ok(string_atom(&target))
}

fn repl_builtin_lfs_link(_vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 2 {
        return Err(SError::new("lfs-link")
            .with_code("wrong-argument-count")
            .with_message("lfs-link requires exactly two arguments: src and dst"));
    }
    let src = extract_string(&args[0]);
    let dst = extract_string(&args[1]);
    let lfs = get_thread_local_lfs_repl_wrapper()?;
    lfs.link_internal(&src, &dst)?;
    Ok(string_atom(&dst))
}

fn repl_builtin_lfs_rename(_vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 2 {
        return Err(SError::new("lfs-rename")
            .with_code("wrong-argument-count")
            .with_message("lfs-rename requires exactly two arguments: src and dst"));
    }
    let src = extract_string(&args[0]);
    let dst = extract_string(&args[1]);
    let lfs = get_thread_local_lfs_repl_wrapper()?;
    lfs.rename_internal(&src, &dst)?;
    Ok(string_atom(&dst))
}

fn repl_builtin_lfs_append(_vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 2 {
        return Err(SError::new("lfs-append")
            .with_code("wrong-argument-count")
            .with_message("lfs-append requires exactly two arguments: path and content"));
    }
    let path = extract_string(&args[0]);
    let content = extract_string(&args[1]);
    let lfs = get_thread_local_lfs_repl_wrapper()?;
    lfs.append_internal(&path, &content)?;
    Ok(string_atom(&path))
}

fn repl_builtin_lfs_tree(_vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
    if !args.is_empty() {
        return Err(SError::new("lfs-tree")
            .with_code("wrong-argument-count")
            .with_message("lfs-tree takes no arguments"));
    }
    let lfs = get_thread_local_lfs_repl_wrapper()?;
    lfs.tree_internal()
}

fn repl_builtin_lfs_free_blocks(_vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
    if !args.is_empty() {
        return Err(SError::new("lfs-free-blocks")
            .with_code("wrong-argument-count")
            .with_message("lfs-free-blocks takes no arguments"));
    }
    let lfs = get_thread_local_lfs_repl_wrapper()?;
    Ok(SExpr::Atom(lfs.free_blocks_internal().to_string()))
}

fn repl_builtin_lfs_total_blocks(_vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
    if !args.is_empty() {
        return Err(SError::new("lfs-total-blocks")
            .with_code("wrong-argument-count")
            .with_message("lfs-total-blocks takes no arguments"));
    }
    let lfs = get_thread_local_lfs_repl_wrapper()?;
    Ok(SExpr::Atom(lfs.total_log_blocks_internal().to_string()))
}

fn repl_builtin_lfs_usage_percent(_vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
    if !args.is_empty() {
        return Err(SError::new("lfs-usage-percent")
            .with_code("wrong-argument-count")
            .with_message("lfs-usage-percent takes no arguments"));
    }
    let lfs = get_thread_local_lfs_repl_wrapper()?;
    Ok(SExpr::Atom(lfs.usage_percent_internal().to_string()))
}

fn repl_builtin_lfs_clean(_vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
    if !args.is_empty() {
        return Err(SError::new("lfs-clean")
            .with_code("wrong-argument-count")
            .with_message("lfs-clean takes no arguments"));
    }
    let lfs = get_thread_local_lfs_repl_wrapper()?;
    let reclaimed = lfs.clean_internal()?;
    Ok(SExpr::Atom(reclaimed.to_string()))
}

/// Configuration for the debug REPL.
#[derive(Default)]
pub struct DebugImgReplConfig {
    /// Whether to operate in read-only mode.
    pub read_only: bool,
    /// An optional function to call when the user requests a sync.
    pub sync_fn: Option<Box<dyn Fn()>>,
    /// An optional banner to display at startup.
    pub banner: Option<String>,
}

/// The debug image REPL for interacting with LFS filesystem images.
///
/// This struct encapsulates all the REPL functionality for exploring and
/// manipulating log-structured filesystem images through a Lisp interface.
pub struct DebugImgRepl<D: BlockDevice + 'static, T: Fn() -> i64 + 'static> {
    lfs: Rc<LfsLisp<D, T>>,
    lfs_ops: Rc<dyn LfsDebugOps>,
    vm: Vm,
    config: DebugImgReplConfig,
}

impl<D: BlockDevice + 'static, T: Fn() -> i64 + 'static> DebugImgRepl<D, T> {
    /// Creates a new debug REPL with the given LfsLisp instance.
    pub fn new(lfs: LfsLisp<D, T>, config: DebugImgReplConfig) -> Self {
        let lfs_rc = Rc::new(lfs);
        let lfs_ops: Rc<dyn LfsDebugOps> = Rc::clone(&lfs_rc) as Rc<dyn LfsDebugOps>;

        let state = LfsLispReplWrapper {
            lfs: Rc::clone(&lfs_rc),
        };

        let mut vm = Vm::new();
        vm.register_builtins();
        vm.register_json_builtins();
        state.register_builtins(&mut vm);

        Self {
            lfs: lfs_rc,
            lfs_ops,
            vm,
            config,
        }
    }

    /// Prints the help message for REPL commands.
    pub fn print_help() {
        println!(
            r#"Commands:
  :help, :h, :?      Show this help
  :quit, :q, :exit   Exit the REPL
  :tree              Show filesystem tree
  :ls [path]         List directory (default: /)
  :cat path          Show file contents
  :stat path         Show file metadata
  :usage             Show filesystem usage
  :sync              Sync changes to disk
  :fns               List available Lisp functions

Lisp evaluation:
  Type any S-expression to evaluate it.
  
Examples:
  (lfs-write "/hello.txt" "Hello, World!")
  (lfs-read "/hello.txt")
  (lfs-mkdir "/mydir")
  (lfs-tree)
  (lfs-exists? "/hello.txt")
  (-> "/hello.txt" lfs-stat)
"#
        );
    }

    /// Prints available functions.
    pub fn print_functions() {
        println!(
            r#"LFS Functions:
  (lfs-stat path)           Get file metadata
  (lfs-lstat path)          Get metadata (no symlink follow)
  (lfs-read path)           Read file contents
  (lfs-write path content)  Write to file
  (lfs-append path content) Append to file
  (lfs-mkdir path)          Create directory
  (lfs-mkdir-all path)      Create directory with parents
  (lfs-remove path)         Remove file
  (lfs-rmdir path)          Remove empty directory
  (lfs-readdir path)        List directory
  (lfs-exists? path)        Check if path exists
  (lfs-is-dir? path)        Check if path is directory
  (lfs-symlink target link) Create symlink
  (lfs-readlink path)       Read symlink target
  (lfs-link src dst)        Create hard link
  (lfs-rename src dst)      Rename file/directory
  (lfs-tree)                Get filesystem tree
  (lfs-free-blocks)         Get free block count
  (lfs-total-blocks)        Get total block count
  (lfs-usage-percent)       Get usage percentage
  (lfs-clean)               Run garbage collection

Core Functions:
  first, rest, cons, append, length, nth, list
  null?, list?, atom?, empty?, eq?
  map, filter, reduce
  quote, if, let, begin, ->, ->>
"#
        );
    }

    /// Handles a single REPL command.
    ///
    /// Returns `Some(true)` to continue, `Some(false)` to quit, or `None` for empty input.
    pub fn handle_command(&mut self, input: &str) -> Option<bool> {
        let input = input.trim();
        if input.is_empty() {
            return None;
        }

        // Handle REPL commands
        if input.starts_with(':') {
            let parts: Vec<&str> = input.split_whitespace().collect();
            match parts[0] {
                ":quit" | ":q" | ":exit" => return Some(false),
                ":help" | ":h" | ":?" => Self::print_help(),
                ":fns" | ":functions" => Self::print_functions(),
                ":tree" => println!("{}", self.lfs_ops.tree()),
                ":usage" => println!("{}", self.lfs_ops.usage()),
                ":ls" => {
                    let path = if parts.len() > 1 { parts[1] } else { "/" };
                    println!("{}", self.lfs_ops.ls(path));
                }
                ":cat" => {
                    if parts.len() < 2 {
                        println!("Usage: :cat <path>");
                    } else {
                        println!("{}", self.lfs_ops.cat(parts[1]));
                    }
                }
                ":stat" => {
                    if parts.len() < 2 {
                        println!("Usage: :stat <path>");
                    } else {
                        println!("{}", self.lfs_ops.stat(parts[1]));
                    }
                }
                ":sync" => {
                    if self.config.read_only {
                        println!("Read-only mode - sync disabled");
                    } else if let Some(ref sync) = self.config.sync_fn {
                        sync();
                        println!("Synced to disk");
                    } else {
                        println!("No sync function available (in-memory filesystem)");
                    }
                }
                _ => println!("Unknown command: {}. Type :help for commands.", parts[0]),
            }
            return Some(true);
        }

        // Evaluate S-expression
        let mut parser = lispdown::Parser::new(input);
        match parser.parse() {
            Ok(expr) => match self.vm.eval(&expr) {
                Ok(result) => println!("{}", result),
                Err(e) => println!("Error: {}", e),
            },
            Err(e) => println!("Parse error: {}", e),
        }

        Some(true)
    }

    /// Runs the REPL interactively.
    ///
    /// This method blocks until the user quits the REPL.
    pub fn run(mut self) {
        let helper = LfsReplHelper::new();
        let mut rl: Editor<LfsReplHelper, rustyline::history::DefaultHistory> =
            Editor::new().expect("Failed to create readline editor");
        rl.set_helper(Some(helper));
        rl.set_edit_mode(EditMode::Vi);

        if let Some(ref banner) = self.config.banner {
            println!("{}", banner);
        } else {
            println!("eudaemon-debug-img - LFS filesystem debugger");
        }
        if self.config.read_only {
            println!("Mode: read-only");
        }
        println!("Type :help for commands, :quit to exit\n");

        loop {
            match rl.readline("lfs> ") {
                Ok(line) => {
                    let _ = rl.add_history_entry(&line);
                    match self.handle_command(&line) {
                        Some(true) => continue,
                        Some(false) => break,
                        None => continue,
                    }
                }
                Err(ReadlineError::Interrupted) => {
                    println!("^C");
                    continue;
                }
                Err(ReadlineError::Eof) => {
                    println!("^D");
                    break;
                }
                Err(err) => {
                    println!("Error: {:?}", err);
                    break;
                }
            }
        }

        // Sync on exit if not read-only
        if !self.config.read_only
            && let Some(sync) = self.config.sync_fn.take()
        {
            sync();
            println!("Changes synced to disk.");
        }
    }

    /// Returns a reference to the underlying LfsLisp.
    pub fn lfs(&self) -> &Rc<LfsLisp<D, T>> {
        &self.lfs
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BLOCK_SIZE: usize = 4096;

    fn zero_time() -> i64 {
        0
    }

    fn create_test_lfs() -> LfsLisp<MemoryBlockDevice, fn() -> i64> {
        let data = vec![0u8; 64 * BLOCK_SIZE];
        LfsLisp::from_vec(data, DeviceId::new(1), zero_time as fn() -> i64).unwrap()
    }

    #[test]
    fn write_and_read_file() {
        let lfs = create_test_lfs();
        lfs.write("/hello.txt", "Hello, World!").unwrap();
        let content = lfs.read("/hello.txt").unwrap();
        assert_eq!(content, "Hello, World!");
        println!("DEBUG: write/read roundtrip successful");
    }

    #[test]
    fn stat_file() {
        let lfs = create_test_lfs();
        lfs.write("/test.txt", "test content").unwrap();
        let stat = lfs.stat("/test.txt").unwrap();
        assert!(stat.to_string().contains("file"));
        assert!(stat.to_string().contains("size"));
        println!("DEBUG: stat returns expected S-expression: {}", stat);
    }

    #[test]
    fn mkdir_and_readdir() {
        let lfs = create_test_lfs();
        lfs.mkdir("/mydir").unwrap();
        lfs.write("/mydir/file.txt", "content").unwrap();

        let entries = lfs.readdir("/mydir").unwrap();
        let entries_str = entries.to_string();
        assert!(entries_str.contains("file.txt"));
        println!("DEBUG: readdir returns: {}", entries);
    }

    #[test]
    fn exists_check() {
        let lfs = create_test_lfs();
        assert!(!lfs.exists("/nonexistent.txt"));
        lfs.write("/exists.txt", "").unwrap();
        assert!(lfs.exists("/exists.txt"));
        println!("DEBUG: exists check works correctly");
    }

    #[test]
    fn tree_representation() {
        let lfs = create_test_lfs();
        lfs.mkdir("/docs").unwrap();
        lfs.write("/docs/readme.md", "# README").unwrap();
        lfs.write("/hello.txt", "Hello").unwrap();

        let tree = lfs.tree().unwrap();
        let tree_str = tree.to_string();
        assert!(tree_str.contains("fs"));
        assert!(tree_str.contains("dir"));
        assert!(tree_str.contains("docs"));
        assert!(tree_str.contains("readme.md"));
        assert!(tree_str.contains("hello.txt"));
        println!("DEBUG: filesystem tree: {}", tree);
    }

    #[test]
    fn symlink_operations() {
        let lfs = create_test_lfs();
        lfs.write("/target.txt", "target content").unwrap();
        lfs.symlink("/target.txt", "/link").unwrap();

        let target = lfs.readlink("/link").unwrap();
        assert_eq!(target, "/target.txt");

        // Reading through the symlink should work
        let content = lfs.read("/link").unwrap();
        assert_eq!(content, "target content");
        println!("DEBUG: symlink operations work correctly");
    }

    #[test]
    fn filesystem_adapter_list_markdown() {
        let data = vec![0u8; 64 * BLOCK_SIZE];
        let sync_fs =
            LfsSyncFilesystem::from_vec(data, DeviceId::new(1), zero_time as fn() -> i64).unwrap();

        // Set up the filesystem
        {
            let arc = sync_fs.share();
            let mut lfs = arc.lock().unwrap();
            lfs.mkdir("/docs").unwrap();
            lfs.write_file("/readme.md", b"# Root README").unwrap();
            lfs.write_file("/docs/guide.md", b"# Guide").unwrap();
            lfs.write_file("/config.json", b"{}").unwrap();
        }

        let files = sync_fs.list_markdown_files().unwrap();

        assert!(files.contains(&"readme.md".to_string()));
        assert!(files.contains(&"docs/guide.md".to_string()));
        assert!(!files.contains(&"config.json".to_string()));
        println!("DEBUG: adapter lists markdown files: {:?}", files);
    }

    #[test]
    fn vm_integration() {
        let lfs = create_test_lfs();
        let state = LfsLispState::new(lfs);

        let mut vm = Vm::new();
        vm.register_builtins();
        state.register_builtins(&mut vm);

        // Test lfs-write and lfs-read
        let write_result = vm
            .eval(&SExpr::List(vec![
                SExpr::Atom("lfs-write".to_string()),
                SExpr::Atom("\"/test.txt\"".to_string()),
                SExpr::Atom("\"Hello from Lisp!\"".to_string()),
            ]))
            .unwrap();
        assert!(write_result.to_string().contains("test.txt"));

        let read_result = vm
            .eval(&SExpr::List(vec![
                SExpr::Atom("lfs-read".to_string()),
                SExpr::Atom("\"/test.txt\"".to_string()),
            ]))
            .unwrap();
        assert!(read_result.to_string().contains("Hello from Lisp!"));

        // Test lfs-exists?
        let exists_result = vm
            .eval(&SExpr::List(vec![
                SExpr::Atom("lfs-exists?".to_string()),
                SExpr::Atom("\"/test.txt\"".to_string()),
            ]))
            .unwrap();
        assert_eq!(exists_result.to_string(), "#t");

        let not_exists_result = vm
            .eval(&SExpr::List(vec![
                SExpr::Atom("lfs-exists?".to_string()),
                SExpr::Atom("\"/nonexistent.txt\"".to_string()),
            ]))
            .unwrap();
        assert_eq!(not_exists_result.to_string(), "#f");

        println!("DEBUG: VM integration tests passed");
    }

    #[test]
    fn free_blocks_and_usage() {
        let lfs = create_test_lfs();
        let initial_free = lfs.free_blocks();
        let usage = lfs.usage_percent();

        assert!(initial_free > 0);
        assert!(usage < 100);

        // Write some data
        lfs.write("/large.txt", &"x".repeat(10000)).unwrap();
        let after_write = lfs.free_blocks();

        assert!(after_write < initial_free);
        println!(
            "DEBUG: free blocks before={}, after={}, usage={}%",
            initial_free, after_write, usage
        );
    }

    #[test]
    fn rename_file() {
        let lfs = create_test_lfs();
        lfs.write("/old.txt", "content").unwrap();
        lfs.rename("/old.txt", "/new.txt").unwrap();

        assert!(!lfs.exists("/old.txt"));
        assert!(lfs.exists("/new.txt"));
        assert_eq!(lfs.read("/new.txt").unwrap(), "content");
        println!("DEBUG: rename works correctly");
    }

    #[test]
    fn append_file() {
        let lfs = create_test_lfs();
        lfs.write("/append.txt", "Hello").unwrap();
        lfs.append("/append.txt", ", World!").unwrap();

        let content = lfs.read("/append.txt").unwrap();
        assert_eq!(content, "Hello, World!");
        println!("DEBUG: append works correctly");
    }

    #[test]
    fn remove_file() {
        let lfs = create_test_lfs();
        lfs.write("/todelete.txt", "content").unwrap();
        assert!(lfs.exists("/todelete.txt"));

        lfs.remove("/todelete.txt").unwrap();
        assert!(!lfs.exists("/todelete.txt"));
        println!("DEBUG: remove works correctly");
    }

    #[test]
    fn mkdir_all_creates_parents() {
        let lfs = create_test_lfs();
        lfs.mkdir_all("/a/b/c").unwrap();

        assert!(lfs.is_dir("/a"));
        assert!(lfs.is_dir("/a/b"));
        assert!(lfs.is_dir("/a/b/c"));
        println!("DEBUG: mkdir_all creates parent directories");
    }
}

//! Lisp interface for the Log-structured File System.
//!
//! This module provides a filesystem implementation that can be used with the lispdown VM
//! for programmatic manipulation of filesystem state through Lisp programs.
//!
//! # Usage
//!
//! Create an `LfsSyncFilesystem` from an `Lfs` instance and set it on the VM:
//!
//! ```ignore
//! let lfs = Lfs::new(...)?;
//! let fs = LfsSyncFilesystem::new(lfs);
//! let mut vm = Vm::new();
//! vm.register_builtins();
//! vm.set_filesystem(Box::new(fs));
//! vm.register_filesystem_builtins();
//! ```
//!
//! Then use the standard `fs-*` builtins:
//! - `(fs-read path)` - Read file contents
//! - `(fs-write path content)` - Write to a file
//! - `(fs-mkdir path)` - Create a directory
//! - `(fs-unlink path)` - Remove a file
//! - `(fs-tree)` - Get filesystem tree
//! - `(fs-free-blocks)` - Get free block count
//! - `(fs-usage-percent)` - Get usage percentage

use std::sync::Arc;
use std::sync::Mutex;

use eudaemonty::Filesystem;
use lispdown::SExpr;

use crate::{BlockDevice, DeviceId, FileType, Lfs, MemoryBlockDevice};

// ============================================================================
// LfsSyncFilesystem - implements eudaemonty::Filesystem
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

/// Convert an LFS error to an eudaemonty Error.
fn lfs_error_to_eudaemonty_error(err: crate::Error) -> eudaemonty::Error {
    eudaemonty::Error::Io(std::io::Error::other(format!("{:?}", err)))
}

/// Convert LFS StatInfo to eudaemonty DirEntry.
fn stat_info_to_dir_entry(info: &crate::StatInfo) -> eudaemonty::DirEntry {
    eudaemonty::DirEntry {
        file_type: info.file_type,
        size: info.size,
        atime_ms: info.atime_ms,
        mtime_ms: info.mtime_ms,
        dev: info.dev,
        ino: info.ino,
    }
}

/// Normalizes a path to always have a leading slash.
fn normalize_path(path: &str) -> String {
    if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{}", path)
    }
}

/// The root path string for LfsSyncFilesystem.
static LFS_ROOT: &str = "/";

impl<D: BlockDevice + Send, T: Fn() -> i64 + Send> Filesystem for LfsSyncFilesystem<D, T> {
    fn root(&self) -> utf8path::Path<'_> {
        utf8path::Path::new(LFS_ROOT)
    }

    fn dup(&self) -> Self {
        Self {
            lfs: Arc::clone(&self.lfs),
        }
    }

    fn read_to_string(&self, path: &str) -> Result<String, eudaemonty::Error> {
        let full_path = normalize_path(path);
        let mut lfs = self.lfs.lock().unwrap();
        let bytes = lfs
            .read_file(&full_path)
            .map_err(lfs_error_to_eudaemonty_error)?;
        String::from_utf8(bytes).map_err(|e| {
            eudaemonty::Error::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("Invalid UTF-8: {}", e),
            ))
        })
    }

    fn exists(&self, path: &str) -> bool {
        let full_path = normalize_path(path);
        let lfs = self.lfs.lock().unwrap();
        lfs.exists(&full_path)
    }

    fn metadata(&self, path: &str) -> Result<eudaemonty::FileMetadata, eudaemonty::Error> {
        let full_path = normalize_path(path);
        let lfs = self.lfs.lock().unwrap();
        let info = lfs
            .stat(&full_path)
            .map_err(lfs_error_to_eudaemonty_error)?;
        Ok(eudaemonty::FileMetadata { size: info.size })
    }

    fn truncate(&self, path: &str, size: u64) -> Result<(), eudaemonty::Error> {
        let full_path = normalize_path(path);
        let mut lfs = self.lfs.lock().unwrap();
        lfs.truncate_path(&full_path, size)
            .map_err(lfs_error_to_eudaemonty_error)
    }

    fn truncate_existing(&self, path: &str, size: u64) -> Result<bool, eudaemonty::Error> {
        let full_path = normalize_path(path);
        let mut lfs = self.lfs.lock().unwrap();
        lfs.truncate_existing(&full_path, size)
            .map_err(lfs_error_to_eudaemonty_error)
    }

    fn punch_hole(&self, _path: &str, _offset: u64, _length: u64) -> Result<(), eudaemonty::Error> {
        Err(eudaemonty::Error::Io(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "punch_hole not supported on LFS",
        )))
    }

    fn write_string(&self, path: &str, contents: &str) -> Result<(), eudaemonty::Error> {
        let full_path = normalize_path(path);
        let mut lfs = self.lfs.lock().unwrap();

        // Ensure parent directory exists
        if let Some(parent) = std::path::Path::new(&full_path).parent() {
            let parent_str = parent.to_string_lossy();
            if parent_str != "/" && !parent_str.is_empty() {
                let _ = lfs.mkdir_all(&parent_str);
            }
        }

        lfs.write_file(&full_path, contents.as_bytes())
            .map_err(lfs_error_to_eudaemonty_error)
    }

    fn append_string(&self, path: &str, contents: &str) -> Result<(), eudaemonty::Error> {
        let full_path = normalize_path(path);
        let mut lfs = self.lfs.lock().unwrap();
        lfs.append_file(&full_path, contents.as_bytes())
            .map_err(lfs_error_to_eudaemonty_error)
    }

    fn mkdir(&self, path: &str) -> Result<(), eudaemonty::Error> {
        let full_path = normalize_path(path);
        let mut lfs = self.lfs.lock().unwrap();
        lfs.mkdir(&full_path).map_err(lfs_error_to_eudaemonty_error)
    }

    fn mkdir_all(&self, path: &str) -> Result<(), eudaemonty::Error> {
        let full_path = normalize_path(path);
        let mut lfs = self.lfs.lock().unwrap();
        lfs.mkdir_all(&full_path)
            .map_err(lfs_error_to_eudaemonty_error)
    }

    fn is_dir(&self, path: &str) -> bool {
        let full_path = normalize_path(path);
        let lfs = self.lfs.lock().unwrap();
        lfs.is_dir(&full_path)
    }

    fn read_dir(
        &self,
        path: &str,
    ) -> Result<Vec<(String, eudaemonty::DirEntry)>, eudaemonty::Error> {
        let full_path = normalize_path(path);
        let lfs = self.lfs.lock().unwrap();
        let entries = lfs
            .read_dir(&full_path)
            .map_err(lfs_error_to_eudaemonty_error)?;
        Ok(entries
            .into_iter()
            .map(|(name, info)| (name, stat_info_to_dir_entry(&info)))
            .collect())
    }

    fn stat(&self, path: &str) -> Result<eudaemonty::DirEntry, eudaemonty::Error> {
        let full_path = normalize_path(path);
        let lfs = self.lfs.lock().unwrap();
        let info = lfs
            .stat(&full_path)
            .map_err(lfs_error_to_eudaemonty_error)?;
        Ok(stat_info_to_dir_entry(&info))
    }

    fn lstat(&self, path: &str) -> Result<eudaemonty::DirEntry, eudaemonty::Error> {
        let full_path = normalize_path(path);
        let lfs = self.lfs.lock().unwrap();
        let info = lfs
            .lstat(&full_path)
            .map_err(lfs_error_to_eudaemonty_error)?;
        Ok(stat_info_to_dir_entry(&info))
    }

    fn symlink(&self, target: &str, linkpath: &str) -> Result<(), eudaemonty::Error> {
        let full_linkpath = normalize_path(linkpath);
        let mut lfs = self.lfs.lock().unwrap();
        lfs.symlink(target, &full_linkpath)
            .map_err(lfs_error_to_eudaemonty_error)
    }

    fn link(&self, src: &str, dst: &str) -> Result<(), eudaemonty::Error> {
        let full_src = normalize_path(src);
        let full_dst = normalize_path(dst);
        let mut lfs = self.lfs.lock().unwrap();
        lfs.link(&full_src, &full_dst)
            .map_err(lfs_error_to_eudaemonty_error)
    }

    fn unlink(&self, path: &str) -> Result<(), eudaemonty::Error> {
        let full_path = normalize_path(path);
        let mut lfs = self.lfs.lock().unwrap();
        lfs.remove(&full_path)
            .map_err(lfs_error_to_eudaemonty_error)
    }

    fn rmdir(&self, path: &str) -> Result<(), eudaemonty::Error> {
        let full_path = normalize_path(path);
        let mut lfs = self.lfs.lock().unwrap();
        lfs.rmdir(&full_path).map_err(lfs_error_to_eudaemonty_error)
    }

    fn readlink(&self, path: &str) -> Result<String, eudaemonty::Error> {
        let full_path = normalize_path(path);
        let lfs = self.lfs.lock().unwrap();
        lfs.readlink(&full_path)
            .map_err(lfs_error_to_eudaemonty_error)
    }

    fn set_times(
        &self,
        path: &str,
        atime: eudaemonty::TimeSpec,
        mtime: eudaemonty::TimeSpec,
    ) -> Result<(), eudaemonty::Error> {
        let full_path = normalize_path(path);
        let mut lfs = self.lfs.lock().unwrap();
        lfs.set_times(&full_path, atime, mtime)
            .map_err(lfs_error_to_eudaemonty_error)
    }

    fn lset_times(
        &self,
        path: &str,
        atime: eudaemonty::TimeSpec,
        mtime: eudaemonty::TimeSpec,
    ) -> Result<(), eudaemonty::Error> {
        let full_path = normalize_path(path);
        let mut lfs = self.lfs.lock().unwrap();
        lfs.lset_times(&full_path, atime, mtime)
            .map_err(lfs_error_to_eudaemonty_error)
    }

    fn create_file(&self, path: &str) -> Result<bool, eudaemonty::Error> {
        let full_path = normalize_path(path);
        let mut lfs = self.lfs.lock().unwrap();
        if lfs.exists(&full_path) {
            return Ok(false);
        }
        lfs.write_file(&full_path, &[])
            .map_err(lfs_error_to_eudaemonty_error)?;
        Ok(true)
    }

    fn rename(&self, src: &str, dst: &str) -> Result<(), eudaemonty::Error> {
        let full_src = normalize_path(src);
        let full_dst = normalize_path(dst);
        let mut lfs = self.lfs.lock().unwrap();
        lfs.rename(&full_src, &full_dst)
            .map_err(lfs_error_to_eudaemonty_error)
    }

    fn mkstemp(&self, template: &str) -> Result<String, eudaemonty::Error> {
        use std::time::{SystemTime, UNIX_EPOCH};

        let chars: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";

        let seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0)
            ^ (std::process::id() as u64);

        let mut state = seed;
        let mut lfs = self.lfs.lock().unwrap();

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

            let full_path = normalize_path(&path);

            if !lfs.exists(&full_path) && lfs.write_file(&full_path, &[]).is_ok() {
                return Ok(path);
            }
        }

        Err(eudaemonty::Error::Io(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "could not create unique temporary file",
        )))
    }

    fn mkdtemp(&self, template: &str) -> Result<String, eudaemonty::Error> {
        use std::time::{SystemTime, UNIX_EPOCH};

        let chars: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";

        let seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0)
            ^ (std::process::id() as u64);

        let mut state = seed;
        let mut lfs = self.lfs.lock().unwrap();

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

            let full_path = normalize_path(&path);

            if !lfs.exists(&full_path) && lfs.mkdir(&full_path).is_ok() {
                return Ok(path);
            }
        }

        Err(eudaemonty::Error::Io(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "could not create unique temporary directory",
        )))
    }

    fn list_markdown_files(&self) -> Result<Vec<String>, eudaemonty::Error> {
        let lfs = self.lfs.lock().unwrap();

        fn find_md_files<D: BlockDevice, T: Fn() -> i64>(
            lfs: &Lfs<D, T>,
            path: &str,
            results: &mut Vec<String>,
        ) -> Result<(), eudaemonty::Error> {
            let entries = lfs.read_dir(path).map_err(lfs_error_to_eudaemonty_error)?;
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

    fn free_blocks(&self) -> Option<u64> {
        Some(self.lfs.lock().unwrap().free_blocks())
    }

    fn total_blocks(&self) -> Option<u64> {
        Some(self.lfs.lock().unwrap().total_log_blocks())
    }

    fn usage_percent(&self) -> Option<u64> {
        Some(self.lfs.lock().unwrap().usage_percent())
    }

    fn clean(&self) -> Option<Result<usize, eudaemonty::Error>> {
        Some(
            self.lfs
                .lock()
                .unwrap()
                .clean()
                .map_err(lfs_error_to_eudaemonty_error),
        )
    }

    fn tree(&self) -> Option<String> {
        let lfs = self.lfs.lock().unwrap();
        tree_string(&lfs, "/").ok()
    }
}

/// Builds a tree-style string representation of the filesystem.
fn tree_string<D: BlockDevice, T: Fn() -> i64>(
    lfs: &Lfs<D, T>,
    path: &str,
) -> Result<String, eudaemonty::Error> {
    fn build_tree<D: BlockDevice, T: Fn() -> i64>(
        lfs: &Lfs<D, T>,
        path: &str,
        prefix: &str,
        is_last: bool,
    ) -> Result<String, eudaemonty::Error> {
        let info = lfs.stat(path).map_err(lfs_error_to_eudaemonty_error)?;
        let name = if path == "/" {
            "/".to_string()
        } else {
            std::path::Path::new(path)
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| path.to_string())
        };

        let mut result = String::new();
        let connector = if path == "/" {
            ""
        } else if is_last {
            "└── "
        } else {
            "├── "
        };

        match info.file_type {
            FileType::Directory => {
                result.push_str(&format!("{}{}{}/\n", prefix, connector, name));

                let entries = lfs.read_dir(path).map_err(lfs_error_to_eudaemonty_error)?;
                let mut children: Vec<_> = entries
                    .into_iter()
                    .filter(|(n, _)| n != "." && n != "..")
                    .collect();
                children.sort_by(|a, b| a.0.cmp(&b.0));

                let child_prefix = if path == "/" {
                    String::new()
                } else {
                    format!("{}{}   ", prefix, if is_last { " " } else { "│" })
                };

                for (i, (child_name, _)) in children.iter().enumerate() {
                    let child_path = if path == "/" {
                        format!("/{}", child_name)
                    } else {
                        format!("{}/{}", path, child_name)
                    };
                    let is_last_child = i == children.len() - 1;
                    result.push_str(&build_tree(lfs, &child_path, &child_prefix, is_last_child)?);
                }
            }
            FileType::RegularFile => {
                result.push_str(&format!(
                    "{}{}{} ({})\n",
                    prefix, connector, name, info.size
                ));
            }
            FileType::Symlink => {
                let target = lfs.readlink(path).unwrap_or_else(|_| "?".to_string());
                result.push_str(&format!("{}{}{} -> {}\n", prefix, connector, name, target));
            }
            FileType::Other => {
                result.push_str(&format!("{}{}{} [other]\n", prefix, connector, name));
            }
        }

        Ok(result)
    }

    build_tree(lfs, path, "", true)
}

impl<T: Fn() -> i64> LfsSyncFilesystem<MemoryBlockDevice, T> {
    /// Creates a new thread-safe filesystem with an in-memory backend.
    pub fn from_vec(data: Vec<u8>, dev: DeviceId, time_source: T) -> crate::Result<Self> {
        let lfs = Lfs::from_vec(data, dev, time_source)?;
        Ok(Self::new(lfs))
    }
}

// ============================================================================
// Helper functions
// ============================================================================

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
                    "directory" => "d",
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_size_bytes() {
        assert_eq!(parse_size("100").unwrap(), 100);
        assert_eq!(parse_size("100B").unwrap(), 100);
        println!("DEBUG: parse_size bytes works");
    }

    #[test]
    fn parse_size_kilobytes() {
        assert_eq!(parse_size("1K").unwrap(), 1024);
        assert_eq!(parse_size("64K").unwrap(), 64 * 1024);
        println!("DEBUG: parse_size kilobytes works");
    }

    #[test]
    fn parse_size_megabytes() {
        assert_eq!(parse_size("1M").unwrap(), 1024 * 1024);
        assert_eq!(parse_size("16M").unwrap(), 16 * 1024 * 1024);
        println!("DEBUG: parse_size megabytes works");
    }

    #[test]
    fn parse_size_gigabytes() {
        assert_eq!(parse_size("1G").unwrap(), 1024 * 1024 * 1024);
        println!("DEBUG: parse_size gigabytes works");
    }

    #[test]
    fn parse_size_invalid() {
        assert!(parse_size("").is_err());
        assert!(parse_size("abc").is_err());
        assert!(parse_size("1X").is_err());
        println!("DEBUG: parse_size rejects invalid input");
    }
}

//! Filesystem abstraction for sandboxed file operations.
//!
//! This module provides [`DirectoryFilesystem`], an implementation of
//! [`Filesystem`](crate::Filesystem) rooted at a directory.
//!
//! # Security Model
//!
//! The filesystem abstraction enforces path safety:
//! - All paths are resolved relative to the root directory
//! - Path traversal via `..` that escapes root is rejected
//! - Symlinks pointing outside root ARE allowed (chroot with symlink tolerance)
//!
//! This allows an agent to operate freely within a markdown wiki while preventing escape.

use std::fs;
use std::io::Seek;
use std::io::SeekFrom;
use std::io::Write;
use std::path::PathBuf;

use crate::DirEntry;
use crate::Error;
use crate::FileMetadata;
use crate::FileType;
use crate::Filesystem;
use crate::TimeSpec;
use utf8path::Path;

/// Filesystem implementation rooted at a directory.
///
/// Security model:
/// - All paths are resolved relative to the root
/// - Path traversal via `..` that escapes root is rejected
/// - Symlinks pointing outside root ARE allowed (chroot with symlink tolerance)
#[derive(Debug, Clone)]
pub struct DirectoryFilesystem {
    root: PathBuf,
    root_str: String,
}

impl DirectoryFilesystem {
    /// Creates a new filesystem rooted at the given directory.
    ///
    /// # Errors
    ///
    /// Returns an error if the path does not exist or is not a directory.
    pub fn new(root: impl AsRef<std::path::Path>) -> Result<Self, Error> {
        let root = root.as_ref().to_path_buf();
        if !root.exists() {
            return Err(Error::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("Root directory does not exist: {}", root.display()),
            )));
        }
        if !root.is_dir() {
            return Err(Error::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("Root path is not a directory: {}", root.display()),
            )));
        }
        let root_str = root.to_string_lossy().to_string();
        Ok(DirectoryFilesystem { root, root_str })
    }

    /// Resolves a relative path within the filesystem root.
    fn resolve(&self, path: &str) -> Result<PathBuf, Error> {
        if path_contains_parent_traversal(path) {
            return Err(Error::Io(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                format!("Paths containing '..' are not allowed: {}", path),
            )));
        }
        Ok(self.root.join(path))
    }
}

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

fn metadata_to_direntry(metadata: &std::fs::Metadata, is_symlink: bool) -> DirEntry {
    let file_type = if is_symlink {
        FileType::Symlink
    } else if metadata.is_dir() {
        FileType::Directory
    } else if metadata.is_file() {
        FileType::RegularFile
    } else {
        FileType::Other
    };
    let (dev, ino) = dev_ino_from_metadata(metadata);
    DirEntry {
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
    }
}

impl Filesystem for DirectoryFilesystem {
    fn root(&self) -> Path<'_> {
        Path::new(&self.root_str)
    }

    fn dup(&self) -> Self {
        self.clone()
    }

    fn read_to_string(&self, path: &str) -> Result<String, Error> {
        let full_path = self.resolve(path)?;
        fs::read_to_string(&full_path).map_err(Error::Io)
    }

    fn exists(&self, path: &str) -> bool {
        self.resolve(path).map(|p| p.exists()).unwrap_or(false)
    }

    fn metadata(&self, path: &str) -> Result<FileMetadata, Error> {
        let full_path = self.resolve(path)?;
        let meta = fs::metadata(&full_path).map_err(Error::Io)?;
        Ok(FileMetadata { size: meta.len() })
    }

    fn truncate(&self, path: &str, size: u64) -> Result<(), Error> {
        let full_path = self.resolve(path)?;
        let file = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .open(&full_path)
            .map_err(Error::Io)?;
        file.set_len(size).map_err(Error::Io)
    }

    fn truncate_existing(&self, path: &str, size: u64) -> Result<bool, Error> {
        let full_path = self.resolve(path)?;
        match fs::OpenOptions::new()
            .write(true)
            .truncate(false)
            .open(&full_path)
        {
            Ok(file) => {
                file.set_len(size).map_err(Error::Io)?;
                Ok(true)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(Error::Io(e)),
        }
    }

    fn punch_hole(&self, path: &str, offset: u64, length: u64) -> Result<(), Error> {
        let full_path = self.resolve(path)?;
        let mut file = fs::OpenOptions::new()
            .write(true)
            .read(true)
            .open(&full_path)
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
        let full_path = self.resolve(path)?;
        if let Some(parent) = full_path.parent() {
            fs::create_dir_all(parent).map_err(Error::Io)?;
        }
        fs::write(&full_path, contents).map_err(Error::Io)
    }

    fn append_string(&self, path: &str, contents: &str) -> Result<(), Error> {
        let full_path = self.resolve(path)?;
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&full_path)
            .map_err(Error::Io)?;
        file.write_all(contents.as_bytes()).map_err(Error::Io)
    }

    fn mkdir(&self, path: &str) -> Result<(), Error> {
        let full_path = self.resolve(path)?;
        fs::create_dir(&full_path).map_err(Error::Io)
    }

    fn mkdir_all(&self, path: &str) -> Result<(), Error> {
        let full_path = self.resolve(path)?;
        fs::create_dir_all(&full_path).map_err(Error::Io)
    }

    fn is_dir(&self, path: &str) -> bool {
        self.resolve(path).map(|p| p.is_dir()).unwrap_or(false)
    }

    fn read_dir(&self, path: &str) -> Result<Vec<(String, DirEntry)>, Error> {
        let full_path = self.resolve(path)?;
        let mut entries = Vec::new();
        for entry in fs::read_dir(&full_path).map_err(Error::Io)? {
            let entry = entry.map_err(Error::Io)?;
            let name = entry.file_name().to_string_lossy().to_string();
            let metadata = entry.metadata().map_err(Error::Io)?;
            let is_symlink = entry.file_type().map(|ft| ft.is_symlink()).unwrap_or(false);
            entries.push((name, metadata_to_direntry(&metadata, is_symlink)));
        }
        Ok(entries)
    }

    fn stat(&self, path: &str) -> Result<DirEntry, Error> {
        let full_path = self.resolve(path)?;
        let metadata = fs::metadata(&full_path).map_err(Error::Io)?;
        Ok(metadata_to_direntry(&metadata, false))
    }

    fn lstat(&self, path: &str) -> Result<DirEntry, Error> {
        let full_path = self.resolve(path)?;
        let metadata = fs::symlink_metadata(&full_path).map_err(Error::Io)?;
        let is_symlink = metadata.is_symlink();
        Ok(metadata_to_direntry(&metadata, is_symlink))
    }

    fn symlink(&self, target: &str, linkpath: &str) -> Result<(), Error> {
        let full_linkpath = self.resolve(linkpath)?;
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(target, &full_linkpath).map_err(Error::Io)
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
        let full_src = self.resolve(src)?;
        let full_dst = self.resolve(dst)?;
        fs::hard_link(&full_src, &full_dst).map_err(Error::Io)
    }

    fn unlink(&self, path: &str) -> Result<(), Error> {
        let full_path = self.resolve(path)?;
        fs::remove_file(&full_path).map_err(Error::Io)
    }

    fn rmdir(&self, path: &str) -> Result<(), Error> {
        let full_path = self.resolve(path)?;
        fs::remove_dir(&full_path).map_err(Error::Io)
    }

    fn readlink(&self, path: &str) -> Result<String, Error> {
        let full_path = self.resolve(path)?;
        let target = fs::read_link(&full_path).map_err(Error::Io)?;
        target.to_str().map(|s| s.to_string()).ok_or_else(|| {
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

        let full_path = self.resolve(path)?;
        let file = fs::File::options()
            .write(true)
            .open(&full_path)
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
        let full_path = self.resolve(path)?;
        let metadata = fs::symlink_metadata(&full_path).map_err(Error::Io)?;
        if metadata.is_symlink() {
            return Ok(());
        }
        self.set_times(path, atime, mtime)
    }

    fn create_file(&self, path: &str) -> Result<bool, Error> {
        let full_path = self.resolve(path)?;
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&full_path)
        {
            Ok(_) => Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
            Err(e) => Err(Error::Io(e)),
        }
    }

    fn rename(&self, src: &str, dst: &str) -> Result<(), Error> {
        let full_src = self.resolve(src)?;
        let full_dst = self.resolve(dst)?;
        fs::rename(&full_src, &full_dst).map_err(Error::Io)
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

            let full_path = self.resolve(&path)?;
            match fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&full_path)
            {
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

            let full_path = self.resolve(&path)?;
            match fs::create_dir(&full_path) {
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

    fn list_markdown_files(&self) -> Result<Vec<String>, Error> {
        fn find_md_files(
            dir: &std::path::Path,
            base: &std::path::Path,
            results: &mut Vec<String>,
        ) -> Result<(), Error> {
            for entry in fs::read_dir(dir).map_err(Error::Io)? {
                let entry = entry.map_err(Error::Io)?;
                let path = entry.path();
                if path.is_dir() {
                    find_md_files(&path, base, results)?;
                } else if path.is_file()
                    && path
                        .extension()
                        .is_some_and(|ext| ext.eq_ignore_ascii_case("md"))
                    && let Ok(rel_path) = path.strip_prefix(base)
                {
                    results.push(rel_path.to_string_lossy().replace('\\', "/"));
                }
            }
            Ok(())
        }

        let mut results = Vec::new();
        find_md_files(&self.root, &self.root, &mut results)?;
        results.sort();
        Ok(results)
    }
}

/// Checks if a path contains parent directory traversal (`..`).
///
/// We reject all paths containing `..` for simplicity and security.
fn path_contains_parent_traversal(path: &str) -> bool {
    let utf8_path = utf8path::Path::new(path);

    for component in utf8_path.components() {
        if matches!(component, utf8path::Component::ParentDir) {
            return true;
        }
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    // ========================================================================
    // path_contains_parent_traversal tests
    // ========================================================================

    #[test]
    fn parent_traversal_safe_path() {
        assert!(!path_contains_parent_traversal("foo/bar.md"));
        assert!(!path_contains_parent_traversal("a/b/c"));
        println!("DEBUG: safe paths don't contain parent traversal");
    }

    #[test]
    fn parent_traversal_detected() {
        assert!(path_contains_parent_traversal("../etc/passwd"));
        assert!(path_contains_parent_traversal("foo/../../bar"));
        assert!(path_contains_parent_traversal("foo/../bar"));
        println!("DEBUG: parent traversal detected");
    }

    // ========================================================================
    // DirectoryFilesystem tests
    // ========================================================================

    #[test]
    fn directory_fs_new_valid_dir() {
        let dir = env::current_dir().unwrap();
        let fs = DirectoryFilesystem::new(&dir);
        assert!(fs.is_ok());
        println!("DEBUG: DirectoryFilesystem created for valid dir");
    }

    #[test]
    fn directory_fs_new_nonexistent() {
        let result = DirectoryFilesystem::new("/nonexistent/path/12345");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("does not exist"));
        println!("DEBUG: error for nonexistent path: {}", err);
    }

    #[test]
    fn directory_fs_new_not_a_dir() {
        let dir = env::current_dir().unwrap();
        let file_path = dir.join("Cargo.toml");
        let result = DirectoryFilesystem::new(&file_path);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("not a directory"));
        println!("DEBUG: error for non-directory path: {}", err);
    }

    #[test]
    fn directory_fs_root() {
        let dir = env::current_dir().unwrap();
        let fs = DirectoryFilesystem::new(&dir).unwrap();
        assert_eq!(fs.root().as_str(), dir.to_str().unwrap());
        println!("DEBUG: root returns the correct path");
    }

    #[test]
    fn directory_fs_read_write_roundtrip() {
        let dir = env::current_dir().unwrap();
        let fs = DirectoryFilesystem::new(&dir).unwrap();
        let test_file = "test_fs_roundtrip.md";

        fs.write_string(test_file, "# Test Content").unwrap();
        let content = fs.read_to_string(test_file).unwrap();
        assert_eq!(content, "# Test Content");

        // Cleanup
        std::fs::remove_file(dir.join(test_file)).unwrap();
        println!("DEBUG: read/write roundtrip works");
    }

    #[test]
    fn directory_fs_exists() {
        let dir = env::current_dir().unwrap();
        let fs = DirectoryFilesystem::new(&dir).unwrap();

        assert!(fs.exists("Cargo.toml"));
        assert!(!fs.exists("nonexistent_file_12345.txt"));
        println!("DEBUG: exists works correctly");
    }

    #[test]
    fn directory_fs_list_markdown_files() {
        let dir = env::current_dir().unwrap();
        let fs = DirectoryFilesystem::new(&dir).unwrap();

        // Create test files
        fs.write_string("test_list_a.md", "# A").unwrap();
        fs.write_string("test_list_b.md", "# B").unwrap();

        let files = fs.list_markdown_files().unwrap();
        assert!(files.iter().any(|f| f == "test_list_a.md"));
        assert!(files.iter().any(|f| f == "test_list_b.md"));

        // Cleanup
        std::fs::remove_file(dir.join("test_list_a.md")).unwrap();
        std::fs::remove_file(dir.join("test_list_b.md")).unwrap();
        println!("DEBUG: list_markdown_files finds md files");
    }

    #[test]
    fn directory_fs_parent_dir_rejected() {
        let dir = env::current_dir().unwrap();
        let fs = DirectoryFilesystem::new(&dir).unwrap();

        let result = fs.read_to_string("../../../etc/passwd");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("are not allowed"));
        println!("DEBUG: parent dir is rejected: {}", err);
    }

    #[test]
    fn directory_fs_read_not_found() {
        let dir = env::current_dir().unwrap();
        let fs = DirectoryFilesystem::new(&dir).unwrap();

        let result = fs.read_to_string("nonexistent_file_12345.md");
        assert!(result.is_err());
        println!("DEBUG: read returns error for missing file");
    }

    #[test]
    fn directory_fs_nested_write() {
        let dir = env::current_dir().unwrap();
        let fs = DirectoryFilesystem::new(&dir).unwrap();

        // Write to a nested path
        fs.write_string("test_nested_dir/test.md", "# Nested")
            .unwrap();
        let content = fs.read_to_string("test_nested_dir/test.md").unwrap();
        assert_eq!(content, "# Nested");

        // Cleanup
        std::fs::remove_file(dir.join("test_nested_dir/test.md")).unwrap();
        std::fs::remove_dir(dir.join("test_nested_dir")).unwrap();
        println!("DEBUG: nested write creates parent dirs");
    }
}

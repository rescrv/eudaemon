//! Filesystem abstraction for sandboxed file operations.
//!
//! This module provides a [`Filesystem`] trait that abstracts over file I/O operations,
//! restricted to a working directory. Two implementations are provided:
//!
//! - [`DirectoryFilesystem`] - Production implementation rooted at a real directory
//! - [`InMemoryFilesystem`] - In-memory implementation for testing
//!
//! # Security Model
//!
//! The filesystem abstraction enforces path safety:
//! - All paths are resolved relative to the root directory
//! - Path traversal via `..` that escapes root is rejected
//! - Symlinks pointing outside root ARE allowed (chroot with symlink tolerance)
//!
//! This allows an agent to operate freely within a markdown wiki while preventing escape.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::RwLock;

use crate::error::{SError, SResult};

/// Abstraction over filesystem operations, restricted to a working directory.
pub trait Filesystem: Send + Sync {
    /// Returns the root directory path (for display purposes).
    fn root(&self) -> &Path;

    /// Lists markdown files, returns paths relative to root.
    fn list_markdown_files(&self) -> SResult<Vec<String>>;

    /// Reads a file as a string. Path is relative to root.
    fn read(&self, path: &str) -> SResult<String>;

    /// Writes content to a file. Path is relative to root.
    fn write(&self, path: &str, content: &str) -> SResult<()>;

    /// Checks if a path exists and is a file.
    fn exists(&self, path: &str) -> bool;
}

/// Filesystem implementation rooted at a directory.
///
/// Security model:
/// - All paths are resolved relative to the root
/// - Path traversal via `..` that escapes root is rejected
/// - Symlinks pointing outside root ARE allowed (chroot with symlink tolerance)
#[derive(Debug)]
pub struct DirectoryFilesystem {
    root: PathBuf,
}

impl DirectoryFilesystem {
    /// Creates a new filesystem rooted at the given directory.
    ///
    /// # Errors
    ///
    /// Returns an error if the path does not exist or is not a directory.
    pub fn new(root: impl AsRef<Path>) -> SResult<Self> {
        let root = root.as_ref().to_path_buf();
        if !root.exists() {
            return Err(SError::new("filesystem")
                .with_code("not-found")
                .with_message("Root directory does not exist")
                .with_string_field("path", &root.display().to_string()));
        }
        if !root.is_dir() {
            return Err(SError::new("filesystem")
                .with_code("not-a-directory")
                .with_message("Root path is not a directory")
                .with_string_field("path", &root.display().to_string()));
        }
        Ok(DirectoryFilesystem { root })
    }

    /// Resolves a relative path within the filesystem root.
    ///
    /// Checks for path traversal attacks using utf8path.
    /// Does NOT follow symlinks for the security check (only validates logical path).
    fn resolve(&self, path: &str) -> SResult<PathBuf> {
        if path_contains_parent_traversal(path) {
            return Err(SError::new("filesystem")
                .with_code("parent-dir-not-allowed")
                .with_message("Paths containing '..' are not allowed")
                .with_string_field("path", path));
        }

        Ok(self.root.join(path))
    }
}

/// Recursively finds all markdown files in a directory.
fn find_markdown_files_recursive(dir: &Path, base: &Path) -> SResult<Vec<String>> {
    let mut files = Vec::new();

    let entries = fs::read_dir(dir).map_err(|e| {
        SError::new("filesystem")
            .with_code("io-error")
            .with_message("Failed to read directory")
            .with_string_field("path", &dir.display().to_string())
            .with_string_field("error", &e.to_string())
    })?;

    for entry in entries {
        let entry = entry.map_err(|e| {
            SError::new("filesystem")
                .with_code("io-error")
                .with_message("Failed to read directory entry")
                .with_string_field("error", &e.to_string())
        })?;

        let path = entry.path();
        if path.is_dir() {
            files.extend(find_markdown_files_recursive(&path, base)?);
        } else if path.is_file()
            && path
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("md"))
        {
            // Get path relative to base
            if let Ok(rel_path) = path.strip_prefix(base) {
                files.push(rel_path.to_string_lossy().replace('\\', "/"));
            }
        }
    }

    Ok(files)
}

impl Filesystem for DirectoryFilesystem {
    fn root(&self) -> &Path {
        &self.root
    }

    fn list_markdown_files(&self) -> SResult<Vec<String>> {
        find_markdown_files_recursive(&self.root, &self.root)
    }

    fn read(&self, path: &str) -> SResult<String> {
        let full_path = self.resolve(path)?;
        fs::read_to_string(&full_path).map_err(|e| {
            SError::new("filesystem")
                .with_code("io-error")
                .with_message("Failed to read file")
                .with_string_field("path", path)
                .with_string_field("error", &e.to_string())
        })
    }

    fn write(&self, path: &str, content: &str) -> SResult<()> {
        let full_path = self.resolve(path)?;

        // Ensure parent directory exists
        if let Some(parent) = full_path.parent() {
            fs::create_dir_all(parent).map_err(|e| {
                SError::new("filesystem")
                    .with_code("io-error")
                    .with_message("Failed to create parent directory")
                    .with_string_field("path", &parent.display().to_string())
                    .with_string_field("error", &e.to_string())
            })?;
        }

        fs::write(&full_path, content).map_err(|e| {
            SError::new("filesystem")
                .with_code("io-error")
                .with_message("Failed to write file")
                .with_string_field("path", path)
                .with_string_field("error", &e.to_string())
        })
    }

    fn exists(&self, path: &str) -> bool {
        self.resolve(path).map(|p| p.is_file()).unwrap_or(false)
    }
}

/// In-memory filesystem for testing.
pub struct InMemoryFilesystem {
    root_name: PathBuf,
    files: RwLock<HashMap<String, String>>,
}

impl InMemoryFilesystem {
    /// Creates a new empty in-memory filesystem.
    pub fn new() -> Self {
        InMemoryFilesystem {
            root_name: PathBuf::from("/memory"),
            files: RwLock::new(HashMap::new()),
        }
    }

    /// Creates an in-memory filesystem with initial files.
    pub fn with_files(files: HashMap<String, String>) -> Self {
        InMemoryFilesystem {
            root_name: PathBuf::from("/memory"),
            files: RwLock::new(files),
        }
    }

    /// Resolves and validates a path for the in-memory filesystem.
    fn resolve(&self, path: &str) -> SResult<String> {
        if path_contains_parent_traversal(path) {
            return Err(SError::new("filesystem")
                .with_code("parent-dir-not-allowed")
                .with_message("Paths containing '..' are not allowed")
                .with_string_field("path", path));
        }

        Ok(path.to_string())
    }
}

impl Default for InMemoryFilesystem {
    fn default() -> Self {
        Self::new()
    }
}

impl Filesystem for InMemoryFilesystem {
    fn root(&self) -> &Path {
        &self.root_name
    }

    fn list_markdown_files(&self) -> SResult<Vec<String>> {
        let files = self.files.read().unwrap();
        let mut result: Vec<String> = files
            .keys()
            .filter(|k| k.ends_with(".md") || k.ends_with(".MD"))
            .cloned()
            .collect();
        result.sort();
        Ok(result)
    }

    fn read(&self, path: &str) -> SResult<String> {
        let normalized = self.resolve(path)?;
        let files = self.files.read().unwrap();
        files.get(&normalized).cloned().ok_or_else(|| {
            SError::new("filesystem")
                .with_code("not-found")
                .with_message("File not found")
                .with_string_field("path", path)
        })
    }

    fn write(&self, path: &str, content: &str) -> SResult<()> {
        let normalized = self.resolve(path)?;
        let mut files = self.files.write().unwrap();
        files.insert(normalized, content.to_string());
        Ok(())
    }

    fn exists(&self, path: &str) -> bool {
        self.resolve(path)
            .map(|normalized| {
                let files = self.files.read().unwrap();
                files.contains_key(&normalized)
            })
            .unwrap_or(false)
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
    // InMemoryFilesystem tests
    // ========================================================================

    #[test]
    fn in_memory_new_is_empty() {
        let fs = InMemoryFilesystem::new();
        let files = fs.list_markdown_files().unwrap();
        assert!(files.is_empty());
        println!("DEBUG: new in-memory fs is empty");
    }

    #[test]
    fn in_memory_default_is_new() {
        let fs: InMemoryFilesystem = Default::default();
        let files = fs.list_markdown_files().unwrap();
        assert!(files.is_empty());
        println!("DEBUG: default matches new");
    }

    #[test]
    fn in_memory_write_and_read() {
        let fs = InMemoryFilesystem::new();
        fs.write("test.md", "# Hello").unwrap();
        let content = fs.read("test.md").unwrap();
        assert_eq!(content, "# Hello");
        println!("DEBUG: write/read roundtrip works");
    }

    #[test]
    fn in_memory_exists() {
        let fs = InMemoryFilesystem::new();
        assert!(!fs.exists("test.md"));
        fs.write("test.md", "content").unwrap();
        assert!(fs.exists("test.md"));
        println!("DEBUG: exists works correctly");
    }

    #[test]
    fn in_memory_list_markdown_files() {
        let fs = InMemoryFilesystem::new();
        fs.write("readme.md", "# Readme").unwrap();
        fs.write("docs/guide.md", "# Guide").unwrap();
        fs.write("config.json", "{}").unwrap();

        let files = fs.list_markdown_files().unwrap();
        assert!(files.contains(&"readme.md".to_string()));
        assert!(files.contains(&"docs/guide.md".to_string()));
        assert!(!files.contains(&"config.json".to_string()));
        println!("DEBUG: list_markdown_files filters by extension");
    }

    #[test]
    fn in_memory_with_files() {
        let mut initial = HashMap::new();
        initial.insert("a.md".to_string(), "content a".to_string());
        initial.insert("b.md".to_string(), "content b".to_string());

        let fs = InMemoryFilesystem::with_files(initial);
        assert_eq!(fs.read("a.md").unwrap(), "content a");
        assert_eq!(fs.read("b.md").unwrap(), "content b");
        println!("DEBUG: with_files initializes correctly");
    }

    #[test]
    fn in_memory_read_not_found() {
        let fs = InMemoryFilesystem::new();
        let result = fs.read("nonexistent.md");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("not-found"));
        println!("DEBUG: read returns error for missing file");
    }

    #[test]
    fn in_memory_parent_dir_rejected() {
        let fs = InMemoryFilesystem::new();
        let result = fs.read("../etc/passwd");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("parent-dir-not-allowed"));
        println!("DEBUG: parent dir is rejected");
    }

    #[test]
    fn in_memory_root_returns_memory() {
        let fs = InMemoryFilesystem::new();
        assert_eq!(fs.root(), Path::new("/memory"));
        println!("DEBUG: root returns /memory");
    }

    #[test]
    fn in_memory_write_overwrites_existing() {
        let fs = InMemoryFilesystem::new();
        fs.write("test.md", "original").unwrap();
        fs.write("test.md", "updated").unwrap();
        let content = fs.read("test.md").unwrap();
        assert_eq!(content, "updated");
        println!("DEBUG: write overwrites existing content");
    }

    #[test]
    fn in_memory_write_parent_dir_rejected() {
        let fs = InMemoryFilesystem::new();
        let result = fs.write("../escape.md", "malicious");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("parent-dir-not-allowed"));
        println!("DEBUG: write rejects parent dir traversal");
    }

    #[test]
    fn in_memory_exists_parent_dir_rejected() {
        let fs = InMemoryFilesystem::new();
        let result = fs.exists("../etc/passwd");
        assert!(!result);
        println!("DEBUG: exists returns false for parent dir traversal");
    }

    #[test]
    fn in_memory_list_uppercase_md_extension() {
        let fs = InMemoryFilesystem::new();
        fs.write("lowercase.md", "# Lower").unwrap();
        fs.write("uppercase.MD", "# Upper").unwrap();
        fs.write("other.txt", "text").unwrap();

        let files = fs.list_markdown_files().unwrap();
        assert_eq!(files.len(), 2);
        assert!(files.contains(&"lowercase.md".to_string()));
        assert!(files.contains(&"uppercase.MD".to_string()));
        println!("DEBUG: list_markdown_files includes .MD extension");
    }

    #[test]
    fn in_memory_list_is_sorted() {
        let fs = InMemoryFilesystem::new();
        fs.write("z.md", "z").unwrap();
        fs.write("a.md", "a").unwrap();
        fs.write("m.md", "m").unwrap();

        let files = fs.list_markdown_files().unwrap();
        assert_eq!(files, vec!["a.md", "m.md", "z.md"]);
        println!("DEBUG: list_markdown_files returns sorted results");
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
        assert!(err.to_string().contains("not-found"));
        println!("DEBUG: error for nonexistent path");
    }

    #[test]
    fn directory_fs_new_not_a_dir() {
        let dir = env::current_dir().unwrap();
        let file_path = dir.join("Cargo.toml");
        let result = DirectoryFilesystem::new(&file_path);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("not-a-directory"));
        println!("DEBUG: error for non-directory path");
    }

    #[test]
    fn directory_fs_root() {
        let dir = env::current_dir().unwrap();
        let fs = DirectoryFilesystem::new(&dir).unwrap();
        assert_eq!(fs.root(), dir.as_path());
        println!("DEBUG: root returns the correct path");
    }

    #[test]
    fn directory_fs_read_write_roundtrip() {
        let dir = env::current_dir().unwrap();
        let fs = DirectoryFilesystem::new(&dir).unwrap();
        let test_file = "test_fs_roundtrip.md";

        fs.write(test_file, "# Test Content").unwrap();
        let content = fs.read(test_file).unwrap();
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
        fs.write("test_list_a.md", "# A").unwrap();
        fs.write("test_list_b.md", "# B").unwrap();

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

        let result = fs.read("../../../etc/passwd");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("parent-dir-not-allowed"));
        println!("DEBUG: parent dir is rejected");
    }

    #[test]
    fn directory_fs_read_not_found() {
        let dir = env::current_dir().unwrap();
        let fs = DirectoryFilesystem::new(&dir).unwrap();

        let result = fs.read("nonexistent_file_12345.md");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("io-error"));
        println!("DEBUG: read returns error for missing file");
    }

    #[test]
    fn directory_fs_nested_write() {
        let dir = env::current_dir().unwrap();
        let fs = DirectoryFilesystem::new(&dir).unwrap();

        // Write to a nested path
        fs.write("test_nested_dir/test.md", "# Nested").unwrap();
        let content = fs.read("test_nested_dir/test.md").unwrap();
        assert_eq!(content, "# Nested");

        // Cleanup
        std::fs::remove_file(dir.join("test_nested_dir/test.md")).unwrap();
        std::fs::remove_dir(dir.join("test_nested_dir")).unwrap();
        println!("DEBUG: nested write creates parent dirs");
    }
}

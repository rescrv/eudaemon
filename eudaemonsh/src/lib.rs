use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;

use eudaemonfs::Lfs;
use eudaemonfs::MemoryBlockDevice;
use eudaemonty::SyncMutFilesystem;
use utf8path::Path;

mod builtins;

pub use builtins::lookup_bin;
pub use builtins::sh;
pub use eudaemonfs::DeviceId;
pub use eudaemonfs::FileBlockDevice;
pub use eudaemonfs::FileLfsExt;
pub use eudaemonfs::LfsExt;
pub use eudaemonfs::MemoryLfsExt;
pub use eudaemonfs::TestFilesystemExt;
pub use eudaemonty::DirEntry;
pub use eudaemonty::FileMetadata;
pub use eudaemonty::FileType;
pub use eudaemonty::Filesystem;

pub use eudaemonty::FileAppendStdioOut;
pub use eudaemonty::FileStdioOut;
pub use eudaemonty::PipeReader;
pub use eudaemonty::PipeWriter;
pub use eudaemonty::StdioIn;
pub use eudaemonty::StdioOut;
pub use eudaemonty::StringStdioIn;
pub use eudaemonty::StringStdioOut;
pub use eudaemonty::TimeSpec;
pub use eudaemonty::mkpipe;

/// A filesystem backed by eudaemonfs using in-memory storage.
pub type EudaemonFilesystem<T> = SyncMutFilesystem<Lfs<MemoryBlockDevice, T>>;

/// A filesystem backed by eudaemonfs using file-backed storage.
pub type FileBackedEudaemonFilesystem<T> = SyncMutFilesystem<Lfs<FileBlockDevice, T>>;

/// Alias for filesystem errors from eudaemonty.
pub use eudaemonty::Error as FsError;

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

impl From<eudaemonty::Error> for Error {
    fn from(err: eudaemonty::Error) -> Self {
        match err {
            eudaemonty::Error::Io(e) => Self::Io(e),
        }
    }
}

/// The execution environment for a command.
///
/// # Variable Storage Design
///
/// Shell variables are stored in two separate maps:
/// - `env`: Environment variables that are exported to child processes
/// - `vars`: Shell-local variables that are not exported
///
/// During variable lookup (for expansion), shell variables (`vars`) take precedence over
/// environment variables (`env`). This matches POSIX shell semantics where a local variable
/// shadows an exported one of the same name.
///
/// The `set` builtin stores variables in `vars`. The `export` builtin moves a variable from
/// `vars` to `env` (or marks an existing env var as exported). The `unset` builtin removes
/// a variable from both maps.
///
/// This two-map approach was chosen over an enum-based single map because:
/// 1. It maintains a clear separation between exported and non-exported variables
/// 2. The `env` map can be passed directly to child processes without filtering
/// 3. It avoids the overhead of copying/filtering when creating variable providers for shvar
///
/// Both maps are plain `HashMap`s (not shared via `Arc<Mutex<>>`). The shell's `run` function
/// handles `set`, `unset`, and `export` specially by mutating its own copy of the environment
/// before passing snapshots to child commands.
pub struct Environment<SI, SO, SE, FS>
where
    SI: StdioIn,
    SO: StdioOut,
    SE: StdioOut,
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
    /// Environment variables (exported, passed to child processes).
    pub env: HashMap<String, String>,
    /// Shell-local variables (not exported, not passed to child processes).
    ///
    /// These take precedence over `env` during variable expansion.
    pub vars: HashMap<String, String>,
    /// Command-line arguments.
    pub args: Vec<String>,
    /// Current working directory.
    pub cwd: Path<'static>,
    /// Whether an exit has been signaled by the `exit` builtin.
    pub exit_signaled: Arc<AtomicBool>,
}

impl<SI, SO, SE, FS> Environment<SI, SO, SE, FS>
where
    SI: StdioIn,
    SO: StdioOut,
    SE: StdioOut,
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
            vars: self.vars.clone(),
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
pub struct Command<SI, SO, SE, FS>
where
    SI: StdioIn,
    SO: StdioOut,
    SE: StdioOut,
    FS: Filesystem,
{
    env: Environment<SI, SO, SE, FS>,
    bin: fn(&Environment<SI, SO, SE, FS>) -> Result<ExitCode, Error>,
}

impl<SI, SO, SE, FS> Command<SI, SO, SE, FS>
where
    SI: StdioIn,
    SO: StdioOut,
    SE: StdioOut,
    FS: Filesystem + 'static,
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

/// Resolve a path relative to a current working directory.
///
/// This function handles:
/// - Absolute paths (starting with `/`)
/// - Current directory (`.`)
/// - Parent directory (`..`)
/// - Relative paths with `./` or `../` prefixes
/// - Simple relative paths
pub fn resolve_path(cwd: &str, path: &str) -> String {
    if path.starts_with('/') {
        path.to_string()
    } else if path == "." {
        cwd.to_string()
    } else if path == ".." {
        let mut parts: Vec<&str> = cwd.split('/').filter(|s| !s.is_empty()).collect();
        parts.pop();
        if parts.is_empty() {
            "/".to_string()
        } else {
            format!("/{}", parts.join("/"))
        }
    } else if let Some(rest) = path.strip_prefix("./") {
        format!("{}/{}", cwd.trim_end_matches('/'), rest)
    } else if let Some(rest) = path.strip_prefix("../") {
        let mut parts: Vec<&str> = cwd.split('/').filter(|s| !s.is_empty()).collect();
        parts.pop();
        let parent = if parts.is_empty() {
            "/".to_string()
        } else {
            format!("/{}", parts.join("/"))
        };
        resolve_path(&parent, rest)
    } else {
        format!("{}/{}", cwd.trim_end_matches('/'), path)
    }
}

/// Format a byte size in human-readable form using powers of 1024.
///
/// Returns a string like "100B", "1.5K", "10M", etc.
pub fn format_human_size(size: u64) -> String {
    format_human_size_with_base(size, 1024)
}

/// Format a byte size in human-readable form using a custom base.
///
/// Use base 1024 for traditional units (KiB, MiB, etc.) or 1000 for SI units (KB, MB, etc.).
pub fn format_human_size_with_base(size: u64, base: u64) -> String {
    const UNITS: &[&str] = &["B", "K", "M", "G", "T", "P"];
    let mut size = size as f64;
    let mut unit_idx = 0;
    let base = base as f64;

    while size >= base && unit_idx < UNITS.len() - 1 {
        size /= base;
        unit_idx += 1;
    }

    if unit_idx == 0 || size >= 10.0 {
        format!("{:>4}{}", size as u64, UNITS[unit_idx])
    } else {
        format!("{:>4.1}{}", size, UNITS[unit_idx])
    }
}

/// Extract a user-friendly message from an Error.
///
/// Converts common I/O error kinds to human-readable strings like
/// "No such file or directory" instead of the default Rust error messages.
pub fn io_error_message(e: &Error) -> String {
    match e {
        Error::Io(io_err) => match io_err.kind() {
            std::io::ErrorKind::NotFound => "No such file or directory".to_string(),
            std::io::ErrorKind::DirectoryNotEmpty => "Directory not empty".to_string(),
            std::io::ErrorKind::NotADirectory => "Not a directory".to_string(),
            std::io::ErrorKind::IsADirectory => "Is a directory".to_string(),
            std::io::ErrorKind::PermissionDenied => "Permission denied".to_string(),
            std::io::ErrorKind::AlreadyExists => "File exists".to_string(),
            _ => io_err.to_string(),
        },
        Error::Shvar(e) => format!("{:?}", e),
        Error::EmptyCommand => "empty command".to_string(),
        Error::UnknownBinary(b) => format!("unknown binary: {}", b),
    }
}

/// Extract a user-friendly message from a filesystem error.
///
/// Converts common I/O error kinds to human-readable strings like
/// "No such file or directory" instead of the default Rust error messages.
pub fn fs_error_message(e: &FsError) -> String {
    match e {
        FsError::Io(io_err) => match io_err.kind() {
            std::io::ErrorKind::NotFound => "No such file or directory".to_string(),
            std::io::ErrorKind::DirectoryNotEmpty => "Directory not empty".to_string(),
            std::io::ErrorKind::NotADirectory => "Not a directory".to_string(),
            std::io::ErrorKind::IsADirectory => "Is a directory".to_string(),
            std::io::ErrorKind::PermissionDenied => "Permission denied".to_string(),
            std::io::ErrorKind::AlreadyExists => "File exists".to_string(),
            _ => io_err.to_string(),
        },
    }
}

/// Parse a size string that may include suffixes (b, k, m, g).
///
/// Supported suffixes (case-insensitive):
/// - `b`: 512-byte blocks
/// - `k`: kilobytes (1024 bytes)
/// - `m`: megabytes (1024*1024 bytes)
/// - `g`: gigabytes (1024*1024*1024 bytes)
///
/// Returns `None` if the string is empty, contains invalid characters,
/// or would overflow when multiplied by the suffix.
pub fn parse_size(s: &str) -> Option<u64> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }

    let (num_str, multiplier) =
        if let Some(prefix) = s.strip_suffix(|c: char| c.is_ascii_alphabetic()) {
            let suffix = s.chars().last()?;
            let mult = match suffix.to_ascii_lowercase() {
                'b' => 512,
                'k' => 1024,
                'm' => 1024 * 1024,
                'g' => 1024 * 1024 * 1024,
                _ => return None,
            };
            (prefix, mult)
        } else {
            (s, 1)
        };

    let num: u64 = num_str.parse().ok()?;
    num.checked_mul(multiplier)
}

/// Test utilities for creating test environments with EudaemonFilesystem.
#[cfg(test)]
pub mod test_utils {
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;

    use utf8path::Path;

    use crate::DeviceId;
    use crate::Environment;
    use crate::EudaemonFilesystem;
    use crate::MemoryLfsExt;
    use crate::StringStdioIn;
    use crate::StringStdioOut;

    /// Time source function that always returns zero (for deterministic tests).
    fn zero_time() -> i64 {
        0
    }

    /// Type alias for the EudaemonFilesystem with a zero time source used in tests.
    pub type TestFilesystem = EudaemonFilesystem<fn() -> i64>;

    /// A builder for creating test environments with EudaemonFilesystem.
    ///
    /// This builder creates environments backed by a real log-structured filesystem
    /// on an in-memory block device.
    pub struct TestEnvBuilder {
        args: Vec<String>,
        stdin: String,
        env_vars: HashMap<String, String>,
        cwd: String,
        fs_size_blocks: usize,
    }

    impl Default for TestEnvBuilder {
        fn default() -> Self {
            Self::new()
        }
    }

    impl TestEnvBuilder {
        /// Create a new test environment builder with default settings.
        ///
        /// The default filesystem size is 256 blocks (1MB).
        pub fn new() -> Self {
            Self {
                args: Vec::new(),
                stdin: String::new(),
                env_vars: HashMap::new(),
                cwd: "/".to_string(),
                fs_size_blocks: 256,
            }
        }

        /// Set the command-line arguments.
        pub fn args(mut self, args: Vec<&str>) -> Self {
            self.args = args.into_iter().map(|s| s.to_string()).collect();
            self
        }

        /// Set the stdin content.
        pub fn stdin(mut self, stdin: &str) -> Self {
            self.stdin = stdin.to_string();
            self
        }

        /// Set environment variables.
        pub fn env_vars(mut self, vars: HashMap<String, String>) -> Self {
            self.env_vars = vars;
            self
        }

        /// Add a single environment variable.
        pub fn env_var(mut self, key: &str, value: &str) -> Self {
            self.env_vars.insert(key.to_string(), value.to_string());
            self
        }

        /// Set the current working directory.
        pub fn cwd(mut self, cwd: &str) -> Self {
            self.cwd = cwd.to_string();
            self
        }

        /// Set the filesystem size in 4KB blocks.
        ///
        /// Default is 256 blocks (1MB). Minimum is 16 blocks (64KB).
        pub fn fs_size_blocks(mut self, blocks: usize) -> Self {
            self.fs_size_blocks = blocks.max(16);
            self
        }

        /// Build the test environment.
        pub fn build(
            self,
        ) -> Environment<StringStdioIn, StringStdioOut, StringStdioOut, TestFilesystem> {
            let fs = EudaemonFilesystem::new_memory(
                self.fs_size_blocks * 4096,
                DeviceId::new(1),
                zero_time as fn() -> i64,
            )
            .expect("failed to create EudaemonFilesystem for test");

            Environment {
                stdin: StringStdioIn::new(&self.stdin),
                stdout: StringStdioOut::new(),
                stderr: StringStdioOut::new(),
                fs,
                env: self.env_vars,
                vars: HashMap::new(),
                args: self.args,
                cwd: Path::from(self.cwd.as_str()).into_owned(),
                exit_signaled: Arc::new(AtomicBool::new(false)),
            }
        }
    }

    /// Create a simple test environment with just arguments.
    ///
    /// This is a convenience function for the common case of needing
    /// a test environment with only command-line arguments.
    pub fn make_test_env(
        args: Vec<&str>,
    ) -> Environment<StringStdioIn, StringStdioOut, StringStdioOut, TestFilesystem> {
        TestEnvBuilder::new().args(args).build()
    }

    /// Create a test environment with arguments and stdin.
    ///
    /// This is a convenience function for commands that read from stdin.
    pub fn make_test_env_with_stdin(
        args: Vec<&str>,
        stdin: &str,
    ) -> Environment<StringStdioIn, StringStdioOut, StringStdioOut, TestFilesystem> {
        TestEnvBuilder::new().args(args).stdin(stdin).build()
    }
}

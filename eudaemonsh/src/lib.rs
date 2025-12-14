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
mod filesystem;

pub use builtins::lookup_bin;
pub use builtins::sh;
pub use filesystem::{DirEntry, FileType, Filesystem, MockFilesystem, RealFilesystem, TimeSpec};

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
                ("SHELL".to_string(), "eudaemonsh".to_string()),
                ("TMPDIR".to_string(), "/tmp".to_string()),
                ("USER".to_string(), "assistant".to_string()),
            ]),
            args: vec!["/bin/eudaemonsh".to_string()],
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

/// Test utilities for creating mock environments.
#[cfg(test)]
pub mod test_utils {
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;

    use utf8path::Path;

    use crate::{Environment, MockFilesystem, StringStderr, StringStdin, StringStdout};

    /// A builder for creating test environments with mock I/O.
    pub struct TestEnvBuilder {
        args: Vec<String>,
        stdin: String,
        env_vars: HashMap<String, String>,
        cwd: String,
        setup_root_dir: bool,
    }

    impl Default for TestEnvBuilder {
        fn default() -> Self {
            Self::new()
        }
    }

    impl TestEnvBuilder {
        /// Create a new test environment builder with default settings.
        pub fn new() -> Self {
            Self {
                args: Vec::new(),
                stdin: String::new(),
                env_vars: HashMap::new(),
                cwd: "/".to_string(),
                setup_root_dir: false,
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

        /// Automatically add the root directory to the mock filesystem.
        pub fn with_root_dir(mut self) -> Self {
            self.setup_root_dir = true;
            self
        }

        /// Build the test environment.
        pub fn build(self) -> Environment<StringStdin, StringStdout, StringStderr, MockFilesystem> {
            let fs = MockFilesystem::new();
            if self.setup_root_dir {
                fs.add_directory("/");
            }
            Environment {
                stdin: StringStdin::new(&self.stdin),
                stdout: StringStdout::new(),
                stderr: StringStderr::new(),
                fs,
                env: self.env_vars,
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
    ) -> Environment<StringStdin, StringStdout, StringStderr, MockFilesystem> {
        TestEnvBuilder::new().args(args).build()
    }

    /// Create a test environment with arguments and stdin.
    ///
    /// This is a convenience function for commands that read from stdin.
    pub fn make_test_env_with_stdin(
        args: Vec<&str>,
        stdin: &str,
    ) -> Environment<StringStdin, StringStdout, StringStderr, MockFilesystem> {
        TestEnvBuilder::new().args(args).stdin(stdin).build()
    }
}

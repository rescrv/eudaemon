use std::cell::RefCell;
use std::collections::HashMap;
use std::io::BufRead;
use std::io::Write;
use std::rc::Rc;

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

/// A trait for filesystem operations.
pub trait Filesystem {
    /// Duplicate the filesystem handle.
    fn dup(&self) -> Self;
    /// Read a file and return its contents as a string.
    fn read_to_string(&self, path: &str) -> Result<String, Error>;
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
}

/// A mock filesystem backed by a HashMap.
#[derive(Clone)]
pub struct MockFilesystem(Rc<RefCell<HashMap<String, String>>>);

impl MockFilesystem {
    /// Create a new empty MockFilesystem.
    pub fn new() -> Self {
        Self(Rc::new(RefCell::new(HashMap::new())))
    }

    /// Add a file to the mock filesystem.
    pub fn add_file(&self, path: &str, contents: &str) {
        self.0
            .borrow_mut()
            .insert(path.to_string(), contents.to_string());
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
            .cloned()
            .ok_or_else(|| Error::Io(std::io::Error::new(std::io::ErrorKind::NotFound, path)))
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
        }
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

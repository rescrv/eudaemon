use std::collections::HashMap;

use utf8path::Path;

mod builtins;

/// Errors that can occur during shell operations.
#[derive(Debug)]
pub enum Error {
    /// An error from shvar parsing.
    Shvar(shvar::Error),
}

impl From<shvar::Error> for Error {
    fn from(err: shvar::Error) -> Self {
        Self::Shvar(err)
    }
}

pub trait Stdin {
    fn dup(&self) -> Self;
}

impl Stdin for () {
    fn dup(&self) -> Self {
        *self
    }
}

impl Stdin for String {
    fn dup(&self) -> Self {
        self.clone()
    }
}

impl Stdin for std::io::Stdin {
    fn dup(&self) -> Self {
        std::io::stdin()
    }
}

pub trait Stdout {
    fn dup(&self) -> Self;
}

impl Stdout for () {
    fn dup(&self) -> Self {
        *self
    }
}

impl Stdout for String {
    fn dup(&self) -> Self {
        self.clone()
    }
}

impl Stdout for std::io::Stdout {
    fn dup(&self) -> Self {
        std::io::stdout()
    }
}

pub trait Stderr {
    fn dup(&self) -> Self;
}

impl Stderr for () {
    fn dup(&self) -> Self {
        *self
    }
}

impl Stderr for String {
    fn dup(&self) -> Self {
        self.clone()
    }
}

impl Stderr for std::io::Stderr {
    fn dup(&self) -> Self {
        std::io::stderr()
    }
}

pub struct Environment<SI, SO, SE>
where
    SI: Stdin,
    SO: Stdout,
    SE: Stderr,
{
    pub stdin: SI,
    pub stdout: SO,
    pub stderr: SE,
    pub env: HashMap<String, String>,
    pub args: Vec<String>,
    pub cwd: Path<'static>,
}

impl Default for Environment<std::io::Stdin, std::io::Stdout, std::io::Stderr> {
    fn default() -> Self {
        Self {
            stdin: std::io::stdin(),
            stdout: std::io::stdout(),
            stderr: std::io::stderr(),
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

impl<SI, SO, SE> Environment<SI, SO, SE>
where
    SI: Stdin,
    SO: Stdout,
    SE: Stderr,
{
    fn dup(&self) -> Self {
        Self {
            stdin: self.stdin.dup(),
            stdout: self.stdout.dup(),
            stderr: self.stderr.dup(),
            env: self.env.clone(),
            args: self.args.clone(),
            cwd: self.cwd.clone(),
        }
    }

    fn with_args(mut self, args: Vec<String>) -> Self {
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

pub struct Shell<SI, SO, SE>
where
    SI: Stdin,
    SO: Stdout,
    SE: Stderr,
{
    env: Environment<SI, SO, SE>,
}

impl<SI, SO, SE> Shell<SI, SO, SE>
where
    SI: Stdin,
    SO: Stdout,
    SE: Stderr,
{
    /// Creates a new shell with the given environment.
    pub fn new(env: Environment<SI, SO, SE>) -> Self {
        Self { env }
    }

    /// Runs a command and returns the result.
    pub fn run(&mut self, command: String) -> Result<ExitCode, Error> {
        let args = shvar::split(&command)?;
        if args.is_empty() {
            todo!("TODO(claude):  Return error");
        }
        let argv0 = args[0].clone();
        let env = self.env.dup().with_args(args);
        let cmd = Command::new(&argv0, env)?;
        cmd.run()
    }
}

impl Shell<(), String, String> {
    /// Takes and clears stdout and stderr, returning them as a tuple.
    pub fn take_output(&mut self) -> (String, String) {
        let stdout = std::mem::take(&mut self.env.stdout);
        let stderr = std::mem::take(&mut self.env.stderr);
        (stdout, stderr)
    }
}

struct Command<SI, SO, SE>
where
    SI: Stdin,
    SO: Stdout,
    SE: Stderr,
{
    env: Environment<SI, SO, SE>,
    bin: fn(&Environment<SI, SO, SE>) -> Result<(), Error>,
}

impl<SI, SO, SE> Command<SI, SO, SE>
where
    SI: Stdin,
    SO: Stdout,
    SE: Stderr,
{
    pub fn new(bin: &str, env: Environment<SI, SO, SE>) -> Result<Self, Error> {
        let bin = builtins::lookup_bin(bin)?;
        Ok(Self { env, bin })
    }

    pub fn env(&mut self) -> &mut Environment<SI, SO, SE> {
        &mut self.env
    }

    pub fn run(self) -> Result<ExitCode, Error> {
        Ok(ExitCode::from(-13))
    }
}

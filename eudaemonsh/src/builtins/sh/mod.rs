use std::collections::HashMap;

use shvar::ExpandOptions;
use utf8path::Path;

use crate::{
    Command, Environment, Error, ExitCode, FileAppendStdioOut, FileStdioOut, Filesystem, FsError,
    PipeReader, StdioIn, StdioOut, mkpipe, resolve_path,
};

/// Options for shell variable expansion.
///
/// Supports `$VARNAME` (bareword) and `${VARNAME}` (curly braces) syntax.
fn shell_expand_options() -> ExpandOptions {
    ExpandOptions {
        bareword: true,
        curly_braces: true,
        parens: false,
    }
}

/// The sh builtin: execute shell commands.
///
/// Usage:
///   sh -c command_string [command_name [argument...]]
///   sh script_file [argument...]
pub fn bin<SI, SO, SE, FS>(env: &Environment<SI, SO, SE, FS>) -> Result<ExitCode, Error>
where
    SI: StdioIn,
    SO: StdioOut,
    SE: StdioOut,
    FS: Filesystem + 'static,
{
    let args = &env.args;

    // Parse arguments
    // sh -c command_string [command_name [args...]]
    // sh script_file [args...]
    if args.len() < 2 {
        env.stderr
            .write_line("sh: usage: sh -c command | sh script")?;
        return Ok(ExitCode::from(2));
    }

    // Create a mutable copy for the shell session so set/unset/export can modify variables
    let mut shell_env = env.dup();

    if args[1] == "-c" {
        // sh -c command_string
        if args.len() < 3 {
            env.stderr
                .write_line("sh: -c: option requires an argument")?;
            return Ok(ExitCode::from(2));
        }
        let command_string = &args[2];
        run(command_string.clone(), &mut shell_env)
    } else {
        // sh script_file
        let script_path = &args[1];
        run_script(script_path, &mut shell_env)
    }
}

/// Run a script file, executing each line.
pub fn run_script<SI, SO, SE, FS>(
    path: &str,
    env: &mut Environment<SI, SO, SE, FS>,
) -> Result<ExitCode, Error>
where
    SI: StdioIn,
    SO: StdioOut,
    SE: StdioOut,
    FS: Filesystem + 'static,
{
    let contents = match env.fs.read_to_string(path) {
        Ok(c) => c,
        Err(FsError::Io(e)) => {
            env.stderr.write_line(&format!("sh: {}: {}", path, e))?;
            return Ok(ExitCode::from(127));
        }
    };

    run_string(&contents, env)
}

/// Run a script from a string, executing each line.
pub fn run_string<SI, SO, SE, FS>(
    script: &str,
    env: &mut Environment<SI, SO, SE, FS>,
) -> Result<ExitCode, Error>
where
    SI: StdioIn,
    SO: StdioOut,
    SE: StdioOut,
    FS: Filesystem + 'static,
{
    let mut last_exit = ExitCode::from(0);
    for line in script.lines() {
        if env.is_exit_signaled() {
            break;
        }
        let line = line.trim();
        // Skip empty lines and comments
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        last_exit = run(line.to_string(), env)?;
    }
    Ok(last_exit)
}

/// Run a single command line with support for && and || operators.
pub fn run<SI, SO, SE, FS>(
    command: String,
    env: &mut Environment<SI, SO, SE, FS>,
) -> Result<ExitCode, Error>
where
    SI: StdioIn,
    SO: StdioOut,
    SE: StdioOut,
    FS: Filesystem + 'static,
{
    // Parse prefix variable assignments one token at a time from the raw command string.
    // This ensures that quotes are preserved until we know whether a token is an assignment.
    // For example: NOPRINT=printed echo '$NOPRINT'
    // - First token "NOPRINT=printed" is an assignment, add to prefix_vars
    // - Remaining command is "echo '$NOPRINT'" which gets split and expanded normally
    let mut prefix_vars: HashMap<String, String> = HashMap::new();
    let mut remaining_command = command.as_str();

    while !remaining_command.is_empty() {
        let Some((first, remainder)) = shvar::split_once(remaining_command)? else {
            break;
        };
        if let Some((name, value)) = parse_assignment(&first) {
            prefix_vars.insert(name.to_string(), value.to_string());
            remaining_command = remainder;
        } else {
            break;
        }
    }

    // If all arguments were assignments, set them in shell environment and return
    // But if there were no assignments at all (empty command), return an error
    if remaining_command.is_empty() {
        if prefix_vars.is_empty() {
            return Err(Error::EmptyCommand);
        }
        for (name, value) in prefix_vars {
            let expanded_value =
                shvar::expand_with_options(shell_expand_options(), &(&env.vars, &env.env), &value)?;
            env.vars.insert(name, expanded_value);
        }
        return Ok(ExitCode::from(0));
    }

    // Expand variables in the remaining command, then split
    // expand_with_options respects quoting, so single-quoted strings won't have variables expanded
    let expanded_command = shvar::expand_with_options(
        shell_expand_options(),
        &(&prefix_vars, &env.vars, &env.env),
        remaining_command,
    )?;
    let args = shvar::split(&expanded_command)?;

    if args.is_empty() || args[0].is_empty() {
        return Err(Error::EmptyCommand);
    }

    // Parse into commands separated by && and ||
    let commands = parse_command_chain(&args)?;
    let result = run_command_chain(&commands, env)?;

    // Update $? with the exit code of the last command
    env.vars.insert("?".to_string(), result.code().to_string());

    Ok(result)
}

/// Handle the `set` builtin: set shell variables.
///
/// Usage: set NAME=VALUE ...
fn handle_set<SI, SO, SE, FS>(
    args: &[String],
    env: &mut Environment<SI, SO, SE, FS>,
) -> Result<ExitCode, Error>
where
    SI: StdioIn,
    SO: StdioOut,
    SE: StdioOut,
    FS: Filesystem,
{
    for arg in args.iter().skip(1) {
        if let Some((name, value)) = arg.split_once('=') {
            env.vars.insert(name.to_string(), value.to_string());
        } else {
            env.stderr
                .write_line(&format!("set: {}: not in NAME=VALUE format", arg))?;
            return Ok(ExitCode::from(1));
        }
    }
    Ok(ExitCode::from(0))
}

/// Handle the `unset` builtin: remove shell and environment variables.
///
/// Usage: unset NAME ...
fn handle_unset<SI, SO, SE, FS>(
    args: &[String],
    env: &mut Environment<SI, SO, SE, FS>,
) -> Result<ExitCode, Error>
where
    SI: StdioIn,
    SO: StdioOut,
    SE: StdioOut,
    FS: Filesystem,
{
    for name in args.iter().skip(1) {
        env.vars.remove(name);
        env.env.remove(name);
    }
    Ok(ExitCode::from(0))
}

/// Handle the `export` builtin: export shell variables to the environment.
///
/// Usage: export NAME ...
///        export NAME=VALUE ...
///
/// If NAME=VALUE is given, sets the variable and exports it.
/// If just NAME is given, moves the variable from vars to env (if it exists in vars).
fn handle_export<SI, SO, SE, FS>(
    args: &[String],
    env: &mut Environment<SI, SO, SE, FS>,
) -> Result<ExitCode, Error>
where
    SI: StdioIn,
    SO: StdioOut,
    SE: StdioOut,
    FS: Filesystem,
{
    for arg in args.iter().skip(1) {
        if let Some((name, value)) = arg.split_once('=') {
            // export NAME=VALUE: set and export
            env.vars.remove(name);
            env.env.insert(name.to_string(), value.to_string());
        } else {
            // export NAME: move from vars to env if present
            if let Some(value) = env.vars.remove(arg) {
                env.env.insert(arg.to_string(), value);
            }
            // If not in vars, check if already in env (no-op) or doesn't exist (no-op)
        }
    }
    Ok(ExitCode::from(0))
}

/// Check if a string is a valid shell variable name.
///
/// A valid variable name starts with a letter or underscore, and contains only
/// letters, digits, and underscores.
fn is_valid_var_name(name: &str) -> bool {
    if name.is_empty() {
        return false;
    }
    let mut chars = name.chars();
    let first = chars.next().unwrap();
    if !first.is_ascii_alphabetic() && first != '_' {
        return false;
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Check if a string is a variable assignment (NAME=value).
///
/// Returns Some((name, value)) if it's a valid assignment, None otherwise.
fn parse_assignment(s: &str) -> Option<(&str, &str)> {
    if let Some(eq_pos) = s.find('=') {
        let name = &s[..eq_pos];
        let value = &s[eq_pos + 1..];
        if is_valid_var_name(name) {
            return Some((name, value));
        }
    }
    None
}

/// Handle the `cd` builtin: change the current working directory.
///
/// Usage: cd [directory]
///
/// If directory is omitted, changes to the home directory (from $HOME).
/// If $HOME is not set and no directory is given, returns an error.
fn handle_cd<SI, SO, SE, FS>(
    args: &[String],
    env: &mut Environment<SI, SO, SE, FS>,
) -> Result<ExitCode, Error>
where
    SI: StdioIn,
    SO: StdioOut,
    SE: StdioOut,
    FS: Filesystem,
{
    let target = if args.len() > 1 {
        args[1].clone()
    } else {
        // No argument: go to $HOME
        if let Some(home) = env.env.get("HOME").or_else(|| env.vars.get("HOME")) {
            home.clone()
        } else {
            env.stderr.write_line("cd: HOME not set")?;
            return Ok(ExitCode::from(1));
        }
    };

    // Resolve the path relative to current directory
    let new_path = resolve_path(env.cwd.as_str(), &target);

    // Check if the target exists
    if !env.fs.exists(&new_path) {
        env.stderr
            .write_line(&format!("cd: {}: No such file or directory", target))?;
        return Ok(ExitCode::from(1));
    }

    // Check if the target is a directory
    if env.fs.is_dir(&new_path) {
        env.cwd = Path::from(new_path.as_str()).into_owned();
        Ok(ExitCode::from(0))
    } else {
        env.stderr
            .write_line(&format!("cd: {}: Not a directory", target))?;
        Ok(ExitCode::from(1))
    }
}

/// Operator between pipelines in a chain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChainOp {
    /// Execute next pipeline only if previous succeeded (&&).
    And,
    /// Execute next pipeline only if previous failed (||).
    Or,
    /// Execute next pipeline unconditionally (;).
    Seq,
}

/// Type of output redirection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RedirectMode {
    /// Truncate and write (>).
    Truncate,
    /// Append (>>).
    Append,
}

/// Output redirection specification.
#[derive(Debug)]
struct OutputRedirect {
    /// The file to redirect to.
    target: String,
    /// Whether to truncate or append.
    mode: RedirectMode,
}

/// Stderr redirection specification.
#[derive(Debug)]
enum StderrRedirect {
    /// Redirect stderr to a file (2> or 2>>).
    File(OutputRedirect),
    /// Redirect stderr to stdout (2>&1).
    ToStdout,
}

/// A pipeline is a sequence of commands connected by pipes.
#[derive(Debug)]
struct Pipeline {
    /// Commands in the pipeline, executed left-to-right with stdout->stdin connections.
    commands: Vec<Vec<String>>,
    /// Output redirection target, if any (from `>` or `>>`).
    redirect_stdout: Option<OutputRedirect>,
    /// Stderr redirection, if any (from `2>`, `2>>`, or `2>&1`).
    redirect_stderr: Option<StderrRedirect>,
}

/// A pipeline in a chain with its following operator.
#[derive(Debug)]
struct ChainedPipeline {
    /// The pipeline to execute.
    pipeline: Pipeline,
    /// Operator to next pipeline, if any.
    next_op: Option<ChainOp>,
}

/// Parse a list of arguments into a chain of pipelines separated by && and ||.
///
/// Each pipeline consists of commands separated by |.
fn parse_command_chain(args: &[String]) -> Result<Vec<ChainedPipeline>, Error> {
    let mut pipelines = Vec::new();
    let mut current_pipeline_args = Vec::new();

    for arg in args {
        if arg == "&&" {
            if !current_pipeline_args.is_empty() {
                pipelines.push(ChainedPipeline {
                    pipeline: parse_pipeline(&current_pipeline_args)?,
                    next_op: Some(ChainOp::And),
                });
                current_pipeline_args = Vec::new();
            }
        } else if arg == "||" {
            if !current_pipeline_args.is_empty() {
                pipelines.push(ChainedPipeline {
                    pipeline: parse_pipeline(&current_pipeline_args)?,
                    next_op: Some(ChainOp::Or),
                });
                current_pipeline_args = Vec::new();
            }
        } else if arg == ";" {
            if !current_pipeline_args.is_empty() {
                pipelines.push(ChainedPipeline {
                    pipeline: parse_pipeline(&current_pipeline_args)?,
                    next_op: Some(ChainOp::Seq),
                });
                current_pipeline_args = Vec::new();
            }
        } else {
            current_pipeline_args.push(arg.clone());
        }
    }

    // Add final pipeline
    if !current_pipeline_args.is_empty() {
        pipelines.push(ChainedPipeline {
            pipeline: parse_pipeline(&current_pipeline_args)?,
            next_op: None,
        });
    }

    Ok(pipelines)
}

/// Parse arguments into a pipeline (commands separated by |).
///
/// Also extracts output redirection (`>`) from the last command.
fn parse_pipeline(args: &[String]) -> Result<Pipeline, Error> {
    let mut commands = Vec::new();
    let mut current_args = Vec::new();

    for arg in args {
        if arg == "|" {
            if !current_args.is_empty() {
                commands.push(std::mem::take(&mut current_args));
            }
        } else {
            current_args.push(arg.clone());
        }
    }

    // Add final command
    if !current_args.is_empty() {
        commands.push(current_args);
    }

    // Extract output redirection from the last command
    let redirect_stdout = if let Some(last_cmd) = commands.last_mut() {
        extract_output_redirection(last_cmd)?
    } else {
        None
    };

    // Extract stderr redirection from the last command
    let redirect_stderr = if let Some(last_cmd) = commands.last_mut() {
        extract_stderr_redirection(last_cmd)?
    } else {
        None
    };

    Ok(Pipeline {
        commands,
        redirect_stdout,
        redirect_stderr,
    })
}

/// Extract output redirection from command arguments.
///
/// Handles the following forms:
/// - `> file` or `>> file` (operator and file as separate tokens)
/// - `>file` or `>>file` (operator attached to filename)
/// - `arg>` or `arg>>` (operator attached to previous arg - treated as part of arg, not redirection)
///
/// Returns the redirection specification if found, and removes the relevant tokens from args.
fn extract_output_redirection(args: &mut Vec<String>) -> Result<Option<OutputRedirect>, Error> {
    // First pass: look for standalone `>` or `>>` operators
    for i in 0..args.len() {
        if args[i] == ">>" {
            if i + 1 < args.len() {
                let target = args.remove(i + 1);
                args.remove(i);
                return Ok(Some(OutputRedirect {
                    target,
                    mode: RedirectMode::Append,
                }));
            } else {
                return Err(Error::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "syntax error: no file after >>",
                )));
            }
        } else if args[i] == ">" {
            if i + 1 < args.len() {
                let target = args.remove(i + 1);
                args.remove(i);
                return Ok(Some(OutputRedirect {
                    target,
                    mode: RedirectMode::Truncate,
                }));
            } else {
                return Err(Error::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "syntax error: no file after >",
                )));
            }
        }
    }

    // Second pass: look for `>>file` or `>file` (operator attached to filename)
    for i in 0..args.len() {
        if let Some(target) = args[i].strip_prefix(">>") {
            if target.is_empty() {
                // This is just `>>` with nothing after - already handled above
                continue;
            }
            let target = target.to_string();
            args.remove(i);
            return Ok(Some(OutputRedirect {
                target,
                mode: RedirectMode::Append,
            }));
        } else if let Some(target) = args[i].strip_prefix('>') {
            if target.is_empty() {
                // This is just `>` with nothing after - already handled above
                continue;
            }
            let target = target.to_string();
            args.remove(i);
            return Ok(Some(OutputRedirect {
                target,
                mode: RedirectMode::Truncate,
            }));
        }
    }

    Ok(None)
}

/// Extract stderr redirection from command arguments.
///
/// Handles the following forms:
/// - `2>&1` (redirect stderr to stdout)
/// - `2>file` or `2>>file` (redirect stderr to a file)
/// - `2> file` or `2>> file` (operator and file as separate tokens)
///
/// Returns the redirection specification if found, and removes the relevant tokens from args.
fn extract_stderr_redirection(args: &mut Vec<String>) -> Result<Option<StderrRedirect>, Error> {
    // Look for 2>&1 (redirect stderr to stdout)
    for i in 0..args.len() {
        if args[i] == "2>&1" {
            args.remove(i);
            return Ok(Some(StderrRedirect::ToStdout));
        }
    }

    // Look for standalone `2>` or `2>>` operators
    for i in 0..args.len() {
        if args[i] == "2>>" {
            if i + 1 < args.len() {
                let target = args.remove(i + 1);
                args.remove(i);
                return Ok(Some(StderrRedirect::File(OutputRedirect {
                    target,
                    mode: RedirectMode::Append,
                })));
            } else {
                return Err(Error::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "syntax error: no file after 2>>",
                )));
            }
        } else if args[i] == "2>" {
            if i + 1 < args.len() {
                let target = args.remove(i + 1);
                args.remove(i);
                return Ok(Some(StderrRedirect::File(OutputRedirect {
                    target,
                    mode: RedirectMode::Truncate,
                })));
            } else {
                return Err(Error::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "syntax error: no file after 2>",
                )));
            }
        }
    }

    // Look for `2>>file`, `2>file`, or `2>&1` attached to a token
    for i in 0..args.len() {
        if args[i].starts_with("2>>") {
            let target = args[i][3..].to_string();
            if target.is_empty() {
                continue;
            }
            args.remove(i);
            return Ok(Some(StderrRedirect::File(OutputRedirect {
                target,
                mode: RedirectMode::Append,
            })));
        } else if args[i].starts_with("2>&1") {
            // Handle 2>&1 as part of a token (shouldn't normally happen, but be safe)
            args.remove(i);
            return Ok(Some(StderrRedirect::ToStdout));
        } else if args[i].starts_with("2>") {
            let target = args[i][2..].to_string();
            if target.is_empty() {
                continue;
            }
            args.remove(i);
            return Ok(Some(StderrRedirect::File(OutputRedirect {
                target,
                mode: RedirectMode::Truncate,
            })));
        }
    }

    Ok(None)
}

/// Run a single command with stdout and stderr redirections.
///
/// This handles all combinations of:
/// - No redirection
/// - Stdout to file (truncate or append)
/// - Stderr to file (truncate or append)
/// - Stderr to stdout (2>&1)
///
/// The `prefix_assignments` parameter contains variable assignments that should be set
/// in the command's environment (but not persist in the shell). This implements the
/// POSIX behavior where `VAR=value cmd` sets VAR only for cmd's execution.
fn run_single_command<SI, SO, SE, FS>(
    argv0: &str,
    args: &[String],
    env: &mut Environment<SI, SO, SE, FS>,
    redirect_stdout: Option<&OutputRedirect>,
    redirect_stderr: Option<&StderrRedirect>,
    prefix_assignments: &[(String, String)],
) -> Result<ExitCode, Error>
where
    SI: StdioIn,
    SO: StdioOut,
    SE: StdioOut,
    FS: Filesystem + 'static,
{
    // Build vars map with prefix assignments applied
    let vars_with_assignments = || {
        let mut vars = env.vars.clone();
        for (name, value) in prefix_assignments {
            vars.insert(name.clone(), value.clone());
        }
        vars
    };

    // Handle the 4 major cases based on redirections
    match (redirect_stdout, redirect_stderr) {
        // No redirections
        (None, None) => {
            let cmd_env = env
                .dup()
                .with_args(args.to_vec())
                .with_vars(vars_with_assignments());
            let command = Command::new(argv0, cmd_env)?;
            command.run()
        }

        // Only stdout redirection
        (Some(stdout_redir), None) => {
            let path = resolve_path(env.cwd.as_str(), &stdout_redir.target);
            match stdout_redir.mode {
                RedirectMode::Truncate => {
                    let file_stdout = FileStdioOut::new(env.fs.dup(), path);
                    let cmd_env = Environment {
                        stdin: env.stdin.dup(),
                        stdout: file_stdout,
                        stderr: env.stderr.dup(),
                        fs: env.fs.dup(),
                        env: env.env.clone(),
                        vars: vars_with_assignments(),
                        args: args.to_vec(),
                        cwd: env.cwd.clone(),
                        exit_signaled: std::sync::Arc::clone(&env.exit_signaled),
                    };
                    let command = Command::new(argv0, cmd_env)?;
                    command.run()
                }
                RedirectMode::Append => {
                    let file_stdout = FileAppendStdioOut::new(env.fs.dup(), path);
                    let cmd_env = Environment {
                        stdin: env.stdin.dup(),
                        stdout: file_stdout,
                        stderr: env.stderr.dup(),
                        fs: env.fs.dup(),
                        env: env.env.clone(),
                        vars: vars_with_assignments(),
                        args: args.to_vec(),
                        cwd: env.cwd.clone(),
                        exit_signaled: std::sync::Arc::clone(&env.exit_signaled),
                    };
                    let command = Command::new(argv0, cmd_env)?;
                    command.run()
                }
            }
        }

        // Only stderr redirection (to file)
        (None, Some(StderrRedirect::File(stderr_redir))) => {
            let path = resolve_path(env.cwd.as_str(), &stderr_redir.target);
            match stderr_redir.mode {
                RedirectMode::Truncate => {
                    let file_stderr = FileStdioOut::new(env.fs.dup(), path);
                    let cmd_env = Environment {
                        stdin: env.stdin.dup(),
                        stdout: env.stdout.dup(),
                        stderr: file_stderr,
                        fs: env.fs.dup(),
                        env: env.env.clone(),
                        vars: vars_with_assignments(),
                        args: args.to_vec(),
                        cwd: env.cwd.clone(),
                        exit_signaled: std::sync::Arc::clone(&env.exit_signaled),
                    };
                    let command = Command::new(argv0, cmd_env)?;
                    command.run()
                }
                RedirectMode::Append => {
                    let file_stderr = FileAppendStdioOut::new(env.fs.dup(), path);
                    let cmd_env = Environment {
                        stdin: env.stdin.dup(),
                        stdout: env.stdout.dup(),
                        stderr: file_stderr,
                        fs: env.fs.dup(),
                        env: env.env.clone(),
                        vars: vars_with_assignments(),
                        args: args.to_vec(),
                        cwd: env.cwd.clone(),
                        exit_signaled: std::sync::Arc::clone(&env.exit_signaled),
                    };
                    let command = Command::new(argv0, cmd_env)?;
                    command.run()
                }
            }
        }

        // Stderr to stdout (2>&1) without stdout redirection
        // This effectively means stderr goes to the same place as stdout
        (None, Some(StderrRedirect::ToStdout)) => {
            // Use a pipe to merge stderr into stdout
            let (reader, writer) = mkpipe();
            let cmd_env = Environment {
                stdin: env.stdin.dup(),
                stdout: writer.clone(),
                stderr: writer,
                fs: env.fs.dup(),
                env: env.env.clone(),
                vars: vars_with_assignments(),
                args: args.to_vec(),
                cwd: env.cwd.clone(),
                exit_signaled: std::sync::Arc::clone(&env.exit_signaled),
            };
            let command = Command::new(argv0, cmd_env)?;
            let result = command.run()?;
            // Copy pipe output to stdout
            while let Some(line) = reader.read_line()? {
                env.stdout.write_line(&line)?;
            }
            Ok(result)
        }

        // Both stdout and stderr redirected to files
        (Some(stdout_redir), Some(StderrRedirect::File(stderr_redir))) => {
            let stdout_path = resolve_path(env.cwd.as_str(), &stdout_redir.target);
            let stderr_path = resolve_path(env.cwd.as_str(), &stderr_redir.target);
            match (stdout_redir.mode, stderr_redir.mode) {
                (RedirectMode::Truncate, RedirectMode::Truncate) => {
                    let file_stdout = FileStdioOut::new(env.fs.dup(), stdout_path);
                    let file_stderr = FileStdioOut::new(env.fs.dup(), stderr_path);
                    let cmd_env = Environment {
                        stdin: env.stdin.dup(),
                        stdout: file_stdout,
                        stderr: file_stderr,
                        fs: env.fs.dup(),
                        env: env.env.clone(),
                        vars: vars_with_assignments(),
                        args: args.to_vec(),
                        cwd: env.cwd.clone(),
                        exit_signaled: std::sync::Arc::clone(&env.exit_signaled),
                    };
                    let command = Command::new(argv0, cmd_env)?;
                    command.run()
                }
                (RedirectMode::Truncate, RedirectMode::Append) => {
                    let file_stdout = FileStdioOut::new(env.fs.dup(), stdout_path);
                    let file_stderr = FileAppendStdioOut::new(env.fs.dup(), stderr_path);
                    let cmd_env = Environment {
                        stdin: env.stdin.dup(),
                        stdout: file_stdout,
                        stderr: file_stderr,
                        fs: env.fs.dup(),
                        env: env.env.clone(),
                        vars: vars_with_assignments(),
                        args: args.to_vec(),
                        cwd: env.cwd.clone(),
                        exit_signaled: std::sync::Arc::clone(&env.exit_signaled),
                    };
                    let command = Command::new(argv0, cmd_env)?;
                    command.run()
                }
                (RedirectMode::Append, RedirectMode::Truncate) => {
                    let file_stdout = FileAppendStdioOut::new(env.fs.dup(), stdout_path);
                    let file_stderr = FileStdioOut::new(env.fs.dup(), stderr_path);
                    let cmd_env = Environment {
                        stdin: env.stdin.dup(),
                        stdout: file_stdout,
                        stderr: file_stderr,
                        fs: env.fs.dup(),
                        env: env.env.clone(),
                        vars: vars_with_assignments(),
                        args: args.to_vec(),
                        cwd: env.cwd.clone(),
                        exit_signaled: std::sync::Arc::clone(&env.exit_signaled),
                    };
                    let command = Command::new(argv0, cmd_env)?;
                    command.run()
                }
                (RedirectMode::Append, RedirectMode::Append) => {
                    let file_stdout = FileAppendStdioOut::new(env.fs.dup(), stdout_path);
                    let file_stderr = FileAppendStdioOut::new(env.fs.dup(), stderr_path);
                    let cmd_env = Environment {
                        stdin: env.stdin.dup(),
                        stdout: file_stdout,
                        stderr: file_stderr,
                        fs: env.fs.dup(),
                        env: env.env.clone(),
                        vars: vars_with_assignments(),
                        args: args.to_vec(),
                        cwd: env.cwd.clone(),
                        exit_signaled: std::sync::Arc::clone(&env.exit_signaled),
                    };
                    let command = Command::new(argv0, cmd_env)?;
                    command.run()
                }
            }
        }

        // Stdout to file, stderr to stdout (2>&1)
        // This means both stdout and stderr go to the same file
        // Use .dup() to share the same buffer so both streams write to the same place
        (Some(stdout_redir), Some(StderrRedirect::ToStdout)) => {
            let path = resolve_path(env.cwd.as_str(), &stdout_redir.target);
            match stdout_redir.mode {
                RedirectMode::Truncate => {
                    let file_stdout = FileStdioOut::new(env.fs.dup(), path);
                    let file_stderr = file_stdout.dup();
                    let cmd_env = Environment {
                        stdin: env.stdin.dup(),
                        stdout: file_stdout,
                        stderr: file_stderr,
                        fs: env.fs.dup(),
                        env: env.env.clone(),
                        vars: vars_with_assignments(),
                        args: args.to_vec(),
                        cwd: env.cwd.clone(),
                        exit_signaled: std::sync::Arc::clone(&env.exit_signaled),
                    };
                    let command = Command::new(argv0, cmd_env)?;
                    command.run()
                }
                RedirectMode::Append => {
                    let file_stdout = FileAppendStdioOut::new(env.fs.dup(), path);
                    let file_stderr = file_stdout.dup();
                    let cmd_env = Environment {
                        stdin: env.stdin.dup(),
                        stdout: file_stdout,
                        stderr: file_stderr,
                        fs: env.fs.dup(),
                        env: env.env.clone(),
                        vars: vars_with_assignments(),
                        args: args.to_vec(),
                        cwd: env.cwd.clone(),
                        exit_signaled: std::sync::Arc::clone(&env.exit_signaled),
                    };
                    let command = Command::new(argv0, cmd_env)?;
                    command.run()
                }
            }
        }
    }
}

/// Run a chain of pipelines respecting && and || operators.
fn run_command_chain<SI, SO, SE, FS>(
    pipelines: &[ChainedPipeline],
    env: &mut Environment<SI, SO, SE, FS>,
) -> Result<ExitCode, Error>
where
    SI: StdioIn,
    SO: StdioOut,
    SE: StdioOut,
    FS: Filesystem + 'static,
{
    if pipelines.is_empty() {
        return Ok(ExitCode::from(0));
    }

    // Run first pipeline unconditionally
    let first = &pipelines[0];
    let mut last_exit = run_pipeline(&first.pipeline, env)?;

    // Process remaining pipelines based on operators
    for i in 1..pipelines.len() {
        let prev_op = pipelines[i - 1].next_op;
        let pipeline = &pipelines[i];

        // Decide whether to run this pipeline based on previous exit code and operator
        let should_run = match prev_op {
            Some(ChainOp::And) => last_exit.code() == 0,
            Some(ChainOp::Or) => last_exit.code() != 0,
            Some(ChainOp::Seq) => true,
            None => true,
        };

        if should_run {
            last_exit = run_pipeline(&pipeline.pipeline, env)?;
        }
    }

    Ok(last_exit)
}

/// Run a pipeline of commands connected by pipes.
fn run_pipeline<SI, SO, SE, FS>(
    pipeline: &Pipeline,
    env: &mut Environment<SI, SO, SE, FS>,
) -> Result<ExitCode, Error>
where
    SI: StdioIn,
    SO: StdioOut,
    SE: StdioOut,
    FS: Filesystem + 'static,
{
    if pipeline.commands.is_empty() {
        return Ok(ExitCode::from(0));
    }

    // Single command - no pipes needed
    if pipeline.commands.len() == 1 {
        let args = &pipeline.commands[0];
        if args.is_empty() {
            return Ok(ExitCode::from(0));
        }

        // Check for leading variable assignments (VAR=value syntax)
        // Collect them until we hit a non-assignment
        let mut prefix_assignments: Vec<(String, String)> = Vec::new();
        for arg in args.iter() {
            if let Some((name, value)) = parse_assignment(arg) {
                prefix_assignments.push((name.to_string(), value.to_string()));
            } else {
                break;
            }
        }
        let assignment_count = prefix_assignments.len();

        // If all arguments are assignments (no command), set them in the shell environment
        if assignment_count == args.len() {
            for (name, value) in prefix_assignments {
                env.vars.insert(name, value);
            }
            return Ok(ExitCode::from(0));
        }

        // The actual command starts after the assignments
        let cmd_args: Vec<String> = args[assignment_count..].to_vec();
        let argv0 = cmd_args[0].clone();

        // Handle shell builtins that modify state (these can't go in a pipeline with other commands)
        // Note: For builtins, we apply the prefix assignments to the shell environment
        // since builtins run in the same process
        if argv0 == "cd" {
            for (name, value) in &prefix_assignments {
                env.vars.insert(name.clone(), value.clone());
            }
            return handle_cd(&cmd_args, env);
        }
        if argv0 == "set" {
            for (name, value) in &prefix_assignments {
                env.vars.insert(name.clone(), value.clone());
            }
            return handle_set(&cmd_args, env);
        }
        if argv0 == "unset" {
            for (name, value) in &prefix_assignments {
                env.vars.insert(name.clone(), value.clone());
            }
            return handle_unset(&cmd_args, env);
        }
        if argv0 == "export" {
            for (name, value) in &prefix_assignments {
                env.vars.insert(name.clone(), value.clone());
            }
            return handle_export(&cmd_args, env);
        }

        // Run single command with appropriate redirections
        return run_single_command(
            &argv0,
            &cmd_args,
            env,
            pipeline.redirect_stdout.as_ref(),
            pipeline.redirect_stderr.as_ref(),
            &prefix_assignments,
        );
    }

    // Multiple commands - connect them with pipes
    // We run each command sequentially, passing output through pipes
    let mut last_exit = ExitCode::from(0);
    let mut current_reader: Option<PipeReader> = None;

    for (i, args) in pipeline.commands.iter().enumerate() {
        if args.is_empty() {
            continue;
        }

        let is_last = i == pipeline.commands.len() - 1;
        let argv0 = args[0].clone();

        // Create pipe for output (unless this is the last command)
        let (next_reader, writer) = if is_last {
            (None, None)
        } else {
            let (r, w) = mkpipe();
            (Some(r), Some(w))
        };

        // Build environment with appropriate stdin/stdout
        if let Some(reader) = current_reader.take() {
            if let Some(writer) = writer {
                // Middle command: read from pipe, write to pipe
                let cmd_env = Environment {
                    stdin: reader,
                    stdout: writer,
                    stderr: env.stderr.dup(),
                    fs: env.fs.dup(),
                    env: env.env.clone(),
                    vars: env.vars.clone(),
                    args: args.clone(),
                    cwd: env.cwd.clone(),
                    exit_signaled: std::sync::Arc::clone(&env.exit_signaled),
                };
                let command = Command::new(&argv0, cmd_env)?;
                last_exit = command.run()?;
            } else {
                // Last command: read from pipe, write to stdout (or file)
                if let Some(ref redirect) = pipeline.redirect_stdout {
                    let path = resolve_path(env.cwd.as_str(), &redirect.target);
                    last_exit = match redirect.mode {
                        RedirectMode::Truncate => {
                            let file_stdout = FileStdioOut::new(env.fs.dup(), path);
                            let cmd_env = Environment {
                                stdin: reader,
                                stdout: file_stdout,
                                stderr: env.stderr.dup(),
                                fs: env.fs.dup(),
                                env: env.env.clone(),
                                vars: env.vars.clone(),
                                args: args.clone(),
                                cwd: env.cwd.clone(),
                                exit_signaled: std::sync::Arc::clone(&env.exit_signaled),
                            };
                            let command = Command::new(&argv0, cmd_env)?;
                            command.run()?
                        }
                        RedirectMode::Append => {
                            let file_stdout = FileAppendStdioOut::new(env.fs.dup(), path);
                            let cmd_env = Environment {
                                stdin: reader,
                                stdout: file_stdout,
                                stderr: env.stderr.dup(),
                                fs: env.fs.dup(),
                                env: env.env.clone(),
                                vars: env.vars.clone(),
                                args: args.clone(),
                                cwd: env.cwd.clone(),
                                exit_signaled: std::sync::Arc::clone(&env.exit_signaled),
                            };
                            let command = Command::new(&argv0, cmd_env)?;
                            command.run()?
                        }
                    };
                } else {
                    let cmd_env = Environment {
                        stdin: reader,
                        stdout: env.stdout.dup(),
                        stderr: env.stderr.dup(),
                        fs: env.fs.dup(),
                        env: env.env.clone(),
                        vars: env.vars.clone(),
                        args: args.clone(),
                        cwd: env.cwd.clone(),
                        exit_signaled: std::sync::Arc::clone(&env.exit_signaled),
                    };
                    let command = Command::new(&argv0, cmd_env)?;
                    last_exit = command.run()?;
                }
            }
        } else if let Some(writer) = writer {
            // First command: read from original stdin, write to pipe
            let cmd_env = Environment {
                stdin: env.stdin.dup(),
                stdout: writer,
                stderr: env.stderr.dup(),
                fs: env.fs.dup(),
                env: env.env.clone(),
                vars: env.vars.clone(),
                args: args.clone(),
                cwd: env.cwd.clone(),
                exit_signaled: std::sync::Arc::clone(&env.exit_signaled),
            };
            let command = Command::new(&argv0, cmd_env)?;
            last_exit = command.run()?;
        }

        current_reader = next_reader;
    }

    Ok(last_exit)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TestFilesystemExt;
    use crate::test_utils::make_test_env;

    #[test]
    fn no_args_shows_usage() {
        let env = make_test_env(vec!["sh"]);
        let result = bin(&env).unwrap();
        assert_eq!(2, result.code());
        assert!(env.stderr.into_string().contains("usage"));
    }

    #[test]
    fn c_flag_requires_command() {
        let env = make_test_env(vec!["sh", "-c"]);
        let result = bin(&env).unwrap();
        assert_eq!(2, result.code());
        assert!(env.stderr.into_string().contains("requires an argument"));
    }

    #[test]
    fn c_flag_runs_cat() {
        let env = make_test_env(vec!["sh", "-c", "cat file.txt"]);
        env.fs.add_file("file.txt", "hello world\n");
        let result = bin(&env).unwrap();
        // Print stdout/stderr for debugging
        println!("stdout: {:?}", env.stdout.into_string());
        println!("stderr: {:?}", env.stderr.into_string());
        assert_eq!(0, result.code());
        assert_eq!("hello world\n", env.stdout.into_string());
    }

    #[test]
    fn script_file_not_found() {
        let env = make_test_env(vec!["sh", "nonexistent.sh"]);
        let result = bin(&env).unwrap();
        assert_eq!(127, result.code());
        assert!(env.stderr.into_string().contains("nonexistent.sh"));
    }

    #[test]
    fn script_file_runs() {
        let env = make_test_env(vec!["sh", "test.sh"]);
        env.fs.add_file("test.sh", "cat a.txt\ncat b.txt");
        env.fs.add_file("a.txt", "hello\n");
        env.fs.add_file("b.txt", "world\n");
        let result = bin(&env).unwrap();
        // Print stdout/stderr for debugging
        println!("stdout: {:?}", env.stdout.into_string());
        println!("stderr: {:?}", env.stderr.into_string());
        assert_eq!(0, result.code());
        assert_eq!("hello\nworld\n", env.stdout.into_string());
    }

    #[test]
    fn script_file_skips_comments_and_blanks() {
        let env = make_test_env(vec!["sh", "test.sh"]);
        env.fs.add_file(
            "test.sh",
            "# comment\n\ncat a.txt\n  # indented comment\n\ncat b.txt\n",
        );
        env.fs.add_file("a.txt", "hello\n");
        env.fs.add_file("b.txt", "world\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("hello\nworld\n", env.stdout.into_string());
    }

    #[test]
    fn run_cat() {
        let mut env = make_test_env(vec!["unused"]);
        env.fs.add_file("test.txt", "hello\n");
        let result = run("cat test.txt".to_string(), &mut env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("hello\n", env.stdout.into_string());
    }

    #[test]
    fn run_empty_command() {
        let mut env = make_test_env(vec!["unused"]);
        let result = run("".to_string(), &mut env);
        println!("result: {:?}", result);
        assert!(matches!(result, Err(Error::EmptyCommand)));
    }

    #[test]
    fn run_unknown_binary() {
        let mut env = make_test_env(vec!["unused"]);
        let result = run("nonexistent".to_string(), &mut env);
        assert!(matches!(result, Err(Error::UnknownBinary(_))));
    }

    // ========================================================================
    // run_string tests
    // ========================================================================

    #[test]
    fn run_string_empty_script() {
        let mut env = make_test_env(vec!["unused"]);
        let result = run_string("", &mut env).unwrap();
        assert_eq!(0, result.code());
    }

    #[test]
    fn run_string_comment_only() {
        let mut env = make_test_env(vec!["unused"]);
        let result = run_string("# just a comment", &mut env).unwrap();
        assert_eq!(0, result.code());
    }

    #[test]
    fn run_string_shebang_only() {
        let mut env = make_test_env(vec!["unused"]);
        let result = run_string("#!/bin/sh", &mut env).unwrap();
        assert_eq!(0, result.code());
    }

    #[test]
    fn run_string_single_command() {
        let mut env = make_test_env(vec!["unused"]);
        env.fs.add_file("test.txt", "hello\n");
        let result = run_string("cat test.txt", &mut env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("hello\n", env.stdout.into_string());
    }

    #[test]
    fn run_string_multiple_commands() {
        let mut env = make_test_env(vec!["unused"]);
        env.fs.add_file("a.txt", "aaa\n");
        env.fs.add_file("b.txt", "bbb\n");
        let result = run_string("cat a.txt\ncat b.txt", &mut env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("aaa\nbbb\n", env.stdout.into_string());
    }

    #[test]
    fn run_string_skips_blank_lines() {
        let mut env = make_test_env(vec!["unused"]);
        env.fs.add_file("a.txt", "aaa\n");
        let result = run_string("\n\ncat a.txt\n\n", &mut env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("aaa\n", env.stdout.into_string());
    }

    #[test]
    fn run_string_skips_comments() {
        let mut env = make_test_env(vec!["unused"]);
        env.fs.add_file("a.txt", "aaa\n");
        let result = run_string("# comment\ncat a.txt\n# another comment", &mut env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("aaa\n", env.stdout.into_string());
    }

    #[test]
    fn run_string_with_shebang() {
        let mut env = make_test_env(vec!["unused"]);
        env.fs.add_file("a.txt", "aaa\n");
        let result = run_string("#!/bin/sh\ncat a.txt", &mut env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("aaa\n", env.stdout.into_string());
    }

    #[test]
    fn run_string_returns_last_exit_code() {
        let mut env = make_test_env(vec!["unused"]);
        env.fs.add_file("a.txt", "aaa\n");
        let result = run_string("cat a.txt\ncat nonexistent.txt", &mut env).unwrap();
        assert_eq!(1, result.code());
    }

    // ========================================================================
    // exit early termination tests
    // ========================================================================

    #[test]
    fn exit_terminates_script() {
        let mut env = make_test_env(vec!["unused"]);
        let result = run_string("exit 0\necho should_not_appear", &mut env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("", env.stdout.into_string());
    }

    #[test]
    fn exit_with_code_terminates_script() {
        let mut env = make_test_env(vec!["unused"]);
        let result = run_string("exit 42\necho should_not_appear", &mut env).unwrap();
        assert_eq!(42, result.code());
        assert_eq!("", env.stdout.into_string());
    }

    #[test]
    fn exit_terminates_after_other_commands() {
        let mut env = make_test_env(vec!["unused"]);
        let result = run_string("echo before\nexit 5\necho after", &mut env).unwrap();
        assert_eq!(5, result.code());
        assert_eq!("before\n", env.stdout.into_string());
    }

    #[test]
    fn exit_in_middle_of_script() {
        let mut env = make_test_env(vec!["unused"]);
        env.fs.add_file("a.txt", "aaa\n");
        env.fs.add_file("b.txt", "bbb\n");
        let result = run_string("cat a.txt\nexit 3\ncat b.txt", &mut env).unwrap();
        assert_eq!(3, result.code());
        assert_eq!("aaa\n", env.stdout.into_string());
    }

    #[test]
    fn true_script_exits_zero() {
        let mut env = make_test_env(vec!["unused"]);
        let result = run_string("#!/bin/sh\nexit 0", &mut env).unwrap();
        assert_eq!(0, result.code());
    }

    #[test]
    fn false_script_exits_one() {
        let mut env = make_test_env(vec!["unused"]);
        let result = run_string("#!/bin/sh\nexit 1", &mut env).unwrap();
        assert_eq!(1, result.code());
    }

    #[test]
    fn multiple_exits_uses_first() {
        let mut env = make_test_env(vec!["unused"]);
        let result = run_string("exit 7\nexit 8\nexit 9", &mut env).unwrap();
        assert_eq!(7, result.code());
    }

    #[test]
    fn exit_signal_persists() {
        let mut env = make_test_env(vec!["unused"]);
        assert!(!env.is_exit_signaled());
        let _ = run_string("exit 0", &mut env).unwrap();
        assert!(env.is_exit_signaled());
    }

    // ========================================================================
    // command chaining tests (&&, ||)
    // ========================================================================

    #[test]
    fn and_chain_both_succeed() {
        let mut env = make_test_env(vec!["unused"]);
        let result = run("true && echo success".to_string(), &mut env).unwrap();
        let stdout = env.stdout.into_string();
        let stderr = env.stderr.into_string();
        println!("stdout: {:?}", stdout);
        println!("stderr: {:?}", stderr);
        assert_eq!(0, result.code());
        assert_eq!("success\n", stdout);
    }

    #[test]
    fn and_chain_first_fails() {
        let mut env = make_test_env(vec!["unused"]);
        let result = run("false && echo should_not_appear".to_string(), &mut env).unwrap();
        let stdout = env.stdout.into_string();
        let stderr = env.stderr.into_string();
        println!("stdout: {:?}", stdout);
        println!("stderr: {:?}", stderr);
        assert_eq!(1, result.code());
        assert_eq!("", stdout);
    }

    #[test]
    fn or_chain_first_succeeds() {
        let mut env = make_test_env(vec!["unused"]);
        let result = run("true || echo should_not_appear".to_string(), &mut env).unwrap();
        let stdout = env.stdout.into_string();
        let stderr = env.stderr.into_string();
        println!("stdout: {:?}", stdout);
        println!("stderr: {:?}", stderr);
        assert_eq!(0, result.code());
        assert_eq!("", stdout);
    }

    #[test]
    fn or_chain_first_fails() {
        let mut env = make_test_env(vec!["unused"]);
        let result = run("false || echo fallback".to_string(), &mut env).unwrap();
        let stdout = env.stdout.into_string();
        let stderr = env.stderr.into_string();
        println!("stdout: {:?}", stdout);
        println!("stderr: {:?}", stderr);
        assert_eq!(0, result.code());
        assert_eq!("fallback\n", stdout);
    }

    #[test]
    fn multiple_and_chain() {
        let mut env = make_test_env(vec!["unused"]);
        let result = run("true && echo one && echo two".to_string(), &mut env).unwrap();
        let stdout = env.stdout.into_string();
        let stderr = env.stderr.into_string();
        println!("stdout: {:?}", stdout);
        println!("stderr: {:?}", stderr);
        assert_eq!(0, result.code());
        assert_eq!("one\ntwo\n", stdout);
    }

    #[test]
    fn and_chain_stops_on_failure() {
        let mut env = make_test_env(vec!["unused"]);
        let result = run(
            "true && false && echo should_not_appear".to_string(),
            &mut env,
        )
        .unwrap();
        let stdout = env.stdout.into_string();
        let stderr = env.stderr.into_string();
        println!("stdout: {:?}", stdout);
        println!("stderr: {:?}", stderr);
        assert_eq!(1, result.code());
        assert_eq!("", stdout);
    }

    #[test]
    fn mixed_and_or_chain() {
        let mut env = make_test_env(vec!["unused"]);
        let result = run(
            "false || echo fallback && echo then_this".to_string(),
            &mut env,
        )
        .unwrap();
        let stdout = env.stdout.into_string();
        let stderr = env.stderr.into_string();
        println!("stdout: {:?}", stdout);
        println!("stderr: {:?}", stderr);
        assert_eq!(0, result.code());
        assert_eq!("fallback\nthen_this\n", stdout);
    }

    #[test]
    fn semicolon_separates_commands() {
        let mut env = make_test_env(vec!["unused"]);
        let result = run("echo first ; echo second".to_string(), &mut env).unwrap();
        let stdout = env.stdout.into_string();
        let stderr = env.stderr.into_string();
        println!("stdout: {:?}", stdout);
        println!("stderr: {:?}", stderr);
        assert_eq!(0, result.code());
        assert_eq!("first\nsecond\n", stdout);
    }

    #[test]
    fn semicolon_runs_after_failure() {
        let mut env = make_test_env(vec!["unused"]);
        let result = run("false ; echo still_runs".to_string(), &mut env).unwrap();
        let stdout = env.stdout.into_string();
        let stderr = env.stderr.into_string();
        println!("stdout: {:?}", stdout);
        println!("stderr: {:?}", stderr);
        assert_eq!(0, result.code());
        assert_eq!("still_runs\n", stdout);
    }

    #[test]
    fn multiple_semicolons() {
        let mut env = make_test_env(vec!["unused"]);
        let result = run("echo one ; echo two ; echo three".to_string(), &mut env).unwrap();
        let stdout = env.stdout.into_string();
        let stderr = env.stderr.into_string();
        println!("stdout: {:?}", stdout);
        println!("stderr: {:?}", stderr);
        assert_eq!(0, result.code());
        assert_eq!("one\ntwo\nthree\n", stdout);
    }

    #[test]
    fn semicolon_with_and_chain() {
        let mut env = make_test_env(vec!["unused"]);
        let result = run("echo first ; true && echo second".to_string(), &mut env).unwrap();
        let stdout = env.stdout.into_string();
        let stderr = env.stderr.into_string();
        println!("stdout: {:?}", stdout);
        println!("stderr: {:?}", stderr);
        assert_eq!(0, result.code());
        assert_eq!("first\nsecond\n", stdout);
    }

    #[test]
    fn and_with_pwd() {
        let mut env = make_test_env(vec!["unused"]);
        let result = run("pwd && echo done".to_string(), &mut env).unwrap();
        let stdout = env.stdout.into_string();
        let stderr = env.stderr.into_string();
        println!("stdout: {:?}", stdout);
        println!("stderr: {:?}", stderr);
        assert_eq!(0, result.code());
        assert_eq!("/\ndone\n", stdout);
    }

    // ========================================================================
    // pipe tests
    // ========================================================================

    #[test]
    fn pipe_echo_to_cat() {
        let mut env = make_test_env(vec!["unused"]);
        let result = run("echo hello | cat".to_string(), &mut env).unwrap();
        let stdout = env.stdout.into_string();
        let stderr = env.stderr.into_string();
        println!("stdout: {:?}", stdout);
        println!("stderr: {:?}", stderr);
        assert_eq!(0, result.code());
        assert_eq!("hello\n", stdout);
    }

    #[test]
    fn pipe_echo_quoted_to_cat() {
        // Test case from log: echo "test" | cat
        let mut env = make_test_env(vec!["unused"]);
        let result = run(r#"echo "test" | cat"#.to_string(), &mut env).unwrap();
        let stdout = env.stdout.into_string();
        let stderr = env.stderr.into_string();
        println!("stdout: {:?}", stdout);
        println!("stderr: {:?}", stderr);
        assert_eq!(0, result.code());
        assert_eq!("test\n", stdout);
    }

    #[test]
    fn pipe_cat_file_to_wc() {
        let mut env = make_test_env(vec!["unused"]);
        env.fs.add_file("test.txt", "one\ntwo\nthree\n");
        let result = run("cat test.txt | wc -l".to_string(), &mut env).unwrap();
        let stdout = env.stdout.into_string();
        let stderr = env.stderr.into_string();
        println!("stdout: {:?}", stdout);
        println!("stderr: {:?}", stderr);
        assert_eq!(0, result.code());
        assert!(
            stdout.trim() == "3",
            "expected '3', got '{}'",
            stdout.trim()
        );
    }

    #[test]
    fn pipe_three_commands() {
        let mut env = make_test_env(vec!["unused"]);
        env.fs.add_file("test.txt", "cherry\napple\nbanana\n");
        let result = run("cat test.txt | sort | head -n 2".to_string(), &mut env).unwrap();
        let stdout = env.stdout.into_string();
        let stderr = env.stderr.into_string();
        println!("stdout: {:?}", stdout);
        println!("stderr: {:?}", stderr);
        assert_eq!(0, result.code());
        assert_eq!("apple\nbanana\n", stdout);
    }

    #[test]
    fn pipe_with_grep() {
        let mut env = make_test_env(vec!["unused"]);
        env.fs
            .add_file("test.txt", "apple\nbanana\napricot\ncherry\n");
        let result = run("cat test.txt | grep ap".to_string(), &mut env).unwrap();
        let stdout = env.stdout.into_string();
        let stderr = env.stderr.into_string();
        println!("stdout: {:?}", stdout);
        println!("stderr: {:?}", stderr);
        assert_eq!(0, result.code());
        assert_eq!("apple\napricot\n", stdout);
    }

    #[test]
    fn pipe_combined_with_and() {
        let mut env = make_test_env(vec!["unused"]);
        let result = run("echo hello | cat && echo done".to_string(), &mut env).unwrap();
        let stdout = env.stdout.into_string();
        let stderr = env.stderr.into_string();
        println!("stdout: {:?}", stdout);
        println!("stderr: {:?}", stderr);
        assert_eq!(0, result.code());
        assert_eq!("hello\ndone\n", stdout);
    }

    #[test]
    fn pipe_exit_code_from_last_command() {
        let mut env = make_test_env(vec!["unused"]);
        let result = run("echo hello | false".to_string(), &mut env).unwrap();
        let stdout = env.stdout.into_string();
        let stderr = env.stderr.into_string();
        println!("stdout: {:?}", stdout);
        println!("stderr: {:?}", stderr);
        assert_eq!(1, result.code());
    }

    // ========================================================================
    // output redirection tests
    // ========================================================================

    #[test]
    fn redirect_echo_to_file() {
        let mut env = make_test_env(vec!["unused"]);
        let result = run("echo hello > /output.txt".to_string(), &mut env).unwrap();
        let stdout = env.stdout.into_string();
        let stderr = env.stderr.into_string();
        println!("stdout: {:?}", stdout);
        println!("stderr: {:?}", stderr);
        assert_eq!(0, result.code());
        assert_eq!("", stdout);
        let contents = env.fs.read_to_string("/output.txt").unwrap();
        assert_eq!("hello\n", contents);
    }

    #[test]
    fn redirect_cat_to_file() {
        let mut env = make_test_env(vec!["unused"]);
        env.fs.add_file("/input.txt", "file contents\n");
        let result = run("cat /input.txt > /output.txt".to_string(), &mut env).unwrap();
        let stdout = env.stdout.into_string();
        let stderr = env.stderr.into_string();
        println!("stdout: {:?}", stdout);
        println!("stderr: {:?}", stderr);
        assert_eq!(0, result.code());
        assert_eq!("", stdout);
        let contents = env.fs.read_to_string("/output.txt").unwrap();
        assert_eq!("file contents\n", contents);
    }

    #[test]
    fn redirect_pipeline_to_file() {
        let mut env = make_test_env(vec!["unused"]);
        env.fs.add_file("/input.txt", "cherry\napple\nbanana\n");
        let result = run("cat /input.txt | sort > /output.txt".to_string(), &mut env).unwrap();
        let stdout = env.stdout.into_string();
        let stderr = env.stderr.into_string();
        println!("stdout: {:?}", stdout);
        println!("stderr: {:?}", stderr);
        assert_eq!(0, result.code());
        assert_eq!("", stdout);
        let contents = env.fs.read_to_string("/output.txt").unwrap();
        assert_eq!("apple\nbanana\ncherry\n", contents);
    }

    #[test]
    fn redirect_with_and_chain() {
        let mut env = make_test_env(vec!["unused"]);
        let result = run(
            "echo hello > /output.txt && echo done".to_string(),
            &mut env,
        )
        .unwrap();
        let stdout = env.stdout.into_string();
        let stderr = env.stderr.into_string();
        println!("stdout: {:?}", stdout);
        println!("stderr: {:?}", stderr);
        assert_eq!(0, result.code());
        assert_eq!("done\n", stdout);
        let contents = env.fs.read_to_string("/output.txt").unwrap();
        assert_eq!("hello\n", contents);
    }

    #[test]
    fn redirect_no_target_is_error() {
        let mut env = make_test_env(vec!["unused"]);
        let result = run("echo hello >".to_string(), &mut env);
        println!("result: {:?}", result);
        assert!(result.is_err());
    }

    #[test]
    fn redirect_relative_path() {
        let mut env = crate::test_utils::TestEnvBuilder::new().cwd("/tmp").build();
        env.fs.mkdir("/tmp").unwrap();
        let result = run("echo hello > output.txt".to_string(), &mut env).unwrap();
        let stdout = env.stdout.into_string();
        let stderr = env.stderr.into_string();
        println!("stdout: {:?}", stdout);
        println!("stderr: {:?}", stderr);
        assert_eq!(0, result.code());
        let contents = env.fs.read_to_string("/tmp/output.txt").unwrap();
        assert_eq!("hello\n", contents);
    }

    #[test]
    fn redirect_no_space_before_filename() {
        // Bug 1 reproduction: echo "test" > /tmp/test.txt with >file (no space)
        let mut env = make_test_env(vec!["unused"]);
        let result = run("echo hello >/output.txt".to_string(), &mut env).unwrap();
        let stdout = env.stdout.into_string();
        let stderr = env.stderr.into_string();
        println!("stdout: {:?}", stdout);
        println!("stderr: {:?}", stderr);
        assert_eq!(0, result.code());
        assert_eq!("", stdout);
        let contents = env.fs.read_to_string("/output.txt").unwrap();
        assert_eq!("hello\n", contents);
    }

    #[test]
    fn append_redirect_with_space() {
        // Bug 2 reproduction: echo "append" >> /tmp/test.txt
        let mut env = make_test_env(vec!["unused"]);
        env.fs.add_file("/output.txt", "first\n");
        let result = run("echo second >> /output.txt".to_string(), &mut env).unwrap();
        let stdout = env.stdout.into_string();
        let stderr = env.stderr.into_string();
        println!("stdout: {:?}", stdout);
        println!("stderr: {:?}", stderr);
        assert_eq!(0, result.code());
        assert_eq!("", stdout);
        let contents = env.fs.read_to_string("/output.txt").unwrap();
        assert_eq!("first\nsecond\n", contents);
    }

    #[test]
    fn append_redirect_no_space() {
        let mut env = make_test_env(vec!["unused"]);
        env.fs.add_file("/output.txt", "first\n");
        let result = run("echo second >>/output.txt".to_string(), &mut env).unwrap();
        let stdout = env.stdout.into_string();
        let stderr = env.stderr.into_string();
        println!("stdout: {:?}", stdout);
        println!("stderr: {:?}", stderr);
        assert_eq!(0, result.code());
        assert_eq!("", stdout);
        let contents = env.fs.read_to_string("/output.txt").unwrap();
        assert_eq!("first\nsecond\n", contents);
    }

    #[test]
    fn append_redirect_creates_file() {
        let mut env = make_test_env(vec!["unused"]);
        let result = run("echo hello >> /output.txt".to_string(), &mut env).unwrap();
        let stdout = env.stdout.into_string();
        let stderr = env.stderr.into_string();
        println!("stdout: {:?}", stdout);
        println!("stderr: {:?}", stderr);
        assert_eq!(0, result.code());
        let contents = env.fs.read_to_string("/output.txt").unwrap();
        assert_eq!("hello\n", contents);
    }

    #[test]
    fn append_redirect_multiple_times() {
        let mut env = make_test_env(vec!["unused"]);
        let _ = run("echo one >> /output.txt".to_string(), &mut env).unwrap();
        let _ = run("echo two >> /output.txt".to_string(), &mut env).unwrap();
        let _ = run("echo three >> /output.txt".to_string(), &mut env).unwrap();
        let contents = env.fs.read_to_string("/output.txt").unwrap();
        assert_eq!("one\ntwo\nthree\n", contents);
    }

    #[test]
    fn append_redirect_in_pipeline() {
        let mut env = make_test_env(vec!["unused"]);
        env.fs.add_file("/input.txt", "cherry\napple\nbanana\n");
        env.fs.add_file("/output.txt", "header\n");
        let result = run("cat /input.txt | sort >> /output.txt".to_string(), &mut env).unwrap();
        let stdout = env.stdout.into_string();
        let stderr = env.stderr.into_string();
        println!("stdout: {:?}", stdout);
        println!("stderr: {:?}", stderr);
        assert_eq!(0, result.code());
        let contents = env.fs.read_to_string("/output.txt").unwrap();
        assert_eq!("header\napple\nbanana\ncherry\n", contents);
    }

    #[test]
    fn append_no_target_is_error() {
        let mut env = make_test_env(vec!["unused"]);
        let result = run("echo hello >>".to_string(), &mut env);
        println!("result: {:?}", result);
        assert!(result.is_err());
    }

    // ========================================================================
    // stderr redirection tests
    // ========================================================================

    #[test]
    fn stderr_redirect_to_file() {
        // Bug 3 reproduction: ls /nonexistent 2>/dev/null
        let mut env = make_test_env(vec!["unused"]);
        let result = run("cat /nonexistent 2>/error.txt".to_string(), &mut env).unwrap();
        let stdout = env.stdout.into_string();
        let stderr = env.stderr.into_string();
        println!("stdout: {:?}", stdout);
        println!("stderr: {:?}", stderr);
        // Command should fail, but stderr should be redirected to file
        assert_eq!(1, result.code());
        assert_eq!("", stderr);
        let error_contents = env.fs.read_to_string("/error.txt").unwrap();
        println!("error_contents: {:?}", error_contents);
        assert!(error_contents.contains("nonexistent"));
    }

    #[test]
    fn stderr_redirect_to_dev_null_style() {
        // Use a writable file path as /dev/null equivalent
        let mut env = make_test_env(vec!["unused"]);
        let result = run("cat /nonexistent 2>/null.txt".to_string(), &mut env).unwrap();
        let stdout = env.stdout.into_string();
        let stderr = env.stderr.into_string();
        println!("stdout: {:?}", stdout);
        println!("stderr: {:?}", stderr);
        assert_eq!(1, result.code());
        assert_eq!("", stderr);
    }

    #[test]
    fn stderr_append_redirect() {
        let mut env = make_test_env(vec!["unused"]);
        env.fs.add_file("/error.txt", "previous error\n");
        let _ = run("cat /nonexistent 2>>/error.txt".to_string(), &mut env).unwrap();
        let error_contents = env.fs.read_to_string("/error.txt").unwrap();
        println!("error_contents: {:?}", error_contents);
        assert!(error_contents.starts_with("previous error\n"));
        assert!(error_contents.contains("nonexistent"));
    }

    #[test]
    fn stderr_to_stdout_redirect() {
        // Test 2>&1 without stdout redirection
        let mut env = make_test_env(vec!["unused"]);
        let result = run("cat /nonexistent 2>&1".to_string(), &mut env).unwrap();
        let stdout = env.stdout.into_string();
        let stderr = env.stderr.into_string();
        println!("stdout: {:?}", stdout);
        println!("stderr: {:?}", stderr);
        assert_eq!(1, result.code());
        // Stderr should be empty (redirected to stdout)
        assert_eq!("", stderr);
        // Stdout should contain the error
        assert!(stdout.contains("nonexistent"));
    }

    #[test]
    fn stderr_and_stdout_to_same_file() {
        // Test > file 2>&1 (both to same file)
        let mut env = make_test_env(vec!["unused"]);
        env.fs.add_file("/input.txt", "hello\n");
        // First cat succeeds, second fails - both outputs go to file
        let result = run(
            "cat /input.txt /nonexistent >/output.txt 2>&1".to_string(),
            &mut env,
        )
        .unwrap();
        let stdout = env.stdout.into_string();
        let stderr = env.stderr.into_string();
        println!("stdout: {:?}", stdout);
        println!("stderr: {:?}", stderr);
        assert_eq!(1, result.code());
        assert_eq!("", stdout);
        assert_eq!("", stderr);
        let output = env.fs.read_to_string("/output.txt").unwrap();
        println!("output: {:?}", output);
        assert!(output.contains("hello"));
        assert!(output.contains("nonexistent"));
    }

    #[test]
    fn separate_stdout_and_stderr_files() {
        let mut env = make_test_env(vec!["unused"]);
        env.fs.add_file("/input.txt", "hello\n");
        let result = run(
            "cat /input.txt /nonexistent >/out.txt 2>/err.txt".to_string(),
            &mut env,
        )
        .unwrap();
        let stdout = env.stdout.into_string();
        let stderr = env.stderr.into_string();
        println!("stdout: {:?}", stdout);
        println!("stderr: {:?}", stderr);
        assert_eq!(1, result.code());
        let out = env.fs.read_to_string("/out.txt").unwrap();
        let err = env.fs.read_to_string("/err.txt").unwrap();
        println!("out: {:?}", out);
        println!("err: {:?}", err);
        assert_eq!("hello\n", out);
        assert!(err.contains("nonexistent"));
    }

    // ========================================================================
    // variable expansion tests
    // ========================================================================

    #[test]
    fn expand_bareword_variable() {
        let mut env = crate::test_utils::TestEnvBuilder::new()
            .env_var("GREETING", "hello")
            .build();
        let result = run("echo $GREETING".to_string(), &mut env).unwrap();
        let stdout = env.stdout.into_string();
        let stderr = env.stderr.into_string();
        println!("stdout: {:?}", stdout);
        println!("stderr: {:?}", stderr);
        assert_eq!(0, result.code());
        assert_eq!("hello\n", stdout);
    }

    #[test]
    fn expand_curly_brace_variable() {
        let mut env = crate::test_utils::TestEnvBuilder::new()
            .env_var("NAME", "world")
            .build();
        let result = run("echo ${NAME}".to_string(), &mut env).unwrap();
        let stdout = env.stdout.into_string();
        let stderr = env.stderr.into_string();
        println!("stdout: {:?}", stdout);
        println!("stderr: {:?}", stderr);
        assert_eq!(0, result.code());
        assert_eq!("world\n", stdout);
    }

    #[test]
    fn expand_multiple_variables() {
        let mut env = crate::test_utils::TestEnvBuilder::new()
            .env_var("FIRST", "hello")
            .env_var("SECOND", "world")
            .build();
        let result = run("echo $FIRST $SECOND".to_string(), &mut env).unwrap();
        let stdout = env.stdout.into_string();
        let stderr = env.stderr.into_string();
        println!("stdout: {:?}", stdout);
        println!("stderr: {:?}", stderr);
        assert_eq!(0, result.code());
        assert_eq!("hello world\n", stdout);
    }

    #[test]
    fn expand_mixed_syntax() {
        let mut env = crate::test_utils::TestEnvBuilder::new()
            .env_var("A", "alpha")
            .env_var("B", "beta")
            .build();
        let result = run("echo $A and ${B}".to_string(), &mut env).unwrap();
        let stdout = env.stdout.into_string();
        let stderr = env.stderr.into_string();
        println!("stdout: {:?}", stdout);
        println!("stderr: {:?}", stderr);
        assert_eq!(0, result.code());
        assert_eq!("alpha and beta\n", stdout);
    }

    #[test]
    fn expand_undefined_variable_is_empty() {
        let mut env = make_test_env(vec!["unused"]);
        let result = run("echo prefix$UNDEFINED suffix".to_string(), &mut env).unwrap();
        let stdout = env.stdout.into_string();
        let stderr = env.stderr.into_string();
        println!("stdout: {:?}", stdout);
        println!("stderr: {:?}", stderr);
        assert_eq!(0, result.code());
        assert_eq!("prefix suffix\n", stdout);
    }

    #[test]
    fn expand_variable_in_pipeline() {
        let mut env = crate::test_utils::TestEnvBuilder::new()
            .env_var("MSG", "hello world")
            .build();
        let result = run("echo $MSG | cat".to_string(), &mut env).unwrap();
        let stdout = env.stdout.into_string();
        let stderr = env.stderr.into_string();
        println!("stdout: {:?}", stdout);
        println!("stderr: {:?}", stderr);
        assert_eq!(0, result.code());
        assert_eq!("hello world\n", stdout);
    }

    #[test]
    fn expand_variable_with_and_chain() {
        let mut env = crate::test_utils::TestEnvBuilder::new()
            .env_var("VAR", "value")
            .build();
        let result = run("echo $VAR && echo done".to_string(), &mut env).unwrap();
        let stdout = env.stdout.into_string();
        let stderr = env.stderr.into_string();
        println!("stdout: {:?}", stdout);
        println!("stderr: {:?}", stderr);
        assert_eq!(0, result.code());
        assert_eq!("value\ndone\n", stdout);
    }

    #[test]
    fn expand_variable_as_filename() {
        let mut env = crate::test_utils::TestEnvBuilder::new()
            .env_var("FILE", "test.txt")
            .build();
        env.fs.add_file("test.txt", "file contents\n");
        let result = run("cat $FILE".to_string(), &mut env).unwrap();
        let stdout = env.stdout.into_string();
        let stderr = env.stderr.into_string();
        println!("stdout: {:?}", stdout);
        println!("stderr: {:?}", stderr);
        assert_eq!(0, result.code());
        assert_eq!("file contents\n", stdout);
    }

    #[test]
    fn expand_curly_brace_default_value() {
        let mut env = make_test_env(vec!["unused"]);
        let result = run("echo ${MISSING:-default}".to_string(), &mut env).unwrap();
        let stdout = env.stdout.into_string();
        let stderr = env.stderr.into_string();
        println!("stdout: {:?}", stdout);
        println!("stderr: {:?}", stderr);
        assert_eq!(0, result.code());
        assert_eq!("default\n", stdout);
    }

    #[test]
    fn expand_curly_brace_default_not_used_when_set() {
        let mut env = crate::test_utils::TestEnvBuilder::new()
            .env_var("PRESENT", "actual")
            .build();
        let result = run("echo ${PRESENT:-default}".to_string(), &mut env).unwrap();
        let stdout = env.stdout.into_string();
        let stderr = env.stderr.into_string();
        println!("stdout: {:?}", stdout);
        println!("stderr: {:?}", stderr);
        assert_eq!(0, result.code());
        assert_eq!("actual\n", stdout);
    }

    // ========================================================================
    // set builtin tests
    // ========================================================================

    #[test]
    fn set_single_variable() {
        let mut env = make_test_env(vec!["unused"]);
        let result = run("set FOO=bar".to_string(), &mut env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!(Some(&"bar".to_string()), env.vars.get("FOO"));
    }

    #[test]
    fn set_variable_then_expand() {
        let mut env = make_test_env(vec!["unused"]);
        let result = run_string("set GREETING=hello\necho $GREETING", &mut env).unwrap();
        let stdout = env.stdout.into_string();
        let stderr = env.stderr.into_string();
        println!("stdout: {:?}", stdout);
        println!("stderr: {:?}", stderr);
        assert_eq!(0, result.code());
        assert_eq!("hello\n", stdout);
    }

    #[test]
    fn set_multiple_variables() {
        let mut env = make_test_env(vec!["unused"]);
        let result = run("set A=alpha B=beta".to_string(), &mut env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!(Some(&"alpha".to_string()), env.vars.get("A"));
        assert_eq!(Some(&"beta".to_string()), env.vars.get("B"));
    }

    #[test]
    fn set_variable_with_equals_in_value() {
        let mut env = make_test_env(vec!["unused"]);
        let result = run("set EQUATION=a=b".to_string(), &mut env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!(Some(&"a=b".to_string()), env.vars.get("EQUATION"));
    }

    #[test]
    fn set_overwrites_existing_var() {
        let mut env = make_test_env(vec!["unused"]);
        let _ = run("set X=old".to_string(), &mut env).unwrap();
        let _ = run("set X=new".to_string(), &mut env).unwrap();
        assert_eq!(Some(&"new".to_string()), env.vars.get("X"));
    }

    #[test]
    fn set_invalid_format_returns_error() {
        let mut env = make_test_env(vec!["unused"]);
        let result = run("set NOEQUALS".to_string(), &mut env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("not in NAME=VALUE format"));
    }

    #[test]
    fn set_shell_var_shadows_env_var() {
        let mut env = crate::test_utils::TestEnvBuilder::new()
            .env_var("VAR", "from_env")
            .build();
        let _ = run("set VAR=from_shell".to_string(), &mut env).unwrap();
        let result = run("echo $VAR".to_string(), &mut env).unwrap();
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert_eq!(0, result.code());
        assert_eq!("from_shell\n", stdout);
    }

    // ========================================================================
    // bare variable assignment tests (VAR=value syntax)
    // ========================================================================

    #[test]
    fn bare_assignment_sets_variable() {
        // Bug 2: FOO=bar should set variable without needing 'set' prefix
        let mut env = make_test_env(vec!["unused"]);
        let result = run("FOO=bar".to_string(), &mut env).unwrap();
        let stdout = env.stdout.into_string();
        let stderr = env.stderr.into_string();
        println!("stdout: {:?}", stdout);
        println!("stderr: {:?}", stderr);
        assert_eq!(0, result.code());
        assert_eq!(Some(&"bar".to_string()), env.vars.get("FOO"));
    }

    #[test]
    fn bare_assignment_then_expand() {
        let mut env = make_test_env(vec!["unused"]);
        let result = run_string("GREETING=hello\necho $GREETING", &mut env).unwrap();
        let stdout = env.stdout.into_string();
        let stderr = env.stderr.into_string();
        println!("stdout: {:?}", stdout);
        println!("stderr: {:?}", stderr);
        assert_eq!(0, result.code());
        assert_eq!("hello\n", stdout);
    }

    #[test]
    fn bare_assignment_multiple_on_same_line() {
        let mut env = make_test_env(vec!["unused"]);
        let result = run("A=alpha B=beta".to_string(), &mut env).unwrap();
        let stdout = env.stdout.into_string();
        let stderr = env.stderr.into_string();
        println!("stdout: {:?}", stdout);
        println!("stderr: {:?}", stderr);
        assert_eq!(0, result.code());
        assert_eq!(Some(&"alpha".to_string()), env.vars.get("A"));
        assert_eq!(Some(&"beta".to_string()), env.vars.get("B"));
    }

    #[test]
    fn bare_assignment_with_equals_in_value() {
        let mut env = make_test_env(vec!["unused"]);
        let result = run("EQUATION=a=b".to_string(), &mut env).unwrap();
        let stdout = env.stdout.into_string();
        let stderr = env.stderr.into_string();
        println!("stdout: {:?}", stdout);
        println!("stderr: {:?}", stderr);
        assert_eq!(0, result.code());
        assert_eq!(Some(&"a=b".to_string()), env.vars.get("EQUATION"));
    }

    #[test]
    fn bare_assignment_before_command() {
        // In POSIX shell, VAR=value cmd sets VAR only for cmd's environment
        let mut env = make_test_env(vec!["unused"]);
        env.fs.add_file("test.txt", "hello\n");
        let result = run("FOO=bar cat test.txt".to_string(), &mut env).unwrap();
        let stdout = env.stdout.into_string();
        let stderr = env.stderr.into_string();
        println!("stdout: {:?}", stdout);
        println!("stderr: {:?}", stderr);
        // The command should succeed
        assert_eq!(0, result.code());
        assert_eq!("hello\n", stdout);
    }

    #[test]
    fn bare_assignment_before_command_does_not_persist() {
        // VAR=value cmd should NOT persist VAR in the shell environment
        let mut env = make_test_env(vec!["unused"]);
        env.fs.add_file("test.txt", "hello\n");
        let _ = run("FOO=bar cat test.txt".to_string(), &mut env).unwrap();
        // FOO should not be set in the shell environment
        assert!(
            !env.vars.contains_key("FOO"),
            "FOO should not persist after VAR=value cmd"
        );
    }

    #[test]
    fn bare_assignment_before_command_is_visible_to_command() {
        // VAR=value echo $VAR should print the value
        let mut env = make_test_env(vec!["unused"]);
        let result = run("MSG=hello echo $MSG".to_string(), &mut env).unwrap();
        let stdout = env.stdout.into_string();
        let stderr = env.stderr.into_string();
        println!("stdout: {:?}", stdout);
        println!("stderr: {:?}", stderr);
        assert_eq!(0, result.code());
        assert_eq!("hello\n", stdout);
    }

    #[test]
    fn multiple_prefix_assignments_before_command() {
        // FOO=1 BAR=2 echo $FOO $BAR should print "1 2"
        let mut env = make_test_env(vec!["unused"]);
        let result = run("FOO=1 BAR=2 echo $FOO $BAR".to_string(), &mut env).unwrap();
        let stdout = env.stdout.into_string();
        let stderr = env.stderr.into_string();
        println!("stdout: {:?}", stdout);
        println!("stderr: {:?}", stderr);
        assert_eq!(0, result.code());
        assert_eq!("1 2\n", stdout);
    }

    #[test]
    fn prefix_assignment_with_spaces_in_value() {
        // MSG="hello world" echo $MSG should print "hello world"
        let mut env = make_test_env(vec!["unused"]);
        let result = run(r#"MSG="hello world" echo $MSG"#.to_string(), &mut env).unwrap();
        let stdout = env.stdout.into_string();
        let stderr = env.stderr.into_string();
        println!("stdout: {:?}", stdout);
        println!("stderr: {:?}", stderr);
        assert_eq!(0, result.code());
        assert_eq!("hello world\n", stdout);
    }

    #[test]
    fn prefix_assignment_single_quoted_arg_not_expanded() {
        // NOPRINT=printed echo '$NOPRINT' should print "$NOPRINT" literally
        // Single quotes prevent variable expansion
        let mut env = make_test_env(vec!["unused"]);
        let result = run(r#"NOPRINT=printed echo '$NOPRINT'"#.to_string(), &mut env).unwrap();
        let stdout = env.stdout.into_string();
        let stderr = env.stderr.into_string();
        println!("stdout: {:?}", stdout);
        println!("stderr: {:?}", stderr);
        assert_eq!(0, result.code());
        assert_eq!("$NOPRINT\n", stdout);
    }

    // ========================================================================
    // unset builtin tests
    // ========================================================================

    #[test]
    fn unset_shell_variable() {
        let mut env = make_test_env(vec!["unused"]);
        let _ = run("set FOO=bar".to_string(), &mut env).unwrap();
        assert!(env.vars.contains_key("FOO"));
        let result = run("unset FOO".to_string(), &mut env).unwrap();
        assert_eq!(0, result.code());
        assert!(!env.vars.contains_key("FOO"));
    }

    #[test]
    fn unset_env_variable() {
        let mut env = crate::test_utils::TestEnvBuilder::new()
            .env_var("MYVAR", "value")
            .build();
        assert!(env.env.contains_key("MYVAR"));
        let result = run("unset MYVAR".to_string(), &mut env).unwrap();
        assert_eq!(0, result.code());
        assert!(!env.env.contains_key("MYVAR"));
    }

    #[test]
    fn unset_multiple_variables() {
        let mut env = make_test_env(vec!["unused"]);
        let _ = run("set A=1 B=2 C=3".to_string(), &mut env).unwrap();
        let result = run("unset A C".to_string(), &mut env).unwrap();
        assert_eq!(0, result.code());
        assert!(!env.vars.contains_key("A"));
        assert!(env.vars.contains_key("B"));
        assert!(!env.vars.contains_key("C"));
    }

    #[test]
    fn unset_nonexistent_is_ok() {
        let mut env = make_test_env(vec!["unused"]);
        let result = run("unset NONEXISTENT".to_string(), &mut env).unwrap();
        assert_eq!(0, result.code());
    }

    #[test]
    fn unset_removes_from_both_vars_and_env() {
        let mut env = crate::test_utils::TestEnvBuilder::new()
            .env_var("DUP", "env_value")
            .build();
        env.vars.insert("DUP".to_string(), "var_value".to_string());
        let result = run("unset DUP".to_string(), &mut env).unwrap();
        assert_eq!(0, result.code());
        assert!(!env.vars.contains_key("DUP"));
        assert!(!env.env.contains_key("DUP"));
    }

    // ========================================================================
    // export builtin tests
    // ========================================================================

    #[test]
    fn export_with_value() {
        let mut env = make_test_env(vec!["unused"]);
        let result = run("export FOO=bar".to_string(), &mut env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!(Some(&"bar".to_string()), env.env.get("FOO"));
        assert!(!env.vars.contains_key("FOO"));
    }

    #[test]
    fn export_existing_shell_var() {
        let mut env = make_test_env(vec!["unused"]);
        let _ = run("set MYVAR=myvalue".to_string(), &mut env).unwrap();
        assert!(env.vars.contains_key("MYVAR"));
        assert!(!env.env.contains_key("MYVAR"));
        let result = run("export MYVAR".to_string(), &mut env).unwrap();
        assert_eq!(0, result.code());
        assert!(!env.vars.contains_key("MYVAR"));
        assert_eq!(Some(&"myvalue".to_string()), env.env.get("MYVAR"));
    }

    #[test]
    fn export_nonexistent_var_is_noop() {
        let mut env = make_test_env(vec!["unused"]);
        let result = run("export NONEXISTENT".to_string(), &mut env).unwrap();
        assert_eq!(0, result.code());
        assert!(!env.vars.contains_key("NONEXISTENT"));
        assert!(!env.env.contains_key("NONEXISTENT"));
    }

    #[test]
    fn export_multiple_vars() {
        let mut env = make_test_env(vec!["unused"]);
        let _ = run("set A=1 B=2".to_string(), &mut env).unwrap();
        let result = run("export A B".to_string(), &mut env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!(Some(&"1".to_string()), env.env.get("A"));
        assert_eq!(Some(&"2".to_string()), env.env.get("B"));
    }

    #[test]
    fn export_with_value_overwrites_existing() {
        let mut env = crate::test_utils::TestEnvBuilder::new()
            .env_var("VAR", "old")
            .build();
        let result = run("export VAR=new".to_string(), &mut env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!(Some(&"new".to_string()), env.env.get("VAR"));
    }

    #[test]
    fn exported_var_is_visible() {
        let mut env = make_test_env(vec!["unused"]);
        let _ = run("export MSG=hello".to_string(), &mut env).unwrap();
        let result = run("echo $MSG".to_string(), &mut env).unwrap();
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert_eq!(0, result.code());
        assert_eq!("hello\n", stdout);
    }

    #[test]
    fn set_then_export_workflow() {
        let mut env = make_test_env(vec!["unused"]);
        let result = run_string("set VAR=value\nexport VAR\necho $VAR", &mut env).unwrap();
        let stdout = env.stdout.into_string();
        let stderr = env.stderr.into_string();
        println!("stdout: {:?}", stdout);
        println!("stderr: {:?}", stderr);
        assert_eq!(0, result.code());
        assert_eq!("value\n", stdout);
        assert_eq!(Some(&"value".to_string()), env.env.get("VAR"));
        assert!(!env.vars.contains_key("VAR"));
    }

    // ========================================================================
    // cd builtin tests
    // ========================================================================

    #[test]
    fn cd_to_existing_directory() {
        let mut env = make_test_env(vec!["unused"]);
        env.fs.mkdir("/home").unwrap();
        let result = run("cd /home".to_string(), &mut env).unwrap();
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert_eq!(0, result.code());
        assert_eq!("/home", env.cwd.as_str());
    }

    #[test]
    fn cd_to_nonexistent_directory() {
        let mut env = make_test_env(vec!["unused"]);
        let result = run("cd /nonexistent".to_string(), &mut env).unwrap();
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert_eq!(1, result.code());
        assert!(stderr.contains("No such file or directory"));
        assert_eq!("/", env.cwd.as_str());
    }

    #[test]
    fn cd_to_file_not_directory() {
        let mut env = make_test_env(vec!["unused"]);
        env.fs.add_file("/myfile.txt", "contents");
        let result = run("cd /myfile.txt".to_string(), &mut env).unwrap();
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert_eq!(1, result.code());
        assert!(stderr.contains("Not a directory"));
        assert_eq!("/", env.cwd.as_str());
    }

    #[test]
    fn cd_relative_path() {
        let mut env = make_test_env(vec!["unused"]);
        env.fs.mkdir("/home").unwrap();
        env.fs.mkdir("/home/user").unwrap();
        let _ = run("cd /home".to_string(), &mut env).unwrap();
        let result = run("cd user".to_string(), &mut env).unwrap();
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert_eq!(0, result.code());
        assert_eq!("/home/user", env.cwd.as_str());
    }

    #[test]
    fn cd_with_dotdot() {
        let mut env = make_test_env(vec!["unused"]);
        env.fs.mkdir("/home").unwrap();
        env.fs.mkdir("/home/user").unwrap();
        let _ = run("cd /home/user".to_string(), &mut env).unwrap();
        let result = run("cd ..".to_string(), &mut env).unwrap();
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert_eq!(0, result.code());
        assert_eq!("/home", env.cwd.as_str());
    }

    #[test]
    fn cd_with_dot() {
        let mut env = make_test_env(vec!["unused"]);
        env.fs.mkdir("/home").unwrap();
        let _ = run("cd /home".to_string(), &mut env).unwrap();
        let result = run("cd .".to_string(), &mut env).unwrap();
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert_eq!(0, result.code());
        assert_eq!("/home", env.cwd.as_str());
    }

    #[test]
    fn cd_no_args_uses_home() {
        let mut env = crate::test_utils::TestEnvBuilder::new()
            .env_var("HOME", "/home/user")
            .build();
        env.fs.mkdir("/home").unwrap();
        env.fs.mkdir("/home/user").unwrap();
        let result = run("cd".to_string(), &mut env).unwrap();
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert_eq!(0, result.code());
        assert_eq!("/home/user", env.cwd.as_str());
    }

    #[test]
    fn cd_no_args_home_not_set() {
        let mut env = make_test_env(vec!["unused"]);
        let result = run("cd".to_string(), &mut env).unwrap();
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert_eq!(1, result.code());
        assert!(stderr.contains("HOME not set"));
    }

    #[test]
    fn cd_affects_subsequent_pwd() {
        let mut env = make_test_env(vec!["unused"]);
        env.fs.mkdir("/home").unwrap();
        let _ = run("cd /home".to_string(), &mut env).unwrap();
        let result = run("pwd".to_string(), &mut env).unwrap();
        let stdout = env.stdout.into_string();
        let stderr = env.stderr.into_string();
        println!("stdout: {:?}", stdout);
        println!("stderr: {:?}", stderr);
        assert_eq!(0, result.code());
        assert_eq!("/home\n", stdout);
    }

    #[test]
    fn cd_in_script() {
        let mut env = make_test_env(vec!["unused"]);
        env.fs.mkdir("/home").unwrap();
        env.fs.mkdir("/home/user").unwrap();
        let result = run_string("cd /home\npwd\ncd user\npwd", &mut env).unwrap();
        let stdout = env.stdout.into_string();
        let stderr = env.stderr.into_string();
        println!("stdout: {:?}", stdout);
        println!("stderr: {:?}", stderr);
        assert_eq!(0, result.code());
        assert_eq!("/home\n/home/user\n", stdout);
    }

    #[test]
    fn cd_with_and_chain() {
        let mut env = make_test_env(vec!["unused"]);
        env.fs.mkdir("/home").unwrap();
        let result = run("cd /home && pwd".to_string(), &mut env).unwrap();
        let stdout = env.stdout.into_string();
        let stderr = env.stderr.into_string();
        println!("stdout: {:?}", stdout);
        println!("stderr: {:?}", stderr);
        assert_eq!(0, result.code());
        assert_eq!("/home\n", stdout);
    }

    #[test]
    fn cd_failure_stops_and_chain() {
        let mut env = make_test_env(vec!["unused"]);
        let result = run(
            "cd /nonexistent && echo should_not_appear".to_string(),
            &mut env,
        )
        .unwrap();
        let stdout = env.stdout.into_string();
        let stderr = env.stderr.into_string();
        println!("stdout: {:?}", stdout);
        println!("stderr: {:?}", stderr);
        assert_eq!(1, result.code());
        assert_eq!("", stdout);
        assert!(stderr.contains("No such file or directory"));
    }

    #[test]
    fn cd_in_pipeline_fails() {
        // cd in a pipeline fails because it's a shell builtin that modifies state
        let mut env = make_test_env(vec!["unused"]);
        env.fs.mkdir("/home").unwrap();
        let result = run("echo hello | cd /home".to_string(), &mut env);
        println!("result: {:?}", result);
        // cd in a pipe is an error - it's not a regular command
        assert!(result.is_err());
        // cwd should be unchanged
        assert_eq!("/", env.cwd.as_str());
    }

    #[test]
    fn cd_later_in_and_chain() {
        // cd can appear later in a && chain
        let mut env = make_test_env(vec!["unused"]);
        env.fs.mkdir("/home").unwrap();
        let result = run("echo hello && cd /home && pwd".to_string(), &mut env).unwrap();
        let stdout = env.stdout.into_string();
        let stderr = env.stderr.into_string();
        println!("stdout: {:?}", stdout);
        println!("stderr: {:?}", stderr);
        assert_eq!(0, result.code());
        assert_eq!("hello\n/home\n", stdout);
        assert_eq!("/home", env.cwd.as_str());
    }

    #[test]
    fn cd_later_in_or_chain() {
        // cd can appear in || chain when previous command fails
        let mut env = make_test_env(vec!["unused"]);
        env.fs.mkdir("/home").unwrap();
        let result = run("false || cd /home".to_string(), &mut env).unwrap();
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert_eq!(0, result.code());
        assert_eq!("/home", env.cwd.as_str());
    }

    #[test]
    fn cd_later_in_semicolon_chain() {
        // cd can appear after semicolon
        let mut env = make_test_env(vec!["unused"]);
        env.fs.mkdir("/home").unwrap();
        let result = run("echo first ; cd /home ; pwd".to_string(), &mut env).unwrap();
        let stdout = env.stdout.into_string();
        let stderr = env.stderr.into_string();
        println!("stdout: {:?}", stdout);
        println!("stderr: {:?}", stderr);
        assert_eq!(0, result.code());
        assert_eq!("first\n/home\n", stdout);
        assert_eq!("/home", env.cwd.as_str());
    }

    // ========================================================================
    // shvar::split behavior tests (for understanding redirection parsing)
    // ========================================================================

    #[test]
    fn shvar_split_redirect_with_space() {
        let parts = shvar::split("echo hello > file.txt").unwrap();
        println!("shvar::split(\"echo hello > file.txt\") = {:?}", parts);
        assert_eq!(parts, vec!["echo", "hello", ">", "file.txt"]);
    }

    #[test]
    fn shvar_split_redirect_no_space_after() {
        let parts = shvar::split("echo hello >file.txt").unwrap();
        println!("shvar::split(\"echo hello >file.txt\") = {:?}", parts);
        // This shows whether > is parsed as part of the next token or separate
        assert_eq!(parts, vec!["echo", "hello", ">file.txt"]);
    }

    #[test]
    fn shvar_split_redirect_no_space_before() {
        let parts = shvar::split("echo hello> file.txt").unwrap();
        println!("shvar::split(\"echo hello> file.txt\") = {:?}", parts);
        // This shows whether > is parsed as part of the previous token or separate
        assert_eq!(parts, vec!["echo", "hello>", "file.txt"]);
    }

    #[test]
    fn shvar_split_append_redirect() {
        let parts = shvar::split("echo hello >> file.txt").unwrap();
        println!("shvar::split(\"echo hello >> file.txt\") = {:?}", parts);
        // This shows whether >> is recognized as a single token
        assert_eq!(parts, vec!["echo", "hello", ">>", "file.txt"]);
    }

    #[test]
    fn shvar_split_append_redirect_no_space() {
        let parts = shvar::split("echo hello >>file.txt").unwrap();
        println!("shvar::split(\"echo hello >>file.txt\") = {:?}", parts);
        assert_eq!(parts, vec!["echo", "hello", ">>file.txt"]);
    }

    #[test]
    fn shvar_split_stderr_redirect() {
        let parts = shvar::split("ls 2>/dev/null").unwrap();
        println!("shvar::split(\"ls 2>/dev/null\") = {:?}", parts);
        // This shows how 2>/dev/null is parsed
        assert_eq!(parts, vec!["ls", "2>/dev/null"]);
    }

    #[test]
    fn ls_with_stderr_redirect() {
        // Bug 3: ls /nonexistent 2>/dev/null
        // The stderr redirect should suppress the error message
        let mut env = make_test_env(vec!["unused"]);
        let result = run("ls /nonexistent 2>/null.txt".to_string(), &mut env).unwrap();
        let stdout = env.stdout.into_string();
        let stderr = env.stderr.into_string();
        println!("stdout: {:?}", stdout);
        println!("stderr: {:?}", stderr);
        // ls should return non-zero for nonexistent file
        assert!(result.code() != 0, "expected non-zero exit code");
        // stderr should be empty (redirected to file)
        assert_eq!("", stderr, "expected empty stderr (redirected)");
        // Error message should be in the file
        let error_contents = env.fs.read_to_string("/null.txt").unwrap();
        println!("error_contents: {:?}", error_contents);
        assert!(
            error_contents.contains("nonexistent") || error_contents.contains("No such"),
            "expected error message in redirected file"
        );
    }

    // ========================================================================
    // $? (exit status) tests
    // ========================================================================

    #[test]
    fn exit_status_after_success() {
        // Bug 1: $? should contain the exit status of the last command
        let mut env = make_test_env(vec!["unused"]);
        let _ = run("true".to_string(), &mut env).unwrap();
        let result = run("echo $?".to_string(), &mut env).unwrap();
        let stdout = env.stdout.into_string();
        let stderr = env.stderr.into_string();
        println!("stdout: {:?}", stdout);
        println!("stderr: {:?}", stderr);
        assert_eq!(0, result.code());
        assert_eq!("0\n", stdout, "expected $? to be 0 after true");
    }

    #[test]
    fn exit_status_after_failure() {
        let mut env = make_test_env(vec!["unused"]);
        let _ = run("false".to_string(), &mut env).unwrap();
        let result = run("echo $?".to_string(), &mut env).unwrap();
        let stdout = env.stdout.into_string();
        let stderr = env.stderr.into_string();
        println!("stdout: {:?}", stdout);
        println!("stderr: {:?}", stderr);
        assert_eq!(0, result.code());
        assert_eq!("1\n", stdout, "expected $? to be 1 after false");
    }

    #[test]
    fn exit_status_updates_after_each_command() {
        let mut env = make_test_env(vec!["unused"]);
        let result = run_string("true\necho $?\nfalse\necho $?\ntrue\necho $?", &mut env).unwrap();
        let stdout = env.stdout.into_string();
        let stderr = env.stderr.into_string();
        println!("stdout: {:?}", stdout);
        println!("stderr: {:?}", stderr);
        assert_eq!(0, result.code());
        assert_eq!("0\n1\n0\n", stdout);
    }

    #[test]
    fn shvar_split_stderr_to_stdout() {
        let parts = shvar::split("ls 2>&1").unwrap();
        println!("shvar::split(\"ls 2>&1\") = {:?}", parts);
        assert_eq!(parts, vec!["ls", "2>&1"]);
    }
}

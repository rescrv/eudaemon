use crate::{
    Command, Environment, Error, ExitCode, Filesystem, FsError, PipeReader, Stderr, Stdin, Stdout,
    mkpipe,
};

/// The sh builtin: execute shell commands.
///
/// Usage:
///   sh -c command_string [command_name [argument...]]
///   sh script_file [argument...]
pub fn bin<SI, SO, SE, FS>(env: &Environment<SI, SO, SE, FS>) -> Result<ExitCode, Error>
where
    SI: Stdin,
    SO: Stdout,
    SE: Stderr,
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

    if args[1] == "-c" {
        // sh -c command_string
        if args.len() < 3 {
            env.stderr
                .write_line("sh: -c: option requires an argument")?;
            return Ok(ExitCode::from(2));
        }
        let command_string = &args[2];
        run(command_string.clone(), env)
    } else {
        // sh script_file
        let script_path = &args[1];
        run_script(script_path, env)
    }
}

/// Run a script file, executing each line.
pub fn run_script<SI, SO, SE, FS>(
    path: &str,
    env: &Environment<SI, SO, SE, FS>,
) -> Result<ExitCode, Error>
where
    SI: Stdin,
    SO: Stdout,
    SE: Stderr,
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
    env: &Environment<SI, SO, SE, FS>,
) -> Result<ExitCode, Error>
where
    SI: Stdin,
    SO: Stdout,
    SE: Stderr,
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
    env: &Environment<SI, SO, SE, FS>,
) -> Result<ExitCode, Error>
where
    SI: Stdin,
    SO: Stdout,
    SE: Stderr,
    FS: Filesystem + 'static,
{
    let args = shvar::split(&command)?;
    if args.is_empty() || args[0].is_empty() {
        return Err(Error::EmptyCommand);
    }

    // Parse into commands separated by && and ||
    let commands = parse_command_chain(&args);
    run_command_chain(&commands, env)
}

/// Operator between pipelines in a chain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChainOp {
    /// Execute next pipeline only if previous succeeded (&&).
    And,
    /// Execute next pipeline only if previous failed (||).
    Or,
}

/// A pipeline is a sequence of commands connected by pipes.
#[derive(Debug)]
struct Pipeline {
    /// Commands in the pipeline, executed left-to-right with stdout->stdin connections.
    commands: Vec<Vec<String>>,
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
fn parse_command_chain(args: &[String]) -> Vec<ChainedPipeline> {
    let mut pipelines = Vec::new();
    let mut current_pipeline_args = Vec::new();

    for arg in args {
        if arg == "&&" {
            if !current_pipeline_args.is_empty() {
                pipelines.push(ChainedPipeline {
                    pipeline: parse_pipeline(&current_pipeline_args),
                    next_op: Some(ChainOp::And),
                });
                current_pipeline_args = Vec::new();
            }
        } else if arg == "||" {
            if !current_pipeline_args.is_empty() {
                pipelines.push(ChainedPipeline {
                    pipeline: parse_pipeline(&current_pipeline_args),
                    next_op: Some(ChainOp::Or),
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
            pipeline: parse_pipeline(&current_pipeline_args),
            next_op: None,
        });
    }

    pipelines
}

/// Parse arguments into a pipeline (commands separated by |).
fn parse_pipeline(args: &[String]) -> Pipeline {
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

    Pipeline { commands }
}

/// Run a chain of pipelines respecting && and || operators.
fn run_command_chain<SI, SO, SE, FS>(
    pipelines: &[ChainedPipeline],
    env: &Environment<SI, SO, SE, FS>,
) -> Result<ExitCode, Error>
where
    SI: Stdin,
    SO: Stdout,
    SE: Stderr,
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
    env: &Environment<SI, SO, SE, FS>,
) -> Result<ExitCode, Error>
where
    SI: Stdin,
    SO: Stdout,
    SE: Stderr,
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
        let argv0 = args[0].clone();
        let cmd_env = env.dup().with_args(args.clone());
        let command = Command::new(&argv0, cmd_env)?;
        return command.run();
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
                    args: args.clone(),
                    cwd: env.cwd.clone(),
                    exit_signaled: std::sync::Arc::clone(&env.exit_signaled),
                };
                let command = Command::new(&argv0, cmd_env)?;
                last_exit = command.run()?;
            } else {
                // Last command: read from pipe, write to original stdout
                let cmd_env = Environment {
                    stdin: reader,
                    stdout: env.stdout.dup(),
                    stderr: env.stderr.dup(),
                    fs: env.fs.dup(),
                    env: env.env.clone(),
                    args: args.clone(),
                    cwd: env.cwd.clone(),
                    exit_signaled: std::sync::Arc::clone(&env.exit_signaled),
                };
                let command = Command::new(&argv0, cmd_env)?;
                last_exit = command.run()?;
            }
        } else if let Some(writer) = writer {
            // First command: read from original stdin, write to pipe
            let cmd_env = Environment {
                stdin: env.stdin.dup(),
                stdout: writer,
                stderr: env.stderr.dup(),
                fs: env.fs.dup(),
                env: env.env.clone(),
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
        let env = make_test_env(vec!["unused"]);
        env.fs.add_file("test.txt", "hello\n");
        let result = run("cat test.txt".to_string(), &env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("hello\n", env.stdout.into_string());
    }

    #[test]
    fn run_empty_command() {
        let env = make_test_env(vec!["unused"]);
        let result = run("".to_string(), &env);
        println!("result: {:?}", result);
        assert!(matches!(result, Err(Error::EmptyCommand)));
    }

    #[test]
    fn run_unknown_binary() {
        let env = make_test_env(vec!["unused"]);
        let result = run("nonexistent".to_string(), &env);
        assert!(matches!(result, Err(Error::UnknownBinary(_))));
    }

    // ========================================================================
    // run_string tests
    // ========================================================================

    #[test]
    fn run_string_empty_script() {
        let env = make_test_env(vec!["unused"]);
        let result = run_string("", &env).unwrap();
        assert_eq!(0, result.code());
    }

    #[test]
    fn run_string_comment_only() {
        let env = make_test_env(vec!["unused"]);
        let result = run_string("# just a comment", &env).unwrap();
        assert_eq!(0, result.code());
    }

    #[test]
    fn run_string_shebang_only() {
        let env = make_test_env(vec!["unused"]);
        let result = run_string("#!/bin/sh", &env).unwrap();
        assert_eq!(0, result.code());
    }

    #[test]
    fn run_string_single_command() {
        let env = make_test_env(vec!["unused"]);
        env.fs.add_file("test.txt", "hello\n");
        let result = run_string("cat test.txt", &env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("hello\n", env.stdout.into_string());
    }

    #[test]
    fn run_string_multiple_commands() {
        let env = make_test_env(vec!["unused"]);
        env.fs.add_file("a.txt", "aaa\n");
        env.fs.add_file("b.txt", "bbb\n");
        let result = run_string("cat a.txt\ncat b.txt", &env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("aaa\nbbb\n", env.stdout.into_string());
    }

    #[test]
    fn run_string_skips_blank_lines() {
        let env = make_test_env(vec!["unused"]);
        env.fs.add_file("a.txt", "aaa\n");
        let result = run_string("\n\ncat a.txt\n\n", &env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("aaa\n", env.stdout.into_string());
    }

    #[test]
    fn run_string_skips_comments() {
        let env = make_test_env(vec!["unused"]);
        env.fs.add_file("a.txt", "aaa\n");
        let result = run_string("# comment\ncat a.txt\n# another comment", &env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("aaa\n", env.stdout.into_string());
    }

    #[test]
    fn run_string_with_shebang() {
        let env = make_test_env(vec!["unused"]);
        env.fs.add_file("a.txt", "aaa\n");
        let result = run_string("#!/bin/sh\ncat a.txt", &env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("aaa\n", env.stdout.into_string());
    }

    #[test]
    fn run_string_returns_last_exit_code() {
        let env = make_test_env(vec!["unused"]);
        env.fs.add_file("a.txt", "aaa\n");
        let result = run_string("cat a.txt\ncat nonexistent.txt", &env).unwrap();
        assert_eq!(1, result.code());
    }

    // ========================================================================
    // exit early termination tests
    // ========================================================================

    #[test]
    fn exit_terminates_script() {
        let env = make_test_env(vec!["unused"]);
        let result = run_string("exit 0\necho should_not_appear", &env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("", env.stdout.into_string());
    }

    #[test]
    fn exit_with_code_terminates_script() {
        let env = make_test_env(vec!["unused"]);
        let result = run_string("exit 42\necho should_not_appear", &env).unwrap();
        assert_eq!(42, result.code());
        assert_eq!("", env.stdout.into_string());
    }

    #[test]
    fn exit_terminates_after_other_commands() {
        let env = make_test_env(vec!["unused"]);
        let result = run_string("echo before\nexit 5\necho after", &env).unwrap();
        assert_eq!(5, result.code());
        assert_eq!("before\n", env.stdout.into_string());
    }

    #[test]
    fn exit_in_middle_of_script() {
        let env = make_test_env(vec!["unused"]);
        env.fs.add_file("a.txt", "aaa\n");
        env.fs.add_file("b.txt", "bbb\n");
        let result = run_string("cat a.txt\nexit 3\ncat b.txt", &env).unwrap();
        assert_eq!(3, result.code());
        assert_eq!("aaa\n", env.stdout.into_string());
    }

    #[test]
    fn true_script_exits_zero() {
        let env = make_test_env(vec!["unused"]);
        let result = run_string("#!/bin/sh\nexit 0", &env).unwrap();
        assert_eq!(0, result.code());
    }

    #[test]
    fn false_script_exits_one() {
        let env = make_test_env(vec!["unused"]);
        let result = run_string("#!/bin/sh\nexit 1", &env).unwrap();
        assert_eq!(1, result.code());
    }

    #[test]
    fn multiple_exits_uses_first() {
        let env = make_test_env(vec!["unused"]);
        let result = run_string("exit 7\nexit 8\nexit 9", &env).unwrap();
        assert_eq!(7, result.code());
    }

    #[test]
    fn exit_signal_persists() {
        let env = make_test_env(vec!["unused"]);
        assert!(!env.is_exit_signaled());
        let _ = run_string("exit 0", &env).unwrap();
        assert!(env.is_exit_signaled());
    }

    // ========================================================================
    // command chaining tests (&&, ||)
    // ========================================================================

    #[test]
    fn and_chain_both_succeed() {
        let env = make_test_env(vec!["unused"]);
        let result = run("true && echo success".to_string(), &env).unwrap();
        let stdout = env.stdout.into_string();
        let stderr = env.stderr.into_string();
        println!("stdout: {:?}", stdout);
        println!("stderr: {:?}", stderr);
        assert_eq!(0, result.code());
        assert_eq!("success\n", stdout);
    }

    #[test]
    fn and_chain_first_fails() {
        let env = make_test_env(vec!["unused"]);
        let result = run("false && echo should_not_appear".to_string(), &env).unwrap();
        let stdout = env.stdout.into_string();
        let stderr = env.stderr.into_string();
        println!("stdout: {:?}", stdout);
        println!("stderr: {:?}", stderr);
        assert_eq!(1, result.code());
        assert_eq!("", stdout);
    }

    #[test]
    fn or_chain_first_succeeds() {
        let env = make_test_env(vec!["unused"]);
        let result = run("true || echo should_not_appear".to_string(), &env).unwrap();
        let stdout = env.stdout.into_string();
        let stderr = env.stderr.into_string();
        println!("stdout: {:?}", stdout);
        println!("stderr: {:?}", stderr);
        assert_eq!(0, result.code());
        assert_eq!("", stdout);
    }

    #[test]
    fn or_chain_first_fails() {
        let env = make_test_env(vec!["unused"]);
        let result = run("false || echo fallback".to_string(), &env).unwrap();
        let stdout = env.stdout.into_string();
        let stderr = env.stderr.into_string();
        println!("stdout: {:?}", stdout);
        println!("stderr: {:?}", stderr);
        assert_eq!(0, result.code());
        assert_eq!("fallback\n", stdout);
    }

    #[test]
    fn multiple_and_chain() {
        let env = make_test_env(vec!["unused"]);
        let result = run("true && echo one && echo two".to_string(), &env).unwrap();
        let stdout = env.stdout.into_string();
        let stderr = env.stderr.into_string();
        println!("stdout: {:?}", stdout);
        println!("stderr: {:?}", stderr);
        assert_eq!(0, result.code());
        assert_eq!("one\ntwo\n", stdout);
    }

    #[test]
    fn and_chain_stops_on_failure() {
        let env = make_test_env(vec!["unused"]);
        let result = run("true && false && echo should_not_appear".to_string(), &env).unwrap();
        let stdout = env.stdout.into_string();
        let stderr = env.stderr.into_string();
        println!("stdout: {:?}", stdout);
        println!("stderr: {:?}", stderr);
        assert_eq!(1, result.code());
        assert_eq!("", stdout);
    }

    #[test]
    fn mixed_and_or_chain() {
        let env = make_test_env(vec!["unused"]);
        let result = run("false || echo fallback && echo then_this".to_string(), &env).unwrap();
        let stdout = env.stdout.into_string();
        let stderr = env.stderr.into_string();
        println!("stdout: {:?}", stdout);
        println!("stderr: {:?}", stderr);
        assert_eq!(0, result.code());
        assert_eq!("fallback\nthen_this\n", stdout);
    }

    #[test]
    fn and_with_pwd() {
        let env = make_test_env(vec!["unused"]);
        let result = run("pwd && echo done".to_string(), &env).unwrap();
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
        let env = make_test_env(vec!["unused"]);
        let result = run("echo hello | cat".to_string(), &env).unwrap();
        let stdout = env.stdout.into_string();
        let stderr = env.stderr.into_string();
        println!("stdout: {:?}", stdout);
        println!("stderr: {:?}", stderr);
        assert_eq!(0, result.code());
        assert_eq!("hello\n", stdout);
    }

    #[test]
    fn pipe_cat_file_to_wc() {
        let env = make_test_env(vec!["unused"]);
        env.fs.add_file("test.txt", "one\ntwo\nthree\n");
        let result = run("cat test.txt | wc -l".to_string(), &env).unwrap();
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
        let env = make_test_env(vec!["unused"]);
        env.fs.add_file("test.txt", "cherry\napple\nbanana\n");
        let result = run("cat test.txt | sort | head -n 2".to_string(), &env).unwrap();
        let stdout = env.stdout.into_string();
        let stderr = env.stderr.into_string();
        println!("stdout: {:?}", stdout);
        println!("stderr: {:?}", stderr);
        assert_eq!(0, result.code());
        assert_eq!("apple\nbanana\n", stdout);
    }

    #[test]
    fn pipe_with_grep() {
        let env = make_test_env(vec!["unused"]);
        env.fs
            .add_file("test.txt", "apple\nbanana\napricot\ncherry\n");
        let result = run("cat test.txt | grep ap".to_string(), &env).unwrap();
        let stdout = env.stdout.into_string();
        let stderr = env.stderr.into_string();
        println!("stdout: {:?}", stdout);
        println!("stderr: {:?}", stderr);
        assert_eq!(0, result.code());
        assert_eq!("apple\napricot\n", stdout);
    }

    #[test]
    fn pipe_combined_with_and() {
        let env = make_test_env(vec!["unused"]);
        let result = run("echo hello | cat && echo done".to_string(), &env).unwrap();
        let stdout = env.stdout.into_string();
        let stderr = env.stderr.into_string();
        println!("stdout: {:?}", stdout);
        println!("stderr: {:?}", stderr);
        assert_eq!(0, result.code());
        assert_eq!("hello\ndone\n", stdout);
    }

    #[test]
    fn pipe_exit_code_from_last_command() {
        let env = make_test_env(vec!["unused"]);
        let result = run("echo hello | false".to_string(), &env).unwrap();
        let stdout = env.stdout.into_string();
        let stderr = env.stderr.into_string();
        println!("stdout: {:?}", stdout);
        println!("stderr: {:?}", stderr);
        assert_eq!(1, result.code());
    }
}

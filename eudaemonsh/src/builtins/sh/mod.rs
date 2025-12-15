use crate::{Command, Environment, Error, ExitCode, Filesystem, FsError, Stderr, Stdin, Stdout};

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

/// Run a single command line.
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
    let argv0 = args[0].clone();
    let env = env.dup().with_args(args);
    let cmd = Command::new(&argv0, env)?;
    cmd.run()
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
}

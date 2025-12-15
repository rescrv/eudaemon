use getopts::Options;

use crate::{Environment, Error, ExitCode, Filesystem, FsError, Stderr, Stdin, Stdout};

fn build_options() -> Options {
    let mut opts = Options::new();
    opts.optflag(
        "a",
        "",
        "Append the output to the files rather than overwriting them.",
    );
    opts.optflag("i", "", "Ignore the SIGINT signal.");
    opts
}

/// The tee builtin: duplicate standard input to stdout and zero or more files.
pub fn bin<SI, SO, SE, FS>(env: &Environment<SI, SO, SE, FS>) -> Result<ExitCode, Error>
where
    SI: Stdin,
    SO: Stdout,
    SE: Stderr,
    FS: Filesystem,
{
    let opts_def = build_options();

    let matches = match opts_def.parse(&env.args[1..]) {
        Ok(m) => m,
        Err(e) => {
            env.stderr.write_line(&format!("tee: {}", e))?;
            return Ok(ExitCode::from(1));
        }
    };

    let append = matches.opt_present("a");
    // -i is a no-op in this implementation (no real signals in virtual environment)

    let files = &matches.free;

    // If append mode, initialize files that don't exist
    if !append {
        // In overwrite mode, truncate/create files before writing
        for path in files {
            if let Err(e) = env.fs.write_string(path, "") {
                env.stderr
                    .write_line(&format!("tee: {}: {}", path, format_io_error(&e)))?;
            }
        }
    }

    let mut exit_code: i8 = 0;

    // Read from stdin and write to stdout and all files
    while let Some(line) = env.stdin.read_line()? {
        let line_with_newline = format!("{}\n", line);

        // Write to stdout
        env.stdout.write_str(&line_with_newline)?;

        // Write to each file (files were truncated at start if not appending)
        for path in files {
            if let Err(e) = env.fs.append_string(path, &line_with_newline) {
                env.stderr
                    .write_line(&format!("tee: {}: {}", path, format_io_error(&e)))?;
                exit_code = 1;
            }
        }
    }

    Ok(ExitCode::from(exit_code))
}

/// Format an error for display, extracting the underlying IO error message.
fn format_io_error(e: &FsError) -> String {
    match e {
        FsError::Io(io_err) => io_err.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TestFilesystemExt;
    use crate::test_utils::make_test_env_with_stdin;

    // ========================================================================
    // Basic functionality tests
    // ========================================================================

    #[test]
    fn empty_stdin() {
        let env = make_test_env_with_stdin(vec!["tee"], "");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("", env.stdout.into_string());
    }

    #[test]
    fn single_line_no_files() {
        let env = make_test_env_with_stdin(vec!["tee"], "hello");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("hello\n", env.stdout.into_string());
    }

    #[test]
    fn multiple_lines_no_files() {
        let env = make_test_env_with_stdin(vec!["tee"], "line1\nline2\nline3");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("line1\nline2\nline3\n", env.stdout.into_string());
    }

    #[test]
    fn single_file() {
        let env = make_test_env_with_stdin(vec!["tee", "output.txt"], "hello world");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("hello world\n", env.stdout.into_string());
        assert_eq!(
            "hello world\n",
            env.fs.read_to_string("output.txt").unwrap()
        );
    }

    #[test]
    fn multiple_files() {
        let env = make_test_env_with_stdin(vec!["tee", "a.txt", "b.txt", "c.txt"], "data");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("data\n", env.stdout.into_string());
        assert_eq!("data\n", env.fs.read_to_string("a.txt").unwrap());
        assert_eq!("data\n", env.fs.read_to_string("b.txt").unwrap());
        assert_eq!("data\n", env.fs.read_to_string("c.txt").unwrap());
    }

    #[test]
    fn multiple_lines_to_file() {
        let env = make_test_env_with_stdin(vec!["tee", "output.txt"], "line1\nline2\nline3");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("line1\nline2\nline3\n", env.stdout.into_string());
        assert_eq!(
            "line1\nline2\nline3\n",
            env.fs.read_to_string("output.txt").unwrap()
        );
    }

    // ========================================================================
    // -a flag: append mode
    // ========================================================================

    #[test]
    fn append_to_existing_file() {
        let env = make_test_env_with_stdin(vec!["tee", "-a", "output.txt"], "new data");
        env.fs.add_file("output.txt", "existing\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("new data\n", env.stdout.into_string());
        assert_eq!(
            "existing\nnew data\n",
            env.fs.read_to_string("output.txt").unwrap()
        );
    }

    #[test]
    fn append_to_nonexistent_file() {
        let env = make_test_env_with_stdin(vec!["tee", "-a", "new.txt"], "data");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("data\n", env.stdout.into_string());
        assert_eq!("data\n", env.fs.read_to_string("new.txt").unwrap());
    }

    #[test]
    fn append_multiple_files() {
        let env = make_test_env_with_stdin(vec!["tee", "-a", "a.txt", "b.txt"], "appended");
        env.fs.add_file("a.txt", "file a\n");
        env.fs.add_file("b.txt", "file b\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!(
            "file a\nappended\n",
            env.fs.read_to_string("a.txt").unwrap()
        );
        assert_eq!(
            "file b\nappended\n",
            env.fs.read_to_string("b.txt").unwrap()
        );
    }

    #[test]
    fn overwrite_existing_file() {
        let env = make_test_env_with_stdin(vec!["tee", "output.txt"], "new data");
        env.fs
            .add_file("output.txt", "old data that should be gone\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("new data\n", env.fs.read_to_string("output.txt").unwrap());
    }

    // ========================================================================
    // -i flag: ignore SIGINT (no-op in this implementation)
    // ========================================================================

    #[test]
    fn ignore_sigint_flag() {
        let env = make_test_env_with_stdin(vec!["tee", "-i", "output.txt"], "data");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("data\n", env.stdout.into_string());
        assert_eq!("data\n", env.fs.read_to_string("output.txt").unwrap());
    }

    #[test]
    fn both_flags() {
        let env = make_test_env_with_stdin(vec!["tee", "-ai", "output.txt"], "new");
        env.fs.add_file("output.txt", "old\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("old\nnew\n", env.fs.read_to_string("output.txt").unwrap());
    }

    // ========================================================================
    // Option parsing tests
    // ========================================================================

    #[test]
    fn illegal_option() {
        let env = make_test_env_with_stdin(vec!["tee", "-x"], "");
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("Unrecognized option"));
    }

    #[test]
    fn double_dash_ends_options() {
        let env = make_test_env_with_stdin(vec!["tee", "--", "-a"], "data");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        // "-a" should be treated as a filename, not an option
        assert_eq!("data\n", env.fs.read_to_string("-a").unwrap());
    }

    // ========================================================================
    // Edge cases
    // ========================================================================

    #[test]
    fn empty_filename() {
        let env = make_test_env_with_stdin(vec!["tee", ""], "data");
        let result = bin(&env).unwrap();
        // Empty filename is an invalid path, so tee reports an error but still outputs to stdout
        assert_eq!(1, result.code());
        assert_eq!("data\n", env.stdout.into_string());
        println!("stderr: {}", env.stderr.into_string());
    }

    #[test]
    fn same_file_twice() {
        let env = make_test_env_with_stdin(vec!["tee", "dup.txt", "dup.txt"], "data");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        // Writing to the same file twice should result in double the data
        assert_eq!("data\ndata\n", env.fs.read_to_string("dup.txt").unwrap());
    }

    #[test]
    fn preserves_blank_lines() {
        let env = make_test_env_with_stdin(vec!["tee", "output.txt"], "a\n\nb");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("a\n\nb\n", env.stdout.into_string());
        assert_eq!("a\n\nb\n", env.fs.read_to_string("output.txt").unwrap());
    }

    #[test]
    fn whitespace_only_lines() {
        let env = make_test_env_with_stdin(vec!["tee", "output.txt"], "  \n\t\n   ");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("  \n\t\n   \n", env.stdout.into_string());
        assert_eq!(
            "  \n\t\n   \n",
            env.fs.read_to_string("output.txt").unwrap()
        );
    }
}

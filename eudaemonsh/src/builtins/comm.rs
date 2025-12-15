//! The comm builtin: select or reject lines common to two files.

use getopts::Options;

use crate::{Environment, Error, ExitCode, Filesystem, FsError, Stderr, Stdin, Stdout};

/// Options for the comm command.
#[derive(Clone, Copy, Debug, Default)]
struct CommOptions {
    /// Suppress printing of column 1 (lines only in file1).
    suppress_col1: bool,
    /// Suppress printing of column 2 (lines only in file2).
    suppress_col2: bool,
    /// Suppress printing of column 3 (lines common to both).
    suppress_col3: bool,
    /// Case insensitive comparison.
    ignore_case: bool,
}

fn build_options() -> Options {
    let mut opts = Options::new();
    opts.optflag("1", "", "Suppress printing of column 1.");
    opts.optflag("2", "", "Suppress printing of column 2.");
    opts.optflag("3", "", "Suppress printing of column 3.");
    opts.optflag("i", "", "Case insensitive comparison of lines.");
    opts
}

/// The comm builtin: select or reject lines common to two files.
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
            env.stderr.write_line(&format!("comm: {}", e))?;
            return Ok(ExitCode::from(1));
        }
    };

    let opts = CommOptions {
        suppress_col1: matches.opt_present("1"),
        suppress_col2: matches.opt_present("2"),
        suppress_col3: matches.opt_present("3"),
        ignore_case: matches.opt_present("i"),
    };

    let free = &matches.free;

    if free.len() != 2 {
        env.stderr.write_line("usage: comm [-123i] file1 file2")?;
        return Ok(ExitCode::from(1));
    }

    let file1_name = &free[0];
    let file2_name = &free[1];

    let file1_contents = read_file_or_stdin(env, file1_name)?;
    let file2_contents = read_file_or_stdin(env, file2_name)?;

    match (file1_contents, file2_contents) {
        (Some(c1), Some(c2)) => {
            process_comm(env, &c1, &c2, &opts)?;
            Ok(ExitCode::from(0))
        }
        (None, _) => {
            env.stderr
                .write_line(&format!("comm: {}: No such file or directory", file1_name))?;
            Ok(ExitCode::from(1))
        }
        (_, None) => {
            env.stderr
                .write_line(&format!("comm: {}: No such file or directory", file2_name))?;
            Ok(ExitCode::from(1))
        }
    }
}

/// Read file contents, or stdin if path is "-".
/// Returns None if file not found.
fn read_file_or_stdin<SI, SO, SE, FS>(
    env: &Environment<SI, SO, SE, FS>,
    path: &str,
) -> Result<Option<String>, Error>
where
    SI: Stdin,
    SO: Stdout,
    SE: Stderr,
    FS: Filesystem,
{
    if path == "-" {
        let mut contents = String::new();
        while let Some(line) = env.stdin.read_line()? {
            contents.push_str(&line);
            contents.push('\n');
        }
        Ok(Some(contents))
    } else {
        match env.fs.read_to_string(path) {
            Ok(contents) => Ok(Some(contents)),
            Err(FsError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }
}

/// Compare two lines, optionally case-insensitive.
fn compare_lines(a: &str, b: &str, ignore_case: bool) -> std::cmp::Ordering {
    if ignore_case {
        a.to_lowercase().cmp(&b.to_lowercase())
    } else {
        a.cmp(b)
    }
}

/// Process the comm algorithm on two file contents.
fn process_comm<SI, SO, SE, FS>(
    env: &Environment<SI, SO, SE, FS>,
    file1: &str,
    file2: &str,
    opts: &CommOptions,
) -> Result<(), Error>
where
    SI: Stdin,
    SO: Stdout,
    SE: Stderr,
    FS: Filesystem,
{
    let mut lines1 = file1.lines().peekable();
    let mut lines2 = file2.lines().peekable();

    // Calculate tab prefixes based on which columns are being printed
    let col1_prefix = "";
    let col2_prefix = if opts.suppress_col1 { "" } else { "\t" };
    let col3_prefix = {
        let mut tabs = 0;
        if !opts.suppress_col1 {
            tabs += 1;
        }
        if !opts.suppress_col2 {
            tabs += 1;
        }
        match tabs {
            0 => "",
            1 => "\t",
            _ => "\t\t",
        }
    };

    loop {
        match (lines1.peek(), lines2.peek()) {
            (None, None) => break,
            (Some(&line1), None) => {
                // File 2 exhausted, print remaining from file 1
                if !opts.suppress_col1 {
                    env.stdout
                        .write_line(&format!("{}{}", col1_prefix, line1))?;
                }
                lines1.next();
            }
            (None, Some(&line2)) => {
                // File 1 exhausted, print remaining from file 2
                if !opts.suppress_col2 {
                    env.stdout
                        .write_line(&format!("{}{}", col2_prefix, line2))?;
                }
                lines2.next();
            }
            (Some(&line1), Some(&line2)) => {
                match compare_lines(line1, line2, opts.ignore_case) {
                    std::cmp::Ordering::Less => {
                        // line1 < line2, line1 is only in file1
                        if !opts.suppress_col1 {
                            env.stdout
                                .write_line(&format!("{}{}", col1_prefix, line1))?;
                        }
                        lines1.next();
                    }
                    std::cmp::Ordering::Greater => {
                        // line1 > line2, line2 is only in file2
                        if !opts.suppress_col2 {
                            env.stdout
                                .write_line(&format!("{}{}", col2_prefix, line2))?;
                        }
                        lines2.next();
                    }
                    std::cmp::Ordering::Equal => {
                        // Lines are equal, common to both
                        if !opts.suppress_col3 {
                            env.stdout
                                .write_line(&format!("{}{}", col3_prefix, line1))?;
                        }
                        lines1.next();
                        lines2.next();
                    }
                }
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::make_test_env_with_stdin;

    // ========================================================================
    // Basic functionality tests
    // ========================================================================

    #[test]
    fn basic_all_columns() {
        let env = make_test_env_with_stdin(vec!["comm", "file1.txt", "file2.txt"], "");
        env.fs.add_file("file1.txt", "a\nb\nc\n");
        env.fs.add_file("file2.txt", "b\nc\nd\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert_eq!("a\n\t\tb\n\t\tc\n\td\n", stdout);
    }

    #[test]
    fn empty_files() {
        let env = make_test_env_with_stdin(vec!["comm", "file1.txt", "file2.txt"], "");
        env.fs.add_file("file1.txt", "");
        env.fs.add_file("file2.txt", "");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("", env.stdout.into_string());
    }

    #[test]
    fn file1_empty() {
        let env = make_test_env_with_stdin(vec!["comm", "file1.txt", "file2.txt"], "");
        env.fs.add_file("file1.txt", "");
        env.fs.add_file("file2.txt", "a\nb\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert_eq!("\ta\n\tb\n", stdout);
    }

    #[test]
    fn file2_empty() {
        let env = make_test_env_with_stdin(vec!["comm", "file1.txt", "file2.txt"], "");
        env.fs.add_file("file1.txt", "a\nb\n");
        env.fs.add_file("file2.txt", "");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert_eq!("a\nb\n", stdout);
    }

    #[test]
    fn identical_files() {
        let env = make_test_env_with_stdin(vec!["comm", "file1.txt", "file2.txt"], "");
        env.fs.add_file("file1.txt", "a\nb\nc\n");
        env.fs.add_file("file2.txt", "a\nb\nc\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert_eq!("\t\ta\n\t\tb\n\t\tc\n", stdout);
    }

    #[test]
    fn no_common_lines() {
        let env = make_test_env_with_stdin(vec!["comm", "file1.txt", "file2.txt"], "");
        env.fs.add_file("file1.txt", "a\nc\ne\n");
        env.fs.add_file("file2.txt", "b\nd\nf\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert_eq!("a\n\tb\nc\n\td\ne\n\tf\n", stdout);
    }

    // ========================================================================
    // Suppress column tests (-1, -2, -3)
    // ========================================================================

    #[test]
    fn suppress_col1() {
        let env = make_test_env_with_stdin(vec!["comm", "-1", "file1.txt", "file2.txt"], "");
        env.fs.add_file("file1.txt", "a\nb\nc\n");
        env.fs.add_file("file2.txt", "b\nc\nd\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert_eq!("\tb\n\tc\nd\n", stdout);
    }

    #[test]
    fn suppress_col2() {
        let env = make_test_env_with_stdin(vec!["comm", "-2", "file1.txt", "file2.txt"], "");
        env.fs.add_file("file1.txt", "a\nb\nc\n");
        env.fs.add_file("file2.txt", "b\nc\nd\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert_eq!("a\n\tb\n\tc\n", stdout);
    }

    #[test]
    fn suppress_col3() {
        let env = make_test_env_with_stdin(vec!["comm", "-3", "file1.txt", "file2.txt"], "");
        env.fs.add_file("file1.txt", "a\nb\nc\n");
        env.fs.add_file("file2.txt", "b\nc\nd\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert_eq!("a\n\td\n", stdout);
    }

    #[test]
    fn suppress_col1_col2() {
        let env = make_test_env_with_stdin(vec!["comm", "-12", "file1.txt", "file2.txt"], "");
        env.fs.add_file("file1.txt", "a\nb\nc\n");
        env.fs.add_file("file2.txt", "b\nc\nd\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert_eq!("b\nc\n", stdout);
    }

    #[test]
    fn suppress_col1_col3() {
        let env = make_test_env_with_stdin(vec!["comm", "-13", "file1.txt", "file2.txt"], "");
        env.fs.add_file("file1.txt", "a\nb\nc\n");
        env.fs.add_file("file2.txt", "b\nc\nd\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert_eq!("d\n", stdout);
    }

    #[test]
    fn suppress_col2_col3() {
        let env = make_test_env_with_stdin(vec!["comm", "-23", "file1.txt", "file2.txt"], "");
        env.fs.add_file("file1.txt", "a\nb\nc\n");
        env.fs.add_file("file2.txt", "b\nc\nd\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert_eq!("a\n", stdout);
    }

    #[test]
    fn suppress_all_columns() {
        let env = make_test_env_with_stdin(vec!["comm", "-123", "file1.txt", "file2.txt"], "");
        env.fs.add_file("file1.txt", "a\nb\nc\n");
        env.fs.add_file("file2.txt", "b\nc\nd\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("", env.stdout.into_string());
    }

    // ========================================================================
    // Case insensitive tests (-i)
    // ========================================================================

    #[test]
    fn case_insensitive() {
        let env = make_test_env_with_stdin(vec!["comm", "-i", "file1.txt", "file2.txt"], "");
        env.fs.add_file("file1.txt", "A\nb\nC\n");
        env.fs.add_file("file2.txt", "a\nB\nc\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert_eq!("\t\tA\n\t\tb\n\t\tC\n", stdout);
    }

    #[test]
    fn case_insensitive_with_suppress() {
        let env = make_test_env_with_stdin(vec!["comm", "-i", "-12", "file1.txt", "file2.txt"], "");
        env.fs.add_file("file1.txt", "A\nb\nC\n");
        env.fs.add_file("file2.txt", "a\nB\nc\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert_eq!("A\nb\nC\n", stdout);
    }

    // ========================================================================
    // Stdin tests
    // ========================================================================

    #[test]
    fn stdin_as_file1() {
        let env = make_test_env_with_stdin(vec!["comm", "-", "file2.txt"], "a\nb\nc\n");
        env.fs.add_file("file2.txt", "b\nc\nd\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert_eq!("a\n\t\tb\n\t\tc\n\td\n", stdout);
    }

    #[test]
    fn stdin_as_file2() {
        let env = make_test_env_with_stdin(vec!["comm", "file1.txt", "-"], "b\nc\nd\n");
        env.fs.add_file("file1.txt", "a\nb\nc\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert_eq!("a\n\t\tb\n\t\tc\n\td\n", stdout);
    }

    // ========================================================================
    // Error handling tests
    // ========================================================================

    #[test]
    fn file1_not_found() {
        let env = make_test_env_with_stdin(vec!["comm", "nonexistent.txt", "file2.txt"], "");
        env.fs.add_file("file2.txt", "a\n");
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("nonexistent.txt"));
    }

    #[test]
    fn file2_not_found() {
        let env = make_test_env_with_stdin(vec!["comm", "file1.txt", "nonexistent.txt"], "");
        env.fs.add_file("file1.txt", "a\n");
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("nonexistent.txt"));
    }

    #[test]
    fn missing_file_args() {
        let env = make_test_env_with_stdin(vec!["comm"], "");
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("usage"));
    }

    #[test]
    fn only_one_file_arg() {
        let env = make_test_env_with_stdin(vec!["comm", "file1.txt"], "");
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("usage"));
    }

    // ========================================================================
    // Regression tests from FreeBSD
    // ========================================================================

    #[test]
    fn regression_00() {
        let env = make_test_env_with_stdin(vec!["comm", "-12", "file1.txt", "file2.txt"], "");
        env.fs.add_file("file1.txt", "a b\nc d\ne f\ne f g\nh i\n");
        env.fs.add_file("file2.txt", "a b\ne f g\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert_eq!("a b\ne f g\n", stdout);
    }

    #[test]
    fn regression_01() {
        let env = make_test_env_with_stdin(vec!["comm", "-12", "file1.txt", "file2.txt"], "");
        env.fs
            .add_file("file1.txt", "a\tb\nc\td\ne\tf\ne\tf\tg\nh\ti\n");
        env.fs.add_file("file2.txt", "a\tb\ne\tf\tg\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert_eq!("a\tb\ne\tf\tg\n", stdout);
    }

    #[test]
    fn regression_02() {
        let env = make_test_env_with_stdin(vec!["comm", "file1.txt", "file2.txt"], "");
        env.fs.add_file("file1.txt", "a\nb\nc");
        env.fs.add_file("file2.txt", "c\nd\ne");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert_eq!("a\nb\n\t\tc\n\td\n\te\n", stdout);
    }

    // ========================================================================
    // Edge cases
    // ========================================================================

    #[test]
    fn no_trailing_newline_file1() {
        let env = make_test_env_with_stdin(vec!["comm", "file1.txt", "file2.txt"], "");
        env.fs.add_file("file1.txt", "a\nb");
        env.fs.add_file("file2.txt", "b\nc\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert_eq!("a\n\t\tb\n\tc\n", stdout);
    }

    #[test]
    fn no_trailing_newline_file2() {
        let env = make_test_env_with_stdin(vec!["comm", "file1.txt", "file2.txt"], "");
        env.fs.add_file("file1.txt", "a\nb\n");
        env.fs.add_file("file2.txt", "b\nc");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert_eq!("a\n\t\tb\n\tc\n", stdout);
    }

    #[test]
    fn single_line_files() {
        let env = make_test_env_with_stdin(vec!["comm", "file1.txt", "file2.txt"], "");
        env.fs.add_file("file1.txt", "hello\n");
        env.fs.add_file("file2.txt", "hello\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert_eq!("\t\thello\n", stdout);
    }

    #[test]
    fn whitespace_lines() {
        let env = make_test_env_with_stdin(vec!["comm", "file1.txt", "file2.txt"], "");
        env.fs.add_file("file1.txt", " \n  \n");
        env.fs.add_file("file2.txt", " \n   \n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        // " " is common (col3), "  " only in file1 (col1), "   " only in file2 (col2)
        println!("stdout: {:?}", stdout);
        assert_eq!("\t\t \n  \n\t   \n", stdout);
    }
}

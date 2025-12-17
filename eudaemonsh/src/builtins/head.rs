//! The head builtin: display first lines of a file.

use getopts::Options;

use crate::{Environment, Error, ExitCode, Filesystem, FsError, Stderr, Stdin, Stdout, parse_size};

/// Options for the head command.
#[derive(Clone, Copy, Debug, Default)]
struct HeadOptions {
    /// Number of lines to display (default 10).
    lines: Option<u64>,
    /// Number of bytes to display.
    bytes: Option<u64>,
    /// Suppress headers when multiple files are specified.
    quiet: bool,
    /// Always print headers.
    verbose: bool,
}

fn build_options() -> Options {
    let mut opts = Options::new();
    opts.optopt(
        "n",
        "lines",
        "Print count lines of each of the specified files.",
        "count",
    );
    opts.optopt(
        "c",
        "bytes",
        "Print bytes of each of the specified files.",
        "bytes",
    );
    opts.optflag(
        "q",
        "quiet",
        "Suppresses printing of headers when multiple files are being examined.",
    );
    opts.optflag("", "silent", "Same as -q.");
    opts.optflag("v", "verbose", "Prepend each file with a header.");
    opts
}

/// Preprocess arguments to convert legacy -NUM syntax to -n NUM.
/// BSD and GNU head support `-NUM` as shorthand for `-n NUM`.
///
/// This function handles a subtle case: when `-n` or `-c` is followed by `-NUM`,
/// the `-NUM` should be passed through as-is (the value for that flag), not converted
/// to `-n NUM`.
fn preprocess_args(args: &[String]) -> Vec<String> {
    let mut result = Vec::new();
    let mut expecting_value = false;

    for arg in args {
        if expecting_value {
            // The previous arg was -n or -c, so this arg is its value.
            // Pass it through without transformation.
            result.push(arg.clone());
            expecting_value = false;
            continue;
        }

        // Check if this arg is -n or -c (flags that expect a value)
        if arg == "-n" || arg == "-c" {
            result.push(arg.clone());
            expecting_value = true;
            continue;
        }

        if let Some(rest) = arg.strip_prefix('-') {
            // Check if the rest is a valid positive integer (legacy -NUM syntax)
            if !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit()) {
                result.push("-n".to_string());
                result.push(rest.to_string());
                continue;
            }
        }
        result.push(arg.clone());
    }
    result
}

/// The head builtin: display first lines of a file.
pub fn bin<SI, SO, SE, FS>(env: &Environment<SI, SO, SE, FS>) -> Result<ExitCode, Error>
where
    SI: Stdin,
    SO: Stdout,
    SE: Stderr,
    FS: Filesystem,
{
    let opts_def = build_options();

    // Preprocess arguments to support legacy -NUM syntax
    let preprocessed_args = preprocess_args(&env.args[1..]);
    let matches = match opts_def.parse(&preprocessed_args) {
        Ok(m) => m,
        Err(e) => {
            env.stderr.write_line(&format!("head: {}", e))?;
            return Ok(ExitCode::from(1));
        }
    };

    let quiet = matches.opt_present("q") || matches.opt_present("silent");
    let verbose = matches.opt_present("v") && !quiet;

    let lines = if let Some(n) = matches.opt_str("n") {
        match parse_size(&n) {
            Some(count) if count > 0 => Some(count),
            _ => {
                env.stderr
                    .write_line(&format!("head: illegal line count -- {}", n))?;
                return Ok(ExitCode::from(1));
            }
        }
    } else {
        None
    };

    let bytes = if let Some(b) = matches.opt_str("c") {
        match parse_size(&b) {
            Some(count) if count > 0 => Some(count),
            _ => {
                env.stderr
                    .write_line(&format!("head: illegal byte count -- {}", b))?;
                return Ok(ExitCode::from(1));
            }
        }
    } else {
        None
    };

    if lines.is_some() && bytes.is_some() {
        env.stderr
            .write_line("head: can't combine line and byte counts")?;
        return Ok(ExitCode::from(1));
    }

    let head_opts = HeadOptions {
        lines,
        bytes,
        quiet,
        verbose,
    };

    let mut exit_code: i8 = 0;
    let files = &matches.free;

    if files.is_empty() {
        head_stdin(env, &head_opts)?;
    } else {
        let print_headers = head_opts.verbose || (!head_opts.quiet && files.len() > 1);
        let mut first = true;

        for file in files {
            if file == "-" {
                if print_headers {
                    if !first {
                        env.stdout.write_str("\n")?;
                    }
                    env.stdout.write_line("==> standard input <==")?;
                }
                head_stdin(env, &head_opts)?;
            } else {
                if print_headers {
                    if !first {
                        env.stdout.write_str("\n")?;
                    }
                    env.stdout.write_line(&format!("==> {} <==", file))?;
                }
                if let Err(code) = head_file(env, file, &head_opts) {
                    exit_code = code;
                }
            }
            first = false;
        }
    }

    Ok(ExitCode::from(exit_code))
}

fn head_stdin<SI, SO, SE, FS>(
    env: &Environment<SI, SO, SE, FS>,
    opts: &HeadOptions,
) -> Result<(), Error>
where
    SI: Stdin,
    SO: Stdout,
    SE: Stderr,
    FS: Filesystem,
{
    let mut contents = String::new();
    while let Some(line) = env.stdin.read_line()? {
        contents.push_str(&line);
        contents.push('\n');
    }
    head_string(env, &contents, opts)
}

fn head_file<SI, SO, SE, FS>(
    env: &Environment<SI, SO, SE, FS>,
    path: &str,
    opts: &HeadOptions,
) -> Result<(), i8>
where
    SI: Stdin,
    SO: Stdout,
    SE: Stderr,
    FS: Filesystem,
{
    match env.fs.read_to_string(path) {
        Ok(contents) => head_string(env, &contents, opts).map_err(|_| 1i8),
        Err(FsError::Io(e)) => {
            let _ = env.stderr.write_line(&format!("head: {}: {}", path, e));
            Err(1)
        }
    }
}

fn head_string<SI, SO, SE, FS>(
    env: &Environment<SI, SO, SE, FS>,
    contents: &str,
    opts: &HeadOptions,
) -> Result<(), Error>
where
    SI: Stdin,
    SO: Stdout,
    SE: Stderr,
    FS: Filesystem,
{
    if let Some(bytes) = opts.bytes {
        head_string_bytes(env, contents, bytes)
    } else {
        let lines = opts.lines.unwrap_or(10);
        head_string_lines(env, contents, lines)
    }
}

fn head_string_lines<SI, SO, SE, FS>(
    env: &Environment<SI, SO, SE, FS>,
    contents: &str,
    count: u64,
) -> Result<(), Error>
where
    SI: Stdin,
    SO: Stdout,
    SE: Stderr,
    FS: Filesystem,
{
    let mut remaining = count as usize;
    for line in contents.lines() {
        if remaining == 0 {
            break;
        }
        env.stdout.write_line(line)?;
        remaining -= 1;
    }
    Ok(())
}

fn head_string_bytes<SI, SO, SE, FS>(
    env: &Environment<SI, SO, SE, FS>,
    contents: &str,
    count: u64,
) -> Result<(), Error>
where
    SI: Stdin,
    SO: Stdout,
    SE: Stderr,
    FS: Filesystem,
{
    let count = count as usize;
    let bytes = contents.as_bytes();
    let end = std::cmp::min(count, bytes.len());
    let output = std::str::from_utf8(&bytes[..end]).unwrap_or("");
    Ok(env.stdout.write_str(output)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TestFilesystemExt;
    use crate::parse_size;
    use crate::test_utils::make_test_env_with_stdin;

    // ========================================================================
    // Basic functionality tests
    // ========================================================================

    #[test]
    fn empty_stdin() {
        let env = make_test_env_with_stdin(vec!["head"], "");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("", env.stdout.into_string());
    }

    #[test]
    fn single_line() {
        let env = make_test_env_with_stdin(vec!["head"], "hello");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("hello\n", env.stdout.into_string());
    }

    #[test]
    fn default_ten_lines() {
        let input = "1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n11\n12";
        let env = make_test_env_with_stdin(vec!["head"], input);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let expected = "1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n";
        assert_eq!(expected, env.stdout.into_string());
    }

    #[test]
    fn fewer_than_ten_lines() {
        let input = "one\ntwo\nthree";
        let env = make_test_env_with_stdin(vec!["head"], input);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("one\ntwo\nthree\n", env.stdout.into_string());
    }

    // ========================================================================
    // -n flag: specify number of lines
    // ========================================================================

    #[test]
    fn n_flag_five_lines() {
        let input = "1\n2\n3\n4\n5\n6\n7\n8\n9\n10";
        let env = make_test_env_with_stdin(vec!["head", "-n", "5"], input);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("1\n2\n3\n4\n5\n", env.stdout.into_string());
    }

    #[test]
    fn n_flag_one_line() {
        let input = "first\nsecond\nthird";
        let env = make_test_env_with_stdin(vec!["head", "-n", "1"], input);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("first\n", env.stdout.into_string());
    }

    #[test]
    fn n_flag_more_than_available() {
        let input = "one\ntwo\nthree";
        let env = make_test_env_with_stdin(vec!["head", "-n", "100"], input);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("one\ntwo\nthree\n", env.stdout.into_string());
    }

    #[test]
    fn n_flag_long_form() {
        let input = "1\n2\n3\n4\n5";
        let env = make_test_env_with_stdin(vec!["head", "--lines=3"], input);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("1\n2\n3\n", env.stdout.into_string());
    }

    #[test]
    fn n_flag_zero_is_error() {
        let env = make_test_env_with_stdin(vec!["head", "-n", "0"], "hello");
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("illegal line count"));
    }

    #[test]
    fn n_flag_negative_is_error() {
        let env = make_test_env_with_stdin(vec!["head", "-n", "-5"], "hello");
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("illegal line count"));
    }

    #[test]
    fn n_flag_non_numeric_is_error() {
        let env = make_test_env_with_stdin(vec!["head", "-n", "abc"], "hello");
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("illegal line count"));
    }

    // ========================================================================
    // -c flag: specify number of bytes
    // ========================================================================

    #[test]
    fn c_flag_five_bytes() {
        let env = make_test_env_with_stdin(vec!["head", "-c", "5"], "hello world");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("hello", env.stdout.into_string());
    }

    #[test]
    fn c_flag_from_file() {
        let env = make_test_env_with_stdin(vec!["head", "-c", "5", "file.txt"], "");
        env.fs.add_file("file.txt", "hello world\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("hello", env.stdout.into_string());
    }

    #[test]
    fn c_flag_more_than_available() {
        let env = make_test_env_with_stdin(vec!["head", "-c", "100", "file.txt"], "");
        env.fs.add_file("file.txt", "short");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("short", env.stdout.into_string());
    }

    #[test]
    fn c_flag_long_form() {
        let env = make_test_env_with_stdin(vec!["head", "--bytes=3", "file.txt"], "");
        env.fs.add_file("file.txt", "hello");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("hel", env.stdout.into_string());
    }

    #[test]
    fn c_flag_zero_is_error() {
        let env = make_test_env_with_stdin(vec!["head", "-c", "0"], "hello");
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("illegal byte count"));
    }

    #[test]
    fn c_and_n_together_is_error() {
        let env = make_test_env_with_stdin(vec!["head", "-c", "5", "-n", "3"], "hello");
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("can't combine"));
    }

    // ========================================================================
    // Size suffixes
    // ========================================================================

    #[test]
    fn size_suffix_k() {
        assert_eq!(Some(1024), parse_size("1k"));
        assert_eq!(Some(2048), parse_size("2K"));
    }

    #[test]
    fn size_suffix_m() {
        assert_eq!(Some(1024 * 1024), parse_size("1m"));
        assert_eq!(Some(2 * 1024 * 1024), parse_size("2M"));
    }

    #[test]
    fn size_suffix_g() {
        assert_eq!(Some(1024 * 1024 * 1024), parse_size("1g"));
    }

    #[test]
    fn size_suffix_b() {
        assert_eq!(Some(512), parse_size("1b"));
        assert_eq!(Some(1024), parse_size("2b"));
    }

    #[test]
    fn size_no_suffix() {
        assert_eq!(Some(42), parse_size("42"));
        assert_eq!(Some(1000), parse_size("1000"));
    }

    #[test]
    fn size_invalid() {
        assert_eq!(None, parse_size(""));
        assert_eq!(None, parse_size("abc"));
        assert_eq!(None, parse_size("1x"));
    }

    // ========================================================================
    // File operations
    // ========================================================================

    #[test]
    fn single_file() {
        let env = make_test_env_with_stdin(vec!["head", "file.txt"], "");
        env.fs.add_file("file.txt", "line1\nline2\nline3\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("line1\nline2\nline3\n", env.stdout.into_string());
    }

    #[test]
    fn file_not_found() {
        let env = make_test_env_with_stdin(vec!["head", "nonexistent.txt"], "");
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("nonexistent.txt"));
    }

    #[test]
    fn multiple_files_with_headers() {
        let env = make_test_env_with_stdin(vec!["head", "a.txt", "b.txt"], "");
        env.fs.add_file("a.txt", "aaa\n");
        env.fs.add_file("b.txt", "bbb\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert!(stdout.contains("==> a.txt <=="));
        assert!(stdout.contains("==> b.txt <=="));
        assert!(stdout.contains("aaa"));
        assert!(stdout.contains("bbb"));
    }

    #[test]
    fn single_file_no_header() {
        let env = make_test_env_with_stdin(vec!["head", "file.txt"], "");
        env.fs.add_file("file.txt", "content\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert!(!stdout.contains("==>"));
        assert_eq!("content\n", stdout);
    }

    // ========================================================================
    // -q flag: suppress headers
    // ========================================================================

    #[test]
    fn q_flag_suppresses_headers() {
        let env = make_test_env_with_stdin(vec!["head", "-q", "a.txt", "b.txt"], "");
        env.fs.add_file("a.txt", "aaa\n");
        env.fs.add_file("b.txt", "bbb\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert!(!stdout.contains("==>"));
        assert!(stdout.contains("aaa"));
        assert!(stdout.contains("bbb"));
    }

    #[test]
    fn quiet_long_form() {
        let env = make_test_env_with_stdin(vec!["head", "--quiet", "a.txt", "b.txt"], "");
        env.fs.add_file("a.txt", "aaa\n");
        env.fs.add_file("b.txt", "bbb\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert!(!stdout.contains("==>"));
    }

    #[test]
    fn silent_long_form() {
        let env = make_test_env_with_stdin(vec!["head", "--silent", "a.txt", "b.txt"], "");
        env.fs.add_file("a.txt", "aaa\n");
        env.fs.add_file("b.txt", "bbb\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert!(!stdout.contains("==>"));
    }

    // ========================================================================
    // -v flag: always print headers
    // ========================================================================

    #[test]
    fn v_flag_forces_header() {
        let env = make_test_env_with_stdin(vec!["head", "-v", "file.txt"], "");
        env.fs.add_file("file.txt", "content\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert!(stdout.contains("==> file.txt <=="));
        assert!(stdout.contains("content"));
    }

    #[test]
    fn verbose_long_form() {
        let env = make_test_env_with_stdin(vec!["head", "--verbose", "file.txt"], "");
        env.fs.add_file("file.txt", "content\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert!(stdout.contains("==> file.txt <=="));
    }

    #[test]
    fn q_overrides_v() {
        let env = make_test_env_with_stdin(vec!["head", "-v", "-q", "a.txt", "b.txt"], "");
        env.fs.add_file("a.txt", "aaa\n");
        env.fs.add_file("b.txt", "bbb\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert!(!stdout.contains("==>"));
    }

    // ========================================================================
    // Stdin with dash
    // ========================================================================

    #[test]
    fn dash_means_stdin() {
        let env = make_test_env_with_stdin(vec!["head", "-"], "from stdin");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("from stdin\n", env.stdout.into_string());
    }

    #[test]
    fn dash_with_file() {
        let env = make_test_env_with_stdin(vec!["head", "a.txt", "-", "b.txt"], "stdin content");
        env.fs.add_file("a.txt", "aaa\n");
        env.fs.add_file("b.txt", "bbb\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert!(stdout.contains("==> a.txt <=="));
        assert!(stdout.contains("==> standard input <=="));
        assert!(stdout.contains("==> b.txt <=="));
        assert!(stdout.contains("aaa"));
        assert!(stdout.contains("stdin content"));
        assert!(stdout.contains("bbb"));
    }

    // ========================================================================
    // Edge cases
    // ========================================================================

    #[test]
    fn empty_file() {
        let env = make_test_env_with_stdin(vec!["head", "empty.txt"], "");
        env.fs.add_file("empty.txt", "");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("", env.stdout.into_string());
    }

    #[test]
    fn file_no_trailing_newline() {
        let env = make_test_env_with_stdin(vec!["head", "file.txt"], "");
        env.fs.add_file("file.txt", "no newline");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("no newline\n", env.stdout.into_string());
    }

    #[test]
    fn multiple_files_one_missing() {
        let env = make_test_env_with_stdin(vec!["head", "a.txt", "missing.txt", "b.txt"], "");
        env.fs.add_file("a.txt", "aaa\n");
        env.fs.add_file("b.txt", "bbb\n");
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stdout = env.stdout.into_string();
        let stderr = env.stderr.into_string();
        println!("stdout: {:?}", stdout);
        println!("stderr: {:?}", stderr);
        assert!(stdout.contains("aaa"));
        assert!(stdout.contains("bbb"));
        assert!(stderr.contains("missing.txt"));
    }

    #[test]
    fn illegal_option() {
        let env = make_test_env_with_stdin(vec!["head", "-x"], "");
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("Unrecognized option"));
    }

    // ========================================================================
    // Legacy -NUM syntax tests
    // ========================================================================

    #[test]
    fn legacy_dash_number_syntax() {
        let input = "1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n11\n12";
        let env = make_test_env_with_stdin(vec!["head", "-5"], input);
        let result = bin(&env).unwrap();
        println!("stdout: {:?}", env.stdout.clone().into_string());
        println!("stderr: {:?}", env.stderr.clone().into_string());
        assert_eq!(0, result.code());
        assert_eq!("1\n2\n3\n4\n5\n", env.stdout.into_string());
    }

    #[test]
    fn legacy_dash_twenty() {
        let input: String = (1..=25).map(|i| format!("{}\n", i)).collect();
        let env = make_test_env_with_stdin(vec!["head", "-20"], &input);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let expected: String = (1..=20).map(|i| format!("{}\n", i)).collect();
        assert_eq!(expected, env.stdout.into_string());
    }

    #[test]
    fn legacy_dash_one() {
        let input = "first\nsecond\nthird";
        let env = make_test_env_with_stdin(vec!["head", "-1"], input);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("first\n", env.stdout.into_string());
    }

    #[test]
    fn legacy_dash_number_with_file() {
        let env = make_test_env_with_stdin(vec!["head", "-3", "file.txt"], "");
        env.fs.add_file("file.txt", "a\nb\nc\nd\ne\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("a\nb\nc\n", env.stdout.into_string());
    }

    // ========================================================================
    // Header format tests
    // ========================================================================

    #[test]
    fn header_format_with_newline_between() {
        let env = make_test_env_with_stdin(vec!["head", "-n", "1", "a.txt", "b.txt"], "");
        env.fs.add_file("a.txt", "aaa\n");
        env.fs.add_file("b.txt", "bbb\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        let expected = "==> a.txt <==\naaa\n\n==> b.txt <==\nbbb\n";
        assert_eq!(expected, stdout);
    }

    // ========================================================================
    // Combined options tests
    // ========================================================================

    #[test]
    fn combined_nq() {
        let env = make_test_env_with_stdin(vec!["head", "-n", "2", "-q", "a.txt", "b.txt"], "");
        env.fs.add_file("a.txt", "a1\na2\na3\n");
        env.fs.add_file("b.txt", "b1\nb2\nb3\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert!(!stdout.contains("==>"));
        assert!(stdout.contains("a1\na2\n"));
        assert!(stdout.contains("b1\nb2\n"));
    }

    #[test]
    fn combined_cv() {
        let env = make_test_env_with_stdin(vec!["head", "-c", "3", "-v", "file.txt"], "");
        env.fs.add_file("file.txt", "hello");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert!(stdout.contains("==> file.txt <=="));
        assert!(stdout.contains("hel"));
    }
}

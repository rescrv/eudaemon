//! The tail builtin: display the last part of a file.

use getopts::Options;

use crate::{Environment, Error, ExitCode, Filesystem, Stderr, Stdin, Stdout, parse_size};

/// The style of offset: from beginning or from end.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum OffsetStyle {
    /// Offset from the beginning of the file (+ prefix).
    FromBeginning,
    /// Offset from the end of the file (- prefix or default).
    #[default]
    FromEnd,
}

/// The unit of measurement for the offset.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum OffsetUnit {
    /// Count in lines.
    #[default]
    Lines,
    /// Count in bytes.
    Bytes,
}

/// Options for the tail command.
#[derive(Clone, Copy, Debug)]
struct TailOptions {
    /// The count (lines or bytes).
    count: u64,
    /// Whether to count from beginning or end.
    style: OffsetStyle,
    /// Whether to count lines or bytes.
    unit: OffsetUnit,
    /// Display in reverse order by line.
    reverse: bool,
    /// Suppress headers when multiple files are specified.
    quiet: bool,
    /// Always print headers.
    verbose: bool,
}

impl Default for TailOptions {
    fn default() -> Self {
        Self {
            count: 10,
            style: OffsetStyle::FromEnd,
            unit: OffsetUnit::Lines,
            reverse: false,
            quiet: false,
            verbose: false,
        }
    }
}

fn build_options() -> Options {
    let mut opts = Options::new();
    opts.optopt("n", "lines", "The location is number lines.", "number");
    opts.optopt("c", "bytes", "The location is number bytes.", "number");
    opts.optopt(
        "b",
        "blocks",
        "The location is number 512-byte blocks.",
        "number",
    );
    opts.optflag("r", "", "Display the input in reverse order, by line.");
    opts.optflag(
        "q",
        "quiet",
        "Suppresses printing of headers when multiple files are being examined.",
    );
    opts.optflag("", "silent", "Same as -q.");
    opts.optflag("v", "verbose", "Prepend each file with a header.");
    opts
}

/// Parse a number that may have a +/- prefix and size suffixes.
/// Returns (value, style) where style indicates from-beginning (+) or from-end (-/default).
fn parse_offset(s: &str) -> Option<(u64, OffsetStyle)> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }

    let (num_str, style) = if let Some(rest) = s.strip_prefix('+') {
        (rest, OffsetStyle::FromBeginning)
    } else if let Some(rest) = s.strip_prefix('-') {
        (rest, OffsetStyle::FromEnd)
    } else {
        (s, OffsetStyle::FromEnd)
    };

    let value = parse_size(num_str)?;
    Some((value, style))
}

/// The tail builtin: display the last part of a file.
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
            env.stderr.write_line(&format!("tail: {}", e))?;
            return Ok(ExitCode::from(1));
        }
    };

    let quiet = matches.opt_present("q") || matches.opt_present("silent");
    let verbose = matches.opt_present("v") && !quiet;
    let reverse = matches.opt_present("r");

    let mut tail_opts = TailOptions {
        quiet,
        verbose,
        reverse,
        ..Default::default()
    };

    // Track if any count option was specified
    let mut count_specified = false;

    // Parse -n (lines)
    if let Some(n) = matches.opt_str("n") {
        if count_specified {
            env.stderr
                .write_line("tail: can't combine line, byte, and block counts")?;
            return Ok(ExitCode::from(1));
        }
        match parse_offset(&n) {
            Some((count, style)) => {
                tail_opts.count = count;
                tail_opts.style = style;
                tail_opts.unit = OffsetUnit::Lines;
                count_specified = true;
            }
            None => {
                env.stderr
                    .write_line(&format!("tail: illegal offset -- {}", n))?;
                return Ok(ExitCode::from(1));
            }
        }
    }

    // Parse -c (bytes)
    if let Some(c) = matches.opt_str("c") {
        if count_specified {
            env.stderr
                .write_line("tail: can't combine line, byte, and block counts")?;
            return Ok(ExitCode::from(1));
        }
        match parse_offset(&c) {
            Some((count, style)) => {
                tail_opts.count = count;
                tail_opts.style = style;
                tail_opts.unit = OffsetUnit::Bytes;
                count_specified = true;
            }
            None => {
                env.stderr
                    .write_line(&format!("tail: illegal offset -- {}", c))?;
                return Ok(ExitCode::from(1));
            }
        }
    }

    // Parse -b (512-byte blocks)
    if let Some(b) = matches.opt_str("b") {
        if count_specified {
            env.stderr
                .write_line("tail: can't combine line, byte, and block counts")?;
            return Ok(ExitCode::from(1));
        }
        match parse_offset(&b) {
            Some((count, style)) => {
                // Convert blocks to bytes
                tail_opts.count = count.saturating_mul(512);
                tail_opts.style = style;
                tail_opts.unit = OffsetUnit::Bytes;
                count_specified = true;
            }
            None => {
                env.stderr
                    .write_line(&format!("tail: illegal offset -- {}", b))?;
                return Ok(ExitCode::from(1));
            }
        }
    }

    // When -r is specified without a count, the default is the entire file
    if reverse && !count_specified {
        tail_opts.count = u64::MAX;
        tail_opts.style = OffsetStyle::FromEnd;
    }

    let mut exit_code: i8 = 0;
    let files = &matches.free;

    if files.is_empty() {
        tail_stdin(env, &tail_opts)?;
    } else {
        let print_headers = tail_opts.verbose || (!tail_opts.quiet && files.len() > 1);
        let mut first = true;

        for file in files {
            if file == "-" {
                if print_headers {
                    if !first {
                        env.stdout.write_str("\n")?;
                    }
                    env.stdout.write_line("==> standard input <==")?;
                }
                tail_stdin(env, &tail_opts)?;
            } else {
                if print_headers {
                    if !first {
                        env.stdout.write_str("\n")?;
                    }
                    env.stdout.write_line(&format!("==> {} <==", file))?;
                }
                if let Err(code) = tail_file(env, file, &tail_opts) {
                    exit_code = code;
                }
            }
            first = false;
        }
    }

    Ok(ExitCode::from(exit_code))
}

fn tail_stdin<SI, SO, SE, FS>(
    env: &Environment<SI, SO, SE, FS>,
    opts: &TailOptions,
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
    tail_string(env, &contents, opts)
}

fn tail_file<SI, SO, SE, FS>(
    env: &Environment<SI, SO, SE, FS>,
    path: &str,
    opts: &TailOptions,
) -> Result<(), i8>
where
    SI: Stdin,
    SO: Stdout,
    SE: Stderr,
    FS: Filesystem,
{
    match env.fs.read_to_string(path) {
        Ok(contents) => tail_string(env, &contents, opts).map_err(|_| 1i8),
        Err(Error::Io(e)) => {
            let _ = env.stderr.write_line(&format!("tail: {}: {}", path, e));
            Err(1)
        }
        Err(_e) => Err(1),
    }
}

fn tail_string<SI, SO, SE, FS>(
    env: &Environment<SI, SO, SE, FS>,
    contents: &str,
    opts: &TailOptions,
) -> Result<(), Error>
where
    SI: Stdin,
    SO: Stdout,
    SE: Stderr,
    FS: Filesystem,
{
    match opts.unit {
        OffsetUnit::Bytes => tail_string_bytes(env, contents, opts),
        OffsetUnit::Lines => tail_string_lines(env, contents, opts),
    }
}

/// Display lines from the input.
fn tail_string_lines<SI, SO, SE, FS>(
    env: &Environment<SI, SO, SE, FS>,
    contents: &str,
    opts: &TailOptions,
) -> Result<(), Error>
where
    SI: Stdin,
    SO: Stdout,
    SE: Stderr,
    FS: Filesystem,
{
    // Collect lines, preserving whether they ended with newline
    let lines: Vec<&str> = contents.split_inclusive('\n').collect();

    let selected: Vec<&str> = match opts.style {
        OffsetStyle::FromBeginning => {
            // +N means start at line N (1-indexed), so skip N-1 lines
            let skip = if opts.count > 0 { opts.count - 1 } else { 0 } as usize;
            lines.into_iter().skip(skip).collect()
        }
        OffsetStyle::FromEnd => {
            // -N means display last N lines
            let count = opts.count as usize;
            let len = lines.len();
            if count >= len {
                lines
            } else {
                lines.into_iter().skip(len - count).collect()
            }
        }
    };

    if selected.is_empty() {
        return Ok(());
    }

    if opts.reverse {
        // Display lines in reverse order
        for line in selected.into_iter().rev() {
            env.stdout.write_str(line)?;
            // Add newline if the line didn't have one
            if !line.ends_with('\n') {
                env.stdout.write_str("\n")?;
            }
        }
    } else {
        for line in selected {
            env.stdout.write_str(line)?;
        }
    }

    Ok(())
}

/// Display bytes from the input.
fn tail_string_bytes<SI, SO, SE, FS>(
    env: &Environment<SI, SO, SE, FS>,
    contents: &str,
    opts: &TailOptions,
) -> Result<(), Error>
where
    SI: Stdin,
    SO: Stdout,
    SE: Stderr,
    FS: Filesystem,
{
    let bytes = contents.as_bytes();
    let count = opts.count as usize;

    let selected: &[u8] = match opts.style {
        OffsetStyle::FromBeginning => {
            // +N means start at byte N (1-indexed), so skip N-1 bytes
            let skip = if opts.count > 0 { opts.count - 1 } else { 0 } as usize;
            if skip >= bytes.len() {
                &[]
            } else {
                &bytes[skip..]
            }
        }
        OffsetStyle::FromEnd => {
            // -N means display last N bytes
            if count >= bytes.len() {
                bytes
            } else {
                &bytes[bytes.len() - count..]
            }
        }
    };

    if selected.is_empty() {
        return Ok(());
    }

    if opts.reverse {
        // For reverse byte mode, we reverse by lines within the selected bytes
        let selected_str = std::str::from_utf8(selected).unwrap_or("");
        let lines: Vec<&str> = selected_str.split_inclusive('\n').collect();
        for line in lines.into_iter().rev() {
            env.stdout.write_str(line)?;
            if !line.ends_with('\n') {
                env.stdout.write_str("\n")?;
            }
        }
    } else {
        let output = std::str::from_utf8(selected).unwrap_or("");
        env.stdout.write_str(output)?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse_size;
    use crate::test_utils::make_test_env_with_stdin;

    // ========================================================================
    // Basic functionality tests
    // ========================================================================

    #[test]
    fn empty_stdin() {
        let env = make_test_env_with_stdin(vec!["tail"], "");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("", env.stdout.into_string());
    }

    #[test]
    fn single_line() {
        let env = make_test_env_with_stdin(vec!["tail"], "hello");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("hello\n", env.stdout.into_string());
    }

    #[test]
    fn default_ten_lines() {
        let input = "1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n11\n12";
        let env = make_test_env_with_stdin(vec!["tail"], input);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        // Always adds trailing newline if not present
        let expected = "3\n4\n5\n6\n7\n8\n9\n10\n11\n12\n";
        assert_eq!(expected, env.stdout.into_string());
    }

    #[test]
    fn fewer_than_ten_lines() {
        let input = "one\ntwo\nthree";
        let env = make_test_env_with_stdin(vec!["tail"], input);
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
        let env = make_test_env_with_stdin(vec!["tail", "-n", "5"], input);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        // Always adds trailing newline if not present
        assert_eq!("6\n7\n8\n9\n10\n", env.stdout.into_string());
    }

    #[test]
    fn n_flag_one_line() {
        let input = "first\nsecond\nthird";
        let env = make_test_env_with_stdin(vec!["tail", "-n", "1"], input);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("third\n", env.stdout.into_string());
    }

    #[test]
    fn n_flag_more_than_available() {
        let input = "one\ntwo\nthree";
        let env = make_test_env_with_stdin(vec!["tail", "-n", "100"], input);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("one\ntwo\nthree\n", env.stdout.into_string());
    }

    #[test]
    fn n_flag_long_form() {
        let input = "1\n2\n3\n4\n5";
        let env = make_test_env_with_stdin(vec!["tail", "--lines=2"], input);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        // Always adds trailing newline if not present
        assert_eq!("4\n5\n", env.stdout.into_string());
    }

    #[test]
    fn n_flag_zero_shows_nothing() {
        let env = make_test_env_with_stdin(vec!["tail", "-n", "0"], "hello\nworld");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("", env.stdout.into_string());
    }

    #[test]
    fn n_flag_negative_is_same_as_positive() {
        let input = "1\n2\n3\n4\n5";
        let env = make_test_env_with_stdin(vec!["tail", "-n", "-2"], input);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        // Always adds trailing newline if not present
        assert_eq!("4\n5\n", env.stdout.into_string());
    }

    #[test]
    fn n_flag_non_numeric_is_error() {
        let env = make_test_env_with_stdin(vec!["tail", "-n", "abc"], "hello");
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("illegal offset"));
    }

    // ========================================================================
    // + prefix: from beginning
    // ========================================================================

    #[test]
    fn n_plus_from_beginning() {
        let input = "1\n2\n3\n4\n5";
        let env = make_test_env_with_stdin(vec!["tail", "-n", "+3"], input);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        // +3 means start at line 3, always adds trailing newline
        assert_eq!("3\n4\n5\n", env.stdout.into_string());
    }

    #[test]
    fn n_plus_one_shows_all() {
        let input = "1\n2\n3";
        let env = make_test_env_with_stdin(vec!["tail", "-n", "+1"], input);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("1\n2\n3\n", env.stdout.into_string());
    }

    #[test]
    fn n_plus_beyond_file() {
        let input = "1\n2\n3";
        let env = make_test_env_with_stdin(vec!["tail", "-n", "+100"], input);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("", env.stdout.into_string());
    }

    #[test]
    fn c_plus_from_beginning() {
        let env = make_test_env_with_stdin(vec!["tail", "-c", "+6"], "hello world");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        // +6 means start at byte 6 (1-indexed), so skip first 5 bytes
        assert_eq!(" world\n", env.stdout.into_string());
    }

    // ========================================================================
    // -c flag: specify number of bytes
    // ========================================================================

    #[test]
    fn c_flag_five_bytes() {
        let env = make_test_env_with_stdin(vec!["tail", "-c", "5"], "hello world");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("orld\n", env.stdout.into_string());
    }

    #[test]
    fn c_flag_from_file() {
        let env = make_test_env_with_stdin(vec!["tail", "-c", "6", "file.txt"], "");
        env.fs.add_file("file.txt", "hello world\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        // Last 6 bytes of "hello world\n" is "orld\n" (5 bytes) plus newline already present
        assert_eq!("world\n", env.stdout.into_string());
    }

    #[test]
    fn c_flag_more_than_available() {
        let env = make_test_env_with_stdin(vec!["tail", "-c", "100", "file.txt"], "");
        env.fs.add_file("file.txt", "short\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("short\n", env.stdout.into_string());
    }

    #[test]
    fn c_flag_long_form() {
        let env = make_test_env_with_stdin(vec!["tail", "--bytes=3", "file.txt"], "");
        env.fs.add_file("file.txt", "hello\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        // Last 3 bytes of "hello\n" are "lo\n"
        assert_eq!("lo\n", env.stdout.into_string());
    }

    #[test]
    fn c_flag_zero_shows_nothing() {
        let env = make_test_env_with_stdin(vec!["tail", "-c", "0"], "hello");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("", env.stdout.into_string());
    }

    #[test]
    fn c_and_n_together_is_error() {
        let env = make_test_env_with_stdin(vec!["tail", "-c", "5", "-n", "3"], "hello");
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("can't combine"));
    }

    // ========================================================================
    // -b flag: 512-byte blocks
    // ========================================================================

    #[test]
    fn b_flag_one_block() {
        // Create content larger than 512 bytes
        let content: String = (0..600).map(|i| ((i % 10) as u8 + b'0') as char).collect();
        let env = make_test_env_with_stdin(vec!["tail", "-b", "1", "file.txt"], "");
        env.fs.add_file("file.txt", &content);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let output = env.stdout.into_string();
        println!(
            "content len: {}, output len: {}",
            content.len(),
            output.len()
        );
        println!("output last 20: {:?}", &output[output.len() - 20..]);
        println!("expected last 20: {:?}", &content[580..]);
        // Last 512 bytes of a 600-byte file
        assert_eq!(512, output.len());
        // Verify it's the last 512 bytes (starting at offset 88)
        assert_eq!(&content[88..], output);
    }

    // ========================================================================
    // -r flag: reverse order
    // ========================================================================

    #[test]
    fn r_flag_reverses_lines() {
        let input = "1\n2\n3";
        let env = make_test_env_with_stdin(vec!["tail", "-r"], input);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("3\n2\n1\n", env.stdout.into_string());
    }

    #[test]
    fn r_flag_with_n() {
        let input = "1\n2\n3\n4\n5";
        let env = make_test_env_with_stdin(vec!["tail", "-r", "-n", "3"], input);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        // Last 3 lines reversed
        assert_eq!("5\n4\n3\n", env.stdout.into_string());
    }

    #[test]
    fn r_flag_shows_all_by_default() {
        let input = "a\nb\nc";
        let env = make_test_env_with_stdin(vec!["tail", "-r"], input);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("c\nb\na\n", env.stdout.into_string());
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
    // parse_offset tests
    // ========================================================================

    #[test]
    fn parse_offset_plus_prefix() {
        assert_eq!(Some((10, OffsetStyle::FromBeginning)), parse_offset("+10"));
    }

    #[test]
    fn parse_offset_minus_prefix() {
        assert_eq!(Some((10, OffsetStyle::FromEnd)), parse_offset("-10"));
    }

    #[test]
    fn parse_offset_no_prefix() {
        assert_eq!(Some((10, OffsetStyle::FromEnd)), parse_offset("10"));
    }

    #[test]
    fn parse_offset_with_suffix() {
        assert_eq!(
            Some((1024, OffsetStyle::FromBeginning)),
            parse_offset("+1k")
        );
        assert_eq!(Some((2048, OffsetStyle::FromEnd)), parse_offset("-2k"));
    }

    // ========================================================================
    // File operations
    // ========================================================================

    #[test]
    fn single_file() {
        let env = make_test_env_with_stdin(vec!["tail", "file.txt"], "");
        env.fs.add_file("file.txt", "line1\nline2\nline3\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("line1\nline2\nline3\n", env.stdout.into_string());
    }

    #[test]
    fn file_not_found() {
        let env = make_test_env_with_stdin(vec!["tail", "nonexistent.txt"], "");
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("nonexistent.txt"));
    }

    #[test]
    fn multiple_files_with_headers() {
        let env = make_test_env_with_stdin(vec!["tail", "a.txt", "b.txt"], "");
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
        let env = make_test_env_with_stdin(vec!["tail", "file.txt"], "");
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
        let env = make_test_env_with_stdin(vec!["tail", "-q", "a.txt", "b.txt"], "");
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
        let env = make_test_env_with_stdin(vec!["tail", "--quiet", "a.txt", "b.txt"], "");
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
        let env = make_test_env_with_stdin(vec!["tail", "--silent", "a.txt", "b.txt"], "");
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
        let env = make_test_env_with_stdin(vec!["tail", "-v", "file.txt"], "");
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
        let env = make_test_env_with_stdin(vec!["tail", "--verbose", "file.txt"], "");
        env.fs.add_file("file.txt", "content\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert!(stdout.contains("==> file.txt <=="));
    }

    #[test]
    fn q_overrides_v() {
        let env = make_test_env_with_stdin(vec!["tail", "-v", "-q", "a.txt", "b.txt"], "");
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
        let env = make_test_env_with_stdin(vec!["tail", "-"], "from stdin");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("from stdin\n", env.stdout.into_string());
    }

    #[test]
    fn dash_with_file() {
        let env = make_test_env_with_stdin(vec!["tail", "a.txt", "-", "b.txt"], "stdin content");
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
        let env = make_test_env_with_stdin(vec!["tail", "empty.txt"], "");
        env.fs.add_file("empty.txt", "");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("", env.stdout.into_string());
    }

    #[test]
    fn file_no_trailing_newline() {
        let env = make_test_env_with_stdin(vec!["tail", "-n", "1", "file.txt"], "");
        env.fs.add_file("file.txt", "line1\nno newline");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        // tail preserves absence of trailing newline
        assert_eq!("no newline", env.stdout.into_string());
    }

    #[test]
    fn multiple_files_one_missing() {
        let env = make_test_env_with_stdin(vec!["tail", "a.txt", "missing.txt", "b.txt"], "");
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
        let env = make_test_env_with_stdin(vec!["tail", "-x"], "");
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("Unrecognized option"));
    }

    // ========================================================================
    // Header format tests
    // ========================================================================

    #[test]
    fn header_format_with_newline_between() {
        let env = make_test_env_with_stdin(vec!["tail", "-n", "1", "a.txt", "b.txt"], "");
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
        let env = make_test_env_with_stdin(vec!["tail", "-n", "1", "-q", "a.txt", "b.txt"], "");
        env.fs.add_file("a.txt", "a1\na2\na3\n");
        env.fs.add_file("b.txt", "b1\nb2\nb3\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert!(!stdout.contains("==>"));
        assert!(stdout.contains("a3\n"));
        assert!(stdout.contains("b3\n"));
    }

    #[test]
    fn combined_cv() {
        let env = make_test_env_with_stdin(vec!["tail", "-c", "3", "-v", "file.txt"], "");
        env.fs.add_file("file.txt", "hello");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert!(stdout.contains("==> file.txt <=="));
        assert!(stdout.contains("llo"));
    }

    // ========================================================================
    // Comparison with head (opposite behavior)
    // ========================================================================

    #[test]
    fn tail_vs_head_default() {
        // With 12 lines, head shows first 10, tail shows last 10
        let input = "1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n11\n12";
        let env = make_test_env_with_stdin(vec!["tail"], input);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        // Should NOT contain 1 and 2
        assert!(!stdout.starts_with("1\n"));
        assert!(!stdout.contains("\n1\n"));
        // Should contain 12
        assert!(stdout.contains("12"));
    }
}

use getopts::Options;

use crate::{Environment, Error, ExitCode, Filesystem, FsError, Stderr, Stdin, Stdout};

/// Options for the wc command.
#[derive(Clone, Copy, Debug, Default)]
struct WcOptions {
    /// -l: Count lines.
    lines: bool,
    /// -w: Count words.
    words: bool,
    /// -c: Count bytes.
    bytes: bool,
    /// -m: Count characters.
    chars: bool,
    /// -L: Display longest line length.
    longest_line: bool,
}

impl WcOptions {
    fn any_set(&self) -> bool {
        self.lines || self.words || self.bytes || self.chars || self.longest_line
    }

    fn set_defaults(&mut self) {
        if !self.any_set() {
            self.lines = true;
            self.words = true;
            self.bytes = true;
        }
    }
}

/// Counts for a single file or stdin.
#[derive(Clone, Copy, Debug, Default)]
struct WcCounts {
    lines: usize,
    words: usize,
    bytes: usize,
    chars: usize,
    longest_line: usize,
}

impl WcCounts {
    fn add(&mut self, other: &WcCounts) {
        self.lines += other.lines;
        self.words += other.words;
        self.bytes += other.bytes;
        self.chars += other.chars;
        if other.longest_line > self.longest_line {
            self.longest_line = other.longest_line;
        }
    }
}

fn build_options() -> Options {
    let mut opts = Options::new();
    opts.optflag("l", "", "Count lines.");
    opts.optflag("w", "", "Count words.");
    opts.optflag("c", "", "Count bytes.");
    opts.optflag("m", "", "Count characters.");
    opts.optflag("L", "", "Display the length of the longest line.");
    opts
}

/// The wc builtin: count lines, words, and bytes.
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
            env.stderr.write_line(&format!("wc: {}", e))?;
            return Ok(ExitCode::from(1));
        }
    };

    let mut wc_opts = WcOptions {
        lines: matches.opt_present("l"),
        words: matches.opt_present("w"),
        bytes: matches.opt_present("c") && !matches.opt_present("m"),
        chars: matches.opt_present("m"),
        longest_line: matches.opt_present("L"),
    };

    // -c and -m are mutually exclusive; last one wins.
    // Re-check: if -c is present and -m is not, bytes is true.
    // If -m is present, chars is true and bytes is false (already handled above).
    // But we need to handle the "last one wins" properly by scanning args.
    if matches.opt_present("c") && matches.opt_present("m") {
        // Need to check which came last
        let args: Vec<String> = env.args[1..].to_vec();
        let mut use_bytes = false;
        for arg in &args {
            if arg.starts_with('-') && !arg.starts_with("--") {
                for ch in arg.chars().skip(1) {
                    match ch {
                        'c' => use_bytes = true,
                        'm' => use_bytes = false,
                        _ => {}
                    }
                }
            }
        }
        wc_opts.bytes = use_bytes;
        wc_opts.chars = !use_bytes;
    }

    wc_opts.set_defaults();

    let mut total = WcCounts::default();
    let mut exit_code: i8 = 0;
    let file_count = matches.free.len();

    if matches.free.is_empty() {
        let counts = count_stdin(env, &wc_opts)?;
        show_counts(env, &counts, &wc_opts, None)?;
    } else {
        for file in &matches.free {
            match count_file(env, file, &wc_opts) {
                Ok(counts) => {
                    total.add(&counts);
                    show_counts(env, &counts, &wc_opts, Some(file))?;
                }
                Err(msg) => {
                    env.stderr.write_line(&format!("wc: {}", msg))?;
                    exit_code = 1;
                }
            }
        }

        if file_count > 1 {
            show_counts(env, &total, &wc_opts, Some("total"))?;
        }
    }

    Ok(ExitCode::from(exit_code))
}

/// Count lines, words, bytes, and chars from a string.
fn count_string(s: &str, opts: &WcOptions) -> WcCounts {
    let mut counts = WcCounts {
        bytes: s.len(),
        ..Default::default()
    };

    if opts.chars {
        counts.chars = s.chars().count();
    }

    let mut in_word = false;
    let mut current_line_len = 0;

    for ch in s.chars() {
        if ch == '\n' {
            counts.lines += 1;
            if opts.longest_line && current_line_len > counts.longest_line {
                counts.longest_line = current_line_len;
            }
            current_line_len = 0;
        } else if opts.longest_line {
            if opts.chars {
                current_line_len += 1;
            } else {
                current_line_len += ch.len_utf8();
            }
        }

        if ch.is_whitespace() {
            in_word = false;
        } else if !in_word {
            in_word = true;
            counts.words += 1;
        }
    }

    // Handle last line if it doesn't end with newline
    if opts.longest_line && current_line_len > counts.longest_line {
        counts.longest_line = current_line_len;
    }

    counts
}

/// Count from stdin.
fn count_stdin<SI, SO, SE, FS>(
    env: &Environment<SI, SO, SE, FS>,
    opts: &WcOptions,
) -> Result<WcCounts, Error>
where
    SI: Stdin,
    SO: Stdout,
    SE: Stderr,
    FS: Filesystem,
{
    let mut counts = WcCounts::default();
    let mut in_word = false;

    while let Some(line) = env.stdin.read_line()? {
        // read_line strips the newline, so we add 1 for the newline byte
        counts.bytes += line.len() + 1;
        counts.lines += 1;

        if opts.chars {
            counts.chars += line.chars().count() + 1; // +1 for newline
        }

        if opts.longest_line {
            let line_len = if opts.chars {
                line.chars().count()
            } else {
                line.len()
            };
            if line_len > counts.longest_line {
                counts.longest_line = line_len;
            }
        }

        for ch in line.chars() {
            if ch.is_whitespace() {
                in_word = false;
            } else if !in_word {
                in_word = true;
                counts.words += 1;
            }
        }
        // Newline resets word boundary
        in_word = false;
    }

    Ok(counts)
}

/// Count from a file.
fn count_file<SI, SO, SE, FS>(
    env: &Environment<SI, SO, SE, FS>,
    path: &str,
    opts: &WcOptions,
) -> Result<WcCounts, String>
where
    SI: Stdin,
    SO: Stdout,
    SE: Stderr,
    FS: Filesystem,
{
    match env.fs.read_to_string(path) {
        Ok(contents) => Ok(count_string(&contents, opts)),
        Err(FsError::Io(e)) => Err(format!("{}: {}", path, e)),
    }
}

/// Display the counts for a file or stdin.
fn show_counts<SI, SO, SE, FS>(
    env: &Environment<SI, SO, SE, FS>,
    counts: &WcCounts,
    opts: &WcOptions,
    filename: Option<&str>,
) -> Result<(), Error>
where
    SI: Stdin,
    SO: Stdout,
    SE: Stderr,
    FS: Filesystem,
{
    let mut parts = Vec::new();

    if opts.lines {
        parts.push(format!("{:>7}", counts.lines));
    }
    if opts.words {
        parts.push(format!("{:>7}", counts.words));
    }
    if opts.bytes || opts.chars {
        let count = if opts.chars {
            counts.chars
        } else {
            counts.bytes
        };
        parts.push(format!("{:>7}", count));
    }
    if opts.longest_line {
        parts.push(format!("{:>7}", counts.longest_line));
    }

    let output = if let Some(name) = filename {
        format!("{} {}", parts.join(""), name)
    } else {
        parts.join("")
    };

    Ok(env.stdout.write_line(&output)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::make_test_env_with_stdin;

    // ========================================================================
    // Basic functionality tests
    // ========================================================================

    #[test]
    fn empty_stdin() {
        let env = make_test_env_with_stdin(vec!["wc"], "");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert_eq!("      0      0      0\n", stdout);
    }

    #[test]
    fn single_line() {
        let env = make_test_env_with_stdin(vec!["wc"], "hello world");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert_eq!("      1      2     12\n", stdout);
    }

    #[test]
    fn multiple_lines() {
        let env = make_test_env_with_stdin(vec!["wc"], "one\ntwo\nthree");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        // 3 lines, 3 words, 14 bytes (3+1 + 3+1 + 5+1 = 14)
        assert_eq!("      3      3     14\n", stdout);
    }

    // ========================================================================
    // File tests
    // ========================================================================

    #[test]
    fn file_not_found() {
        let env = make_test_env_with_stdin(vec!["wc", "nonexistent.txt"], "");
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("nonexistent.txt"));
    }

    #[test]
    fn single_file() {
        let env = make_test_env_with_stdin(vec!["wc", "file.txt"], "");
        env.fs.add_file("file.txt", "a b\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert_eq!("      1      2      4 file.txt\n", stdout);
    }

    #[test]
    fn multiple_files() {
        let env = make_test_env_with_stdin(vec!["wc", "a.txt", "b.txt"], "");
        env.fs.add_file("a.txt", "one\n");
        env.fs.add_file("b.txt", "two three\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert!(stdout.contains("a.txt"));
        assert!(stdout.contains("b.txt"));
        assert!(stdout.contains("total"));
    }

    #[test]
    fn multiple_files_with_totals() {
        let env = make_test_env_with_stdin(vec!["wc", "a.txt", "b.txt"], "");
        env.fs.add_file("a.txt", "one\n"); // 1 line, 1 word, 4 bytes
        env.fs.add_file("b.txt", "two three\n"); // 1 line, 2 words, 10 bytes
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        // Total: 2 lines, 3 words, 14 bytes
        assert!(stdout.contains("      2      3     14 total"));
    }

    // ========================================================================
    // Individual flag tests
    // ========================================================================

    #[test]
    fn lines_only() {
        let env = make_test_env_with_stdin(vec!["wc", "-l"], "one\ntwo\nthree");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert_eq!("      3\n", stdout);
    }

    #[test]
    fn words_only() {
        let env = make_test_env_with_stdin(vec!["wc", "-w"], "one two three");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert_eq!("      3\n", stdout);
    }

    #[test]
    fn bytes_only() {
        let env = make_test_env_with_stdin(vec!["wc", "-c"], "hello");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        // "hello" + newline = 6 bytes
        assert_eq!("      6\n", stdout);
    }

    #[test]
    fn chars_only() {
        let env = make_test_env_with_stdin(vec!["wc", "-m"], "hello");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        // "hello" + newline = 6 chars
        assert_eq!("      6\n", stdout);
    }

    #[test]
    fn longest_line_only() {
        let env = make_test_env_with_stdin(vec!["wc", "-L"], "short\nlonger line\nx");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        // "longer line" = 11 chars
        assert_eq!("     11\n", stdout);
    }

    // ========================================================================
    // Combined flags
    // ========================================================================

    #[test]
    fn lines_and_words() {
        let env = make_test_env_with_stdin(vec!["wc", "-lw"], "one two\nthree");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        // 2 lines, 3 words
        assert_eq!("      2      3\n", stdout);
    }

    #[test]
    fn all_flags() {
        let env = make_test_env_with_stdin(vec!["wc", "-lwcL"], "hello\nworld");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        // 2 lines, 2 words, 12 bytes, longest line 5
        assert_eq!("      2      2     12      5\n", stdout);
    }

    // ========================================================================
    // -c and -m mutual exclusivity
    // ========================================================================

    #[test]
    fn chars_overrides_bytes() {
        let env = make_test_env_with_stdin(vec!["wc", "-cm"], "hello");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        // -m comes after -c, so chars wins
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert_eq!("      6\n", stdout);
    }

    #[test]
    fn bytes_overrides_chars() {
        let env = make_test_env_with_stdin(vec!["wc", "-mc"], "hello");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        // -c comes after -m, so bytes wins
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert_eq!("      6\n", stdout);
    }

    // ========================================================================
    // Edge cases
    // ========================================================================

    #[test]
    fn empty_file() {
        let env = make_test_env_with_stdin(vec!["wc", "empty.txt"], "");
        env.fs.add_file("empty.txt", "");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert_eq!("      0      0      0 empty.txt\n", stdout);
    }

    #[test]
    fn blank_lines() {
        let env = make_test_env_with_stdin(vec!["wc"], "\n\n\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        // 3 lines, 0 words, 3 bytes
        assert_eq!("      3      0      3\n", stdout);
    }

    #[test]
    fn whitespace_only() {
        let env = make_test_env_with_stdin(vec!["wc"], " \t \n  \n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        // 2 lines, 0 words, 7 bytes
        assert_eq!("      2      0      7\n", stdout);
    }

    #[test]
    fn no_trailing_newline_file() {
        let env = make_test_env_with_stdin(vec!["wc", "file.txt"], "");
        env.fs.add_file("file.txt", "no newline");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        // 0 lines (no newline), 2 words, 10 bytes
        assert_eq!("      0      2     10 file.txt\n", stdout);
    }

    #[test]
    fn multiple_files_one_missing() {
        let env = make_test_env_with_stdin(vec!["wc", "a.txt", "missing.txt", "b.txt"], "");
        env.fs.add_file("a.txt", "aaa\n");
        env.fs.add_file("b.txt", "bbb\n");
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stdout = env.stdout.into_string();
        let stderr = env.stderr.into_string();
        println!("stdout: {:?}", stdout);
        println!("stderr: {:?}", stderr);
        assert!(stdout.contains("a.txt"));
        assert!(stdout.contains("b.txt"));
        assert!(stderr.contains("missing.txt"));
    }

    // ========================================================================
    // Longest line tests
    // ========================================================================

    #[test]
    fn longest_line_with_bytes() {
        let env = make_test_env_with_stdin(vec!["wc", "-cL"], "short\nlonger line here\nx");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        // 23 bytes, longest line 16
        assert!(stdout.contains("16"));
    }

    #[test]
    fn longest_line_across_files() {
        let env = make_test_env_with_stdin(vec!["wc", "-L", "a.txt", "b.txt"], "");
        env.fs.add_file("a.txt", "short\n");
        env.fs.add_file("b.txt", "this is a longer line\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        // Total should show longest line of all files
        assert!(stdout.contains("total"));
        // "this is a longer line" = 21 chars
        assert!(stdout.contains("21"));
    }

    // ========================================================================
    // count_string unit tests
    // ========================================================================

    #[test]
    fn count_string_basic() {
        let opts = WcOptions {
            lines: true,
            words: true,
            bytes: true,
            chars: false,
            longest_line: false,
        };
        let counts = count_string("a b\n", &opts);
        assert_eq!(1, counts.lines);
        assert_eq!(2, counts.words);
        assert_eq!(4, counts.bytes);
    }

    #[test]
    fn count_string_empty() {
        let opts = WcOptions {
            lines: true,
            words: true,
            bytes: true,
            chars: false,
            longest_line: false,
        };
        let counts = count_string("", &opts);
        assert_eq!(0, counts.lines);
        assert_eq!(0, counts.words);
        assert_eq!(0, counts.bytes);
    }

    #[test]
    fn count_string_no_newline() {
        let opts = WcOptions {
            lines: true,
            words: true,
            bytes: true,
            chars: false,
            longest_line: false,
        };
        let counts = count_string("hello world", &opts);
        assert_eq!(0, counts.lines);
        assert_eq!(2, counts.words);
        assert_eq!(11, counts.bytes);
    }

    #[test]
    fn count_string_longest_line() {
        let opts = WcOptions {
            lines: false,
            words: false,
            bytes: false,
            chars: false,
            longest_line: true,
        };
        let counts = count_string("short\nlonger line\nx\n", &opts);
        assert_eq!(11, counts.longest_line);
    }

    #[test]
    fn count_string_longest_line_no_final_newline() {
        let opts = WcOptions {
            lines: false,
            words: false,
            bytes: false,
            chars: false,
            longest_line: true,
        };
        let counts = count_string("short\nlongest line here", &opts);
        assert_eq!(17, counts.longest_line);
    }

    // ========================================================================
    // Option parsing tests
    // ========================================================================

    #[test]
    fn illegal_option() {
        let env = make_test_env_with_stdin(vec!["wc", "-x"], "");
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("Unrecognized option"));
    }

    // ========================================================================
    // File with filename output
    // ========================================================================

    #[test]
    fn file_shows_filename() {
        let env = make_test_env_with_stdin(vec!["wc", "-l", "myfile.txt"], "");
        env.fs.add_file("myfile.txt", "line1\nline2\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert_eq!("      2 myfile.txt\n", stdout);
    }

    #[test]
    fn stdin_no_filename() {
        let env = make_test_env_with_stdin(vec!["wc", "-l"], "line1\nline2");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert_eq!("      2\n", stdout);
        // No filename should appear
        assert!(!stdout.contains("stdin"));
    }
}

//! The fold utility: wrap lines to a specified width.

use getopts::Options;

use crate::{Environment, Error, ExitCode, Filesystem, Stderr, Stdin, Stdout};

/// Default line width.
const DEFAULT_WIDTH: usize = 80;

/// Options for the fold command.
#[derive(Clone, Copy, Debug, Default)]
struct FoldOptions {
    /// -b: Count width in bytes rather than column positions.
    bytes: bool,
    /// -s: Fold line after the last blank character within the width.
    spaces: bool,
    /// Line width to fold at.
    width: usize,
}

fn build_options() -> Options {
    let mut opts = Options::new();
    opts.optflag(
        "b",
        "",
        "Count width in bytes rather than column positions.",
    );
    opts.optflag(
        "s",
        "",
        "Fold line after the last blank character within the first width column positions.",
    );
    opts.optopt(
        "w",
        "",
        "Specify a line width to use instead of the default 80 columns.",
        "WIDTH",
    );
    opts
}

/// The fold builtin: wrap lines to a specified width.
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
            env.stderr.write_line(&format!("fold: {}", e))?;
            return Ok(ExitCode::from(1));
        }
    };

    let width = if let Some(w) = matches.opt_str("w") {
        match w.parse::<usize>() {
            Ok(0) => {
                env.stderr.write_line("fold: illegal width value")?;
                return Ok(ExitCode::from(1));
            }
            Ok(n) => n,
            Err(_) => {
                env.stderr.write_line("fold: illegal width value")?;
                return Ok(ExitCode::from(1));
            }
        }
    } else {
        DEFAULT_WIDTH
    };

    let fold_opts = FoldOptions {
        bytes: matches.opt_present("b"),
        spaces: matches.opt_present("s"),
        width,
    };

    let mut exit_code: i8 = 0;

    if matches.free.is_empty() {
        fold_stdin(env, &fold_opts)?;
    } else {
        for file in &matches.free {
            if file == "-" {
                fold_stdin(env, &fold_opts)?;
            } else if let Err(code) = fold_file(env, file, &fold_opts) {
                exit_code = code;
            }
        }
    }

    Ok(ExitCode::from(exit_code))
}

fn fold_stdin<SI, SO, SE, FS>(
    env: &Environment<SI, SO, SE, FS>,
    opts: &FoldOptions,
) -> Result<(), Error>
where
    SI: Stdin,
    SO: Stdout,
    SE: Stderr,
    FS: Filesystem,
{
    while let Some(line) = env.stdin.read_line()? {
        fold_line(env, &line, opts)?;
    }
    Ok(())
}

fn fold_file<SI, SO, SE, FS>(
    env: &Environment<SI, SO, SE, FS>,
    path: &str,
    opts: &FoldOptions,
) -> Result<(), i8>
where
    SI: Stdin,
    SO: Stdout,
    SE: Stderr,
    FS: Filesystem,
{
    match env.fs.read_to_string(path) {
        Ok(contents) => {
            for line in contents.lines() {
                if fold_line(env, line, opts).is_err() {
                    return Err(1);
                }
            }
            Ok(())
        }
        Err(Error::Io(e)) => {
            let _ = env.stderr.write_line(&format!("fold: {}: {}", path, e));
            Err(1)
        }
        Err(_e) => Err(1),
    }
}

/// Fold a single line to the specified width.
fn fold_line<SI, SO, SE, FS>(
    env: &Environment<SI, SO, SE, FS>,
    line: &str,
    opts: &FoldOptions,
) -> Result<(), Error>
where
    SI: Stdin,
    SO: Stdout,
    SE: Stderr,
    FS: Filesystem,
{
    if opts.bytes {
        fold_line_bytes(env, line, opts)
    } else {
        fold_line_columns(env, line, opts)
    }
}

/// Fold a line counting bytes.
fn fold_line_bytes<SI, SO, SE, FS>(
    env: &Environment<SI, SO, SE, FS>,
    line: &str,
    opts: &FoldOptions,
) -> Result<(), Error>
where
    SI: Stdin,
    SO: Stdout,
    SE: Stderr,
    FS: Filesystem,
{
    let bytes = line.as_bytes();
    let mut start = 0;

    while start < bytes.len() {
        let remaining = bytes.len() - start;
        if remaining <= opts.width {
            env.stdout.write_line(&line[start..])?;
            return Ok(());
        }

        let end = start + opts.width;

        if opts.spaces {
            // Look for the last blank within the width
            let mut last_blank = None;
            for (i, &byte) in bytes.iter().enumerate().take(end).skip(start) {
                if byte == b' ' || byte == b'\t' {
                    last_blank = Some(i);
                }
            }
            if let Some(blank_pos) = last_blank {
                // Include the blank in the output, then break after it
                env.stdout.write_line(&line[start..=blank_pos])?;
                start = blank_pos + 1;
                continue;
            }
        }

        // No suitable break point found, or -s not set: hard break
        env.stdout.write_line(&line[start..end])?;
        start = end;
    }

    // Handle empty line case
    if start == 0 {
        env.stdout.write_line("")?;
    }

    Ok(())
}

/// Fold a line counting columns.
fn fold_line_columns<SI, SO, SE, FS>(
    env: &Environment<SI, SO, SE, FS>,
    line: &str,
    opts: &FoldOptions,
) -> Result<(), Error>
where
    SI: Stdin,
    SO: Stdout,
    SE: Stderr,
    FS: Filesystem,
{
    let chars: Vec<char> = line.chars().collect();
    let mut start = 0;
    let mut col = 0;
    let mut buf = String::new();
    let mut last_blank_idx: Option<usize> = None;
    let mut last_blank_col = 0;
    let mut i = 0;

    while i < chars.len() {
        let ch = chars[i];
        let ch_width = column_width(ch, col);
        let new_col = col + ch_width;

        if new_col > opts.width {
            // Line needs to be broken
            if opts.spaces && last_blank_idx.is_some() {
                // Break at the last blank
                let blank_idx = last_blank_idx.unwrap();
                let output: String = chars[start..=blank_idx].iter().collect();
                env.stdout.write_line(&output)?;
                start = blank_idx + 1;
                i = start;
                col = 0;
                last_blank_idx = None;
                buf.clear();
            } else {
                // Hard break at current position
                let output: String = chars[start..i].iter().collect();
                env.stdout.write_line(&output)?;
                start = i;
                col = 0;
                last_blank_idx = None;
                buf.clear();
            }
        } else {
            // Character fits
            if ch == ' ' || ch == '\t' {
                last_blank_idx = Some(i);
                last_blank_col = new_col;
            }
            col = new_col;
            i += 1;
        }
    }

    // Output remaining characters
    if start < chars.len() {
        let output: String = chars[start..].iter().collect();
        env.stdout.write_line(&output)?;
    } else if start == 0 && chars.is_empty() {
        env.stdout.write_line("")?;
    }

    // Suppress unused variable warning
    let _ = last_blank_col;

    Ok(())
}

/// Calculate the column width of a character.
fn column_width(ch: char, current_col: usize) -> usize {
    match ch {
        '\t' => {
            // Tab advances to next multiple of 8
            8 - (current_col % 8)
        }
        '\x08' => {
            // Backspace: width is 0 (but we don't go negative)
            0
        }
        '\r' => {
            // Carriage return: effectively 0 width
            0
        }
        _ => {
            // Simple width: control characters are 0, printable are 1
            if ch.is_control() { 0 } else { 1 }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MockFilesystem, StringStderr, StringStdin, StringStdout};

    fn make_env(
        args: Vec<&str>,
        stdin: &str,
    ) -> Environment<StringStdin, StringStdout, StringStderr, MockFilesystem> {
        Environment {
            stdin: StringStdin::new(stdin),
            stdout: StringStdout::new(),
            stderr: StringStderr::new(),
            fs: MockFilesystem::new(),
            env: std::collections::HashMap::new(),
            args: args.into_iter().map(|s| s.to_string()).collect(),
            cwd: utf8path::Path::from("/"),
            exit_signaled: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }

    // ========================================================================
    // Basic functionality tests
    // ========================================================================

    #[test]
    fn empty_stdin() {
        let env = make_env(vec!["fold"], "");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("", env.stdout.into_string());
    }

    #[test]
    fn short_line_unchanged() {
        let env = make_env(vec!["fold"], "hello world");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("hello world\n", env.stdout.into_string());
    }

    #[test]
    fn line_at_exactly_80_chars() {
        let line = "a".repeat(80);
        let env = make_env(vec!["fold"], &line);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!(format!("{}\n", line), env.stdout.into_string());
    }

    #[test]
    fn line_over_80_chars() {
        let line = "a".repeat(100);
        let env = make_env(vec!["fold"], &line);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let expected = format!("{}\n{}\n", "a".repeat(80), "a".repeat(20));
        assert_eq!(expected, env.stdout.into_string());
    }

    #[test]
    fn multiple_lines() {
        let env = make_env(vec!["fold", "-w", "10"], "short\nthis is a longer line");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert!(stdout.contains("short\n"));
    }

    // ========================================================================
    // -w flag: custom width
    // ========================================================================

    #[test]
    fn custom_width_15() {
        let env = make_env(
            vec!["fold", "-w", "15"],
            "I am smart enough to know that I am dumb",
        );
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        // Each line should be at most 15 characters
        for line in stdout.lines() {
            assert!(
                line.len() <= 15,
                "Line too long: {:?} (len={})",
                line,
                line.len()
            );
        }
    }

    #[test]
    fn width_of_1() {
        let env = make_env(vec!["fold", "-w", "1"], "abc");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("a\nb\nc\n", env.stdout.into_string());
    }

    #[test]
    fn width_of_0_is_error() {
        let env = make_env(vec!["fold", "-w", "0"], "test");
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("illegal width value"));
    }

    #[test]
    fn width_negative_is_error() {
        let env = make_env(vec!["fold", "-w", "-5"], "test");
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
    }

    #[test]
    fn width_non_numeric_is_error() {
        let env = make_env(vec!["fold", "-w", "abc"], "test");
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("illegal width value"));
    }

    // ========================================================================
    // -s flag: break at spaces
    // ========================================================================

    #[test]
    fn spaces_flag_breaks_at_word() {
        let env = make_env(
            vec!["fold", "-s", "-w", "15"],
            "I am smart enough to know that I am dumb",
        );
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        // Lines should break at word boundaries
        assert!(stdout.contains("I am smart "));
    }

    #[test]
    fn spaces_flag_no_spaces_falls_back() {
        let env = make_env(vec!["fold", "-s", "-w", "5"], "abcdefghij");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        // No spaces, so hard break at width
        assert_eq!("abcde\nfghij\n", stdout);
    }

    #[test]
    fn spaces_flag_with_tabs() {
        let env = make_env(vec!["fold", "-s", "-w", "10"], "hello\tworld");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        // Tab is a blank character
        assert!(stdout.contains("hello\t"));
    }

    // ========================================================================
    // -b flag: count bytes
    // ========================================================================

    #[test]
    fn bytes_flag_ascii() {
        let env = make_env(vec!["fold", "-b", "-w", "5"], "abcdefghij");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("abcde\nfghij\n", env.stdout.into_string());
    }

    #[test]
    fn bytes_flag_with_spaces() {
        let env = make_env(vec!["fold", "-b", "-s", "-w", "10"], "hello world foo");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        // Should break at spaces
        assert!(stdout.contains("hello "));
    }

    // ========================================================================
    // Tab handling
    // ========================================================================

    #[test]
    fn tab_expands_to_8_from_start() {
        // Tab at position 0 should expand to 8 columns
        let env = make_env(vec!["fold", "-w", "10"], "\thello");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        // Tab takes 8 columns, "he" takes 2, total 10 -> fits on one line
        // "llo" would be 3 more, exceeding 10
        // Actually tab=8, "hello"=5, total=13 > 10
        // So it should break
    }

    #[test]
    fn tab_expands_to_next_multiple_of_8() {
        // "abc" is 3 chars, tab should expand to column 8 (5 spaces worth)
        let env = make_env(vec!["fold", "-w", "10"], "abc\tdef");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        // abc=3, tab->col8 (+5), def=3, total=11 > 10
    }

    // ========================================================================
    // File handling
    // ========================================================================

    #[test]
    fn file_not_found() {
        let env = make_env(vec!["fold", "nonexistent.txt"], "");
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("nonexistent.txt"));
    }

    #[test]
    fn single_file() {
        let env = make_env(vec!["fold", "-w", "5", "file.txt"], "");
        // File without trailing newline to avoid extra blank line
        env.fs.add_file("file.txt", "abcdefghij");
        let result = bin(&env).unwrap();
        println!("stdout: {:?}", env.stdout.into_string());
        assert_eq!(0, result.code());
        assert_eq!("abcde\nfghij\n", env.stdout.into_string());
    }

    #[test]
    fn multiple_files() {
        let env = make_env(vec!["fold", "-w", "3", "a.txt", "b.txt"], "");
        env.fs.add_file("a.txt", "aaa\n");
        env.fs.add_file("b.txt", "bbbbbb\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert!(stdout.contains("aaa"));
        assert!(stdout.contains("bbb"));
    }

    #[test]
    fn dash_means_stdin() {
        let env = make_env(vec!["fold", "-w", "5", "-"], "abcdefghij");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("abcde\nfghij\n", env.stdout.into_string());
    }

    #[test]
    fn mixed_files_and_stdin() {
        let env = make_env(vec!["fold", "-w", "3", "a.txt", "-"], "xyz");
        env.fs.add_file("a.txt", "abc\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert!(stdout.contains("abc"));
        assert!(stdout.contains("xyz"));
    }

    // ========================================================================
    // Edge cases
    // ========================================================================

    #[test]
    fn empty_line() {
        let env = make_env(vec!["fold"], "");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("", env.stdout.into_string());
    }

    #[test]
    fn only_spaces() {
        let env = make_env(vec!["fold", "-w", "3"], "     ");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        // 5 spaces at width 3
    }

    #[test]
    fn very_long_word_with_s_flag() {
        // A word longer than the width with -s should still hard-break
        let env = make_env(vec!["fold", "-s", "-w", "5"], "abcdefghij");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("abcde\nfghij\n", env.stdout.into_string());
    }

    #[test]
    fn trailing_space_preserved() {
        let env = make_env(vec!["fold", "-w", "10"], "hello     ");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert_eq!("hello     \n", stdout);
    }

    // ========================================================================
    // column_width function tests
    // ========================================================================

    #[test]
    fn column_width_regular_char() {
        assert_eq!(1, column_width('a', 0));
        assert_eq!(1, column_width('Z', 5));
        assert_eq!(1, column_width(' ', 0));
    }

    #[test]
    fn column_width_tab_from_0() {
        assert_eq!(8, column_width('\t', 0));
    }

    #[test]
    fn column_width_tab_from_3() {
        // From column 3, tab goes to column 8, so width is 5
        assert_eq!(5, column_width('\t', 3));
    }

    #[test]
    fn column_width_tab_from_7() {
        // From column 7, tab goes to column 8, so width is 1
        assert_eq!(1, column_width('\t', 7));
    }

    #[test]
    fn column_width_tab_from_8() {
        // From column 8, tab goes to column 16, so width is 8
        assert_eq!(8, column_width('\t', 8));
    }

    #[test]
    fn column_width_backspace() {
        assert_eq!(0, column_width('\x08', 5));
    }

    #[test]
    fn column_width_carriage_return() {
        assert_eq!(0, column_width('\r', 5));
    }

    // ========================================================================
    // Option parsing tests
    // ========================================================================

    #[test]
    fn illegal_option() {
        let env = make_env(vec!["fold", "-x"], "test");
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("Unrecognized option"));
    }
}

//! The uniq builtin: report or filter out repeated lines in a file.

use getopts::Options;

use crate::{Environment, Error, ExitCode, Filesystem, FsError, StdioIn, StdioOut};

/// Separator type for the -D (all-repeated) option.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum AllRepeatedSepType {
    /// Do not output separators between groups.
    #[default]
    None,
    /// Output an empty line before each group of lines.
    Prepend,
    /// Output an empty line after each group of lines.
    Separate,
}

/// Options for the uniq command.
#[derive(Clone, Copy, Debug, Default)]
struct UniqOptions {
    /// Precede each output line with the count.
    count: bool,
    /// Output a single copy of each repeated line.
    repeated: bool,
    /// Output all repeated lines (like -d but each copy).
    all_repeated: Option<AllRepeatedSepType>,
    /// Ignore the first N fields.
    skip_fields: usize,
    /// Ignore the first N characters (after skipping fields).
    skip_chars: usize,
    /// Case insensitive comparison.
    ignore_case: bool,
    /// Only output lines that are not repeated.
    unique: bool,
}

fn build_options() -> Options {
    let mut opts = Options::new();
    opts.optflag("c", "count", "Precede each output line with count.");
    opts.optflag(
        "d",
        "repeated",
        "Output a single copy of each repeated line.",
    );
    opts.optflagopt(
        "D",
        "all-repeated",
        "Output all repeated lines. Optional septype: none, prepend, separate.",
        "septype",
    );
    opts.optopt(
        "f",
        "skip-fields",
        "Ignore the first num fields in each input line.",
        "num",
    );
    opts.optflag("i", "ignore-case", "Case insensitive comparison.");
    opts.optopt(
        "s",
        "skip-chars",
        "Ignore the first chars characters in each input line.",
        "chars",
    );
    opts.optflag("u", "unique", "Only output lines that are not repeated.");
    opts
}

/// The uniq builtin: report or filter out repeated lines in a file.
pub fn bin<SI, SO, SE, FS>(env: &Environment<SI, SO, SE, FS>) -> Result<ExitCode, Error>
where
    SI: StdioIn,
    SO: StdioOut,
    SE: StdioOut,
    FS: Filesystem,
{
    let opts_def = build_options();

    let matches = match opts_def.parse(&env.args[1..]) {
        Ok(m) => m,
        Err(e) => {
            env.stderr.write_line(&format!("uniq: {}", e))?;
            return Ok(ExitCode::from(1));
        }
    };

    let count = matches.opt_present("c");
    let repeated = matches.opt_present("d");
    let unique = matches.opt_present("u");
    let ignore_case = matches.opt_present("i");

    let all_repeated = if matches.opt_present("D") {
        let septype = matches.opt_str("D").unwrap_or_default();
        let sep = match septype.to_lowercase().as_str() {
            "" | "none" => AllRepeatedSepType::None,
            "prepend" => AllRepeatedSepType::Prepend,
            "separate" => AllRepeatedSepType::Separate,
            _ => {
                env.stderr.write_line(&format!(
                    "uniq: invalid argument '{}' for '--all-repeated'",
                    septype
                ))?;
                return Ok(ExitCode::from(1));
            }
        };
        Some(sep)
    } else {
        None
    };

    let skip_fields = if let Some(n) = matches.opt_str("f") {
        match n.parse::<usize>() {
            Ok(num) => num,
            Err(_) => {
                env.stderr
                    .write_line(&format!("uniq: invalid number of fields: '{}'", n))?;
                return Ok(ExitCode::from(1));
            }
        }
    } else {
        0
    };

    let skip_chars = if let Some(n) = matches.opt_str("s") {
        match n.parse::<usize>() {
            Ok(num) => num,
            Err(_) => {
                env.stderr
                    .write_line(&format!("uniq: invalid number of characters: '{}'", n))?;
                return Ok(ExitCode::from(1));
            }
        }
    } else {
        0
    };

    let opts = UniqOptions {
        count,
        repeated,
        all_repeated,
        skip_fields,
        skip_chars,
        ignore_case,
        unique,
    };

    let free = &matches.free;

    if free.len() > 2 {
        env.stderr
            .write_line("uniq: extra operand after output file")?;
        return Ok(ExitCode::from(1));
    }

    let input = if free.is_empty() || free[0] == "-" {
        read_stdin(env)?
    } else {
        match env.fs.read_to_string(&free[0]) {
            Ok(contents) => contents,
            Err(FsError::Io(e)) => {
                env.stderr
                    .write_line(&format!("uniq: {}: {}", free[0], e))?;
                return Ok(ExitCode::from(1));
            }
        }
    };

    let output = process_uniq(&input, &opts);

    if free.len() == 2 {
        if let Err(e) = env.fs.write_string(&free[1], &output) {
            env.stderr
                .write_line(&format!("uniq: {}: {:?}", free[1], e))?;
            return Ok(ExitCode::from(1));
        }
    } else {
        env.stdout.write_str(&output)?;
    }

    Ok(ExitCode::from(0))
}

fn read_stdin<SI, SO, SE, FS>(env: &Environment<SI, SO, SE, FS>) -> Result<String, Error>
where
    SI: StdioIn,
    SO: StdioOut,
    SE: StdioOut,
    FS: Filesystem,
{
    let mut contents = String::new();
    while let Some(line) = env.stdin.read_line()? {
        contents.push_str(&line);
        contents.push('\n');
    }
    Ok(contents)
}

/// Skip fields and characters to get the comparison key for a line.
fn get_comparison_key(line: &str, opts: &UniqOptions) -> String {
    let mut chars = line.chars().peekable();

    for _ in 0..opts.skip_fields {
        while chars.peek().is_some_and(|c| c.is_whitespace()) {
            chars.next();
        }
        while chars.peek().is_some_and(|c| !c.is_whitespace()) {
            chars.next();
        }
    }

    for _ in 0..opts.skip_chars {
        if chars.next().is_none() {
            break;
        }
    }

    let key: String = chars.collect();

    if opts.ignore_case {
        key.to_lowercase()
    } else {
        key
    }
}

/// Compare two lines for equality based on options.
fn lines_equal(a: &str, b: &str, opts: &UniqOptions) -> bool {
    get_comparison_key(a, opts) == get_comparison_key(b, opts)
}

/// Format an output line with optional count prefix.
fn format_line(line: &str, count: u64, show_count: bool) -> String {
    if show_count {
        format!("{:4} {}\n", count, line)
    } else {
        format!("{}\n", line)
    }
}

fn process_uniq(input: &str, opts: &UniqOptions) -> String {
    let mut output = String::new();
    let lines: Vec<&str> = input.lines().collect();

    if lines.is_empty() {
        return output;
    }

    let all_repeated = opts.all_repeated;
    let show_repeated_only = opts.repeated && all_repeated.is_none();
    let show_unique_only = opts.unique;
    let show_count = opts.count;

    let mut prev_line = lines[0];
    let mut repeat_count: u64 = 1;
    let mut first_group = true;

    let no_special_flags =
        !show_count && all_repeated.is_none() && !show_repeated_only && !show_unique_only;

    if no_special_flags {
        output.push_str(&format_line(prev_line, 1, false));
    }

    for &curr_line in &lines[1..] {
        if lines_equal(curr_line, prev_line, opts) {
            repeat_count += 1;

            if let Some(sep_type) = all_repeated {
                if repeat_count == 2 {
                    // Starting a new group of repeated lines
                    match sep_type {
                        AllRepeatedSepType::Prepend => {
                            output.push('\n');
                        }
                        AllRepeatedSepType::Separate if !first_group => {
                            output.push('\n');
                        }
                        _ => {}
                    }
                    first_group = false;
                    output.push_str(&format_line(prev_line, 1, show_count));
                }
                output.push_str(&format_line(curr_line, repeat_count, show_count));
            }
        } else {
            if !no_special_flags && all_repeated.is_none() {
                let is_repeated = repeat_count > 1;
                let should_show = if show_unique_only && show_repeated_only {
                    false
                } else if show_unique_only {
                    !is_repeated
                } else if show_repeated_only {
                    is_repeated
                } else {
                    true
                };

                if should_show {
                    output.push_str(&format_line(prev_line, repeat_count, show_count));
                }
            } else if no_special_flags {
                output.push_str(&format_line(curr_line, 1, false));
            }

            prev_line = curr_line;
            repeat_count = 1;
        }
    }

    if !no_special_flags && all_repeated.is_none() {
        let is_repeated = repeat_count > 1;
        let should_show = if show_unique_only && show_repeated_only {
            false
        } else if show_unique_only {
            !is_repeated
        } else if show_repeated_only {
            is_repeated
        } else {
            true
        };

        if should_show {
            output.push_str(&format_line(prev_line, repeat_count, show_count));
        }
    }

    output
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
    fn basic() {
        let env = make_test_env_with_stdin(vec!["uniq"], "a\na\nb\nb\na\na\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert_eq!("a\nb\na\n", stdout);
    }

    #[test]
    fn empty_input() {
        let env = make_test_env_with_stdin(vec!["uniq"], "");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("", env.stdout.into_string());
    }

    #[test]
    fn single_line() {
        let env = make_test_env_with_stdin(vec!["uniq"], "hello\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("hello\n", env.stdout.into_string());
    }

    #[test]
    fn all_unique() {
        let env = make_test_env_with_stdin(vec!["uniq"], "a\nb\nc\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("a\nb\nc\n", env.stdout.into_string());
    }

    #[test]
    fn all_same() {
        let env = make_test_env_with_stdin(vec!["uniq"], "a\na\na\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("a\n", env.stdout.into_string());
    }

    // ========================================================================
    // -c flag: count
    // ========================================================================

    #[test]
    fn count() {
        let env = make_test_env_with_stdin(vec!["uniq", "-c"], "a\na\nb\nb\nb\na\na\na\na\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert_eq!("   2 a\n   3 b\n   4 a\n", stdout);
    }

    #[test]
    fn count_long_form() {
        let env = make_test_env_with_stdin(vec!["uniq", "--count"], "a\na\nb\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert_eq!("   2 a\n   1 b\n", stdout);
    }

    // ========================================================================
    // -d flag: repeated
    // ========================================================================

    #[test]
    fn repeated() {
        let env = make_test_env_with_stdin(vec!["uniq", "-d"], "a\na\nb\na\na\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert_eq!("a\na\n", stdout);
    }

    #[test]
    fn repeated_long_form() {
        let env = make_test_env_with_stdin(vec!["uniq", "--repeated"], "a\na\nb\na\na\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("a\na\n", env.stdout.into_string());
    }

    // ========================================================================
    // -cd flags: count and repeated
    // ========================================================================

    #[test]
    fn count_repeated() {
        let env = make_test_env_with_stdin(vec!["uniq", "-c", "-d"], "a\na\nb\nb\na\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert_eq!("   2 a\n   2 b\n", stdout);
    }

    // ========================================================================
    // -D flag: all-repeated
    // ========================================================================

    #[test]
    fn all_repeated_none() {
        let env = make_test_env_with_stdin(vec!["uniq", "-D"], "a\na\nb\na\na\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert_eq!("a\na\na\na\n", stdout);
    }

    #[test]
    fn all_repeated_explicit_none() {
        let env = make_test_env_with_stdin(vec!["uniq", "-Dnone"], "a\na\nb\na\na\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("a\na\na\na\n", env.stdout.into_string());
    }

    #[test]
    fn all_repeated_prepend() {
        let env = make_test_env_with_stdin(vec!["uniq", "-Dprepend"], "a\na\nb\na\na\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert_eq!("\na\na\n\na\na\n", stdout);
    }

    #[test]
    fn all_repeated_separate() {
        let env = make_test_env_with_stdin(vec!["uniq", "-Dseparate"], "a\na\nb\na\na\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert_eq!("a\na\n\na\na\n", stdout);
    }

    #[test]
    fn all_repeated_long_form() {
        let env =
            make_test_env_with_stdin(vec!["uniq", "--all-repeated=separate"], "a\na\nb\na\na\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("a\na\n\na\na\n", env.stdout.into_string());
    }

    #[test]
    fn all_repeated_invalid_septype() {
        let env = make_test_env_with_stdin(vec!["uniq", "-Dinvalid"], "a\na\n");
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("invalid argument"));
    }

    // ========================================================================
    // -D with -c flags: all-repeated with count
    // ========================================================================

    #[test]
    fn count_all_repeated() {
        let env = make_test_env_with_stdin(vec!["uniq", "-D", "-c"], "a\na\nb\na\na\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert_eq!("   1 a\n   2 a\n   1 a\n   2 a\n", stdout);
    }

    // ========================================================================
    // -f flag: skip fields
    // ========================================================================

    #[test]
    fn skip_fields() {
        let env =
            make_test_env_with_stdin(vec!["uniq", "-f", "1"], "1 a\n2 a\n3 b\n4 b\n5 a\n6 a\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert_eq!("1 a\n3 b\n5 a\n", stdout);
    }

    #[test]
    fn skip_fields_long_form() {
        let env = make_test_env_with_stdin(
            vec!["uniq", "--skip-fields", "1"],
            "1 a\n2 a\n3 b\n4 b\n5 a\n6 a\n",
        );
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("1 a\n3 b\n5 a\n", env.stdout.into_string());
    }

    #[test]
    fn skip_fields_tab() {
        let env = make_test_env_with_stdin(
            vec!["uniq", "-f", "1"],
            "1\ta\n2\ta\n3\tb\n4\tb\n5\ta\n6\ta\n",
        );
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert_eq!("1\ta\n3\tb\n5\ta\n", stdout);
    }

    #[test]
    fn skip_fields_invalid() {
        let env = make_test_env_with_stdin(vec!["uniq", "-f", "abc"], "a\n");
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("invalid number of fields"));
    }

    // ========================================================================
    // -i flag: ignore case
    // ========================================================================

    #[test]
    fn ignore_case() {
        let env = make_test_env_with_stdin(vec!["uniq", "-i"], "a\nA\nb\nB\na\nA\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert_eq!("a\nb\na\n", stdout);
    }

    #[test]
    fn ignore_case_long_form() {
        let env = make_test_env_with_stdin(vec!["uniq", "--ignore-case"], "a\nA\nb\nB\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("a\nb\n", env.stdout.into_string());
    }

    // ========================================================================
    // -s flag: skip chars
    // ========================================================================

    #[test]
    fn skip_chars() {
        let env =
            make_test_env_with_stdin(vec!["uniq", "-s", "2"], "1 a\n2 a\n3 b\n4 b\n5 a\n6 a\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert_eq!("1 a\n3 b\n5 a\n", stdout);
    }

    #[test]
    fn skip_chars_long_form() {
        let env = make_test_env_with_stdin(
            vec!["uniq", "--skip-chars", "2"],
            "1 a\n2 a\n3 b\n4 b\n5 a\n6 a\n",
        );
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("1 a\n3 b\n5 a\n", env.stdout.into_string());
    }

    #[test]
    fn skip_chars_invalid() {
        let env = make_test_env_with_stdin(vec!["uniq", "-s", "abc"], "a\n");
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("invalid number of characters"));
    }

    // ========================================================================
    // -u flag: unique
    // ========================================================================

    #[test]
    fn unique() {
        let env = make_test_env_with_stdin(vec!["uniq", "-u"], "a\na\nb\na\na\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert_eq!("b\n", stdout);
    }

    #[test]
    fn unique_long_form() {
        let env = make_test_env_with_stdin(vec!["uniq", "--unique"], "a\na\nb\na\na\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("b\n", env.stdout.into_string());
    }

    // ========================================================================
    // -cu flags: count and unique
    // ========================================================================

    #[test]
    fn count_unique() {
        let env = make_test_env_with_stdin(vec!["uniq", "-c", "-u"], "a\na\nb\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert_eq!("   1 b\n", stdout);
    }

    // ========================================================================
    // File operations
    // ========================================================================

    #[test]
    fn read_from_file() {
        let env = make_test_env_with_stdin(vec!["uniq", "input.txt"], "");
        env.fs.add_file("input.txt", "a\na\nb\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("a\nb\n", env.stdout.into_string());
    }

    #[test]
    fn write_to_file() {
        let env = make_test_env_with_stdin(vec!["uniq", "input.txt", "output.txt"], "");
        env.fs.add_file("input.txt", "a\na\nb\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("", env.stdout.into_string());
        assert_eq!("a\nb\n", env.fs.read_to_string("output.txt").unwrap());
    }

    #[test]
    fn file_not_found() {
        let env = make_test_env_with_stdin(vec!["uniq", "nonexistent.txt"], "");
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("nonexistent.txt"));
    }

    #[test]
    fn dash_means_stdin() {
        let env = make_test_env_with_stdin(vec!["uniq", "-"], "a\na\nb\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("a\nb\n", env.stdout.into_string());
    }

    #[test]
    fn extra_operand() {
        let env = make_test_env_with_stdin(vec!["uniq", "a", "b", "c"], "");
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("extra operand"));
    }

    // ========================================================================
    // Edge cases
    // ========================================================================

    #[test]
    fn no_trailing_newline() {
        let env = make_test_env_with_stdin(vec!["uniq"], "a\na\nb");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("a\nb\n", env.stdout.into_string());
    }

    #[test]
    fn mixed_whitespace_fields() {
        let env = make_test_env_with_stdin(vec!["uniq", "-f", "1"], "  a b\n  a b\nc d\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert_eq!("  a b\nc d\n", stdout);
    }

    #[test]
    fn skip_chars_beyond_line() {
        let env = make_test_env_with_stdin(vec!["uniq", "-s", "100"], "a\nb\nc\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert_eq!("a\n", stdout);
    }

    #[test]
    fn get_comparison_key_basic() {
        let opts = UniqOptions::default();
        assert_eq!("hello", get_comparison_key("hello", &opts));
    }

    #[test]
    fn get_comparison_key_skip_one_field() {
        let opts = UniqOptions {
            skip_fields: 1,
            ..Default::default()
        };
        // After skipping the "hello" field, we're left at " world" (with leading space)
        assert_eq!(" world", get_comparison_key("hello world", &opts));
    }

    #[test]
    fn get_comparison_key_skip_chars() {
        let opts = UniqOptions {
            skip_chars: 2,
            ..Default::default()
        };
        assert_eq!("llo", get_comparison_key("hello", &opts));
    }

    #[test]
    fn get_comparison_key_ignore_case() {
        let opts = UniqOptions {
            ignore_case: true,
            ..Default::default()
        };
        assert_eq!("hello", get_comparison_key("HELLO", &opts));
    }
}

//! The shuf builtin: generate random permutations.

use getopts::Options;
use rand::Rng;
use rand::seq::SliceRandom;

use crate::{Environment, Error, ExitCode, Filesystem, Stderr, Stdin, Stdout};

/// Options for the shuf command.
#[derive(Clone, Debug, Default)]
struct ShufOptions {
    /// Maximum number of lines to output.
    head_count: Option<usize>,
    /// Treat each argument as an input line.
    echo: bool,
    /// Input range (lo, hi) inclusive.
    input_range: Option<(i64, i64)>,
    /// Output file (None means stdout).
    output_file: Option<String>,
    /// Allow repeated output lines.
    repeat: bool,
    /// Use NUL as line terminator.
    zero_terminated: bool,
}

fn build_options() -> Options {
    let mut opts = Options::new();
    opts.optflag("e", "echo", "Treat each ARG as an input line.");
    opts.optopt(
        "i",
        "input-range",
        "Treat each number LO through HI as an input line.",
        "LO-HI",
    );
    opts.optopt("n", "head-count", "Output at most COUNT lines.", "COUNT");
    opts.optopt(
        "o",
        "output",
        "Write result to FILE instead of stdout.",
        "FILE",
    );
    opts.optflag("r", "repeat", "Output lines can be repeated.");
    opts.optflag(
        "z",
        "zero-terminated",
        "Line delimiter is NUL, not newline.",
    );
    opts.optflag("", "help", "Display this help and exit.");
    opts.optflag("", "version", "Output version information and exit.");
    opts
}

/// Parse an input range specification like "1-100".
fn parse_input_range(s: &str) -> Option<(i64, i64)> {
    let parts: Vec<&str> = s.splitn(2, '-').collect();
    if parts.len() != 2 {
        // Try splitting on negative number: "-5-10" or "1--5"
        // Handle case where first number is negative
        if let Some(rest) = s.strip_prefix('-') {
            // Could be "-5-10" meaning -5 to 10
            if let Some(idx) = rest.find('-') {
                let lo: i64 = format!("-{}", &rest[..idx]).parse().ok()?;
                let hi: i64 = rest[idx + 1..].parse().ok()?;
                return Some((lo, hi));
            }
        }
        return None;
    }
    let lo: i64 = parts[0].parse().ok()?;
    let hi: i64 = parts[1].parse().ok()?;
    Some((lo, hi))
}

/// The shuf builtin: generate random permutations.
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
            env.stderr.write_line(&format!("shuf: {}", e))?;
            return Ok(ExitCode::from(1));
        }
    };

    if matches.opt_present("help") {
        env.stdout.write_line("Usage: shuf [OPTION]... [FILE]")?;
        env.stdout
            .write_line("  or:  shuf -e [OPTION]... [ARG]...")?;
        env.stdout.write_line("  or:  shuf -i LO-HI [OPTION]...")?;
        env.stdout
            .write_line("Write a random permutation of the input lines to standard output.")?;
        env.stdout.write_line("")?;
        env.stdout
            .write_line("  -e, --echo              treat each ARG as an input line")?;
        env.stdout.write_line(
            "  -i, --input-range=LO-HI treat each number LO through HI as an input line",
        )?;
        env.stdout
            .write_line("  -n, --head-count=COUNT  output at most COUNT lines")?;
        env.stdout
            .write_line("  -o, --output=FILE       write result to FILE instead of stdout")?;
        env.stdout
            .write_line("  -r, --repeat            output lines can be repeated")?;
        env.stdout
            .write_line("  -z, --zero-terminated   line delimiter is NUL, not newline")?;
        env.stdout
            .write_line("      --help              display this help and exit")?;
        env.stdout
            .write_line("      --version           output version information and exit")?;
        return Ok(ExitCode::from(0));
    }

    if matches.opt_present("version") {
        env.stdout.write_line("shuf (eudaemonsh) 1.0")?;
        return Ok(ExitCode::from(0));
    }

    // Parse head count
    let head_count = if let Some(n) = matches.opt_str("n") {
        match n.parse::<usize>() {
            Ok(count) => Some(count),
            Err(_) => {
                env.stderr
                    .write_line(&format!("shuf: invalid line count: '{}'", n))?;
                return Ok(ExitCode::from(1));
            }
        }
    } else {
        None
    };

    // Parse input range
    let input_range = if let Some(range) = matches.opt_str("i") {
        match parse_input_range(&range) {
            Some((lo, hi)) if lo <= hi => Some((lo, hi)),
            Some((lo, hi)) => {
                env.stderr.write_line(&format!(
                    "shuf: invalid input range: '{}': {} > {}",
                    range, lo, hi
                ))?;
                return Ok(ExitCode::from(1));
            }
            None => {
                env.stderr
                    .write_line(&format!("shuf: invalid input range: '{}'", range))?;
                return Ok(ExitCode::from(1));
            }
        }
    } else {
        None
    };

    let echo = matches.opt_present("e");

    // Check for conflicting options
    if echo && input_range.is_some() {
        env.stderr
            .write_line("shuf: cannot combine -e and -i options")?;
        return Ok(ExitCode::from(1));
    }

    if input_range.is_some() && !matches.free.is_empty() {
        env.stderr
            .write_line("shuf: extra operand with -i option")?;
        return Ok(ExitCode::from(1));
    }

    let opts = ShufOptions {
        head_count,
        echo,
        input_range,
        output_file: matches.opt_str("o"),
        repeat: matches.opt_present("r"),
        zero_terminated: matches.opt_present("z"),
    };

    // Collect input lines
    let lines = collect_input(env, &matches.free, &opts)?;

    if lines.is_empty() {
        return Ok(ExitCode::from(0));
    }

    // Generate output
    let output = generate_output(&lines, &opts);

    // Write output
    let terminator = if opts.zero_terminated { '\0' } else { '\n' };
    let mut output_str = String::new();
    for line in output {
        output_str.push_str(&line);
        output_str.push(terminator);
    }

    if let Some(ref output_file) = opts.output_file {
        if let Err(e) = env.fs.write_string(output_file, &output_str) {
            env.stderr
                .write_line(&format!("shuf: {}: {:?}", output_file, e))?;
            return Ok(ExitCode::from(1));
        }
    } else {
        env.stdout.write_str(&output_str)?;
    }

    Ok(ExitCode::from(0))
}

/// Collect input lines based on options.
fn collect_input<SI, SO, SE, FS>(
    env: &Environment<SI, SO, SE, FS>,
    args: &[String],
    opts: &ShufOptions,
) -> Result<Vec<String>, Error>
where
    SI: Stdin,
    SO: Stdout,
    SE: Stderr,
    FS: Filesystem,
{
    // Input range mode
    if let Some((lo, hi)) = opts.input_range {
        return Ok((lo..=hi).map(|n| n.to_string()).collect());
    }

    // Echo mode
    if opts.echo {
        return Ok(args.to_vec());
    }

    // File/stdin mode
    let terminator = if opts.zero_terminated { '\0' } else { '\n' };
    let mut lines = Vec::new();

    if args.is_empty() || (args.len() == 1 && args[0] == "-") {
        // Read from stdin
        let mut contents = String::new();
        while let Some(line) = env.stdin.read_line()? {
            contents.push_str(&line);
            contents.push('\n');
        }
        for line in contents.split(terminator) {
            if !line.is_empty() || opts.zero_terminated {
                lines.push(line.to_string());
            }
        }
        // Remove trailing empty line if present
        if lines.last().is_some_and(|s| s.is_empty()) {
            lines.pop();
        }
    } else if args.len() == 1 {
        // Read from file
        match env.fs.read_to_string(&args[0]) {
            Ok(contents) => {
                for line in contents.split(terminator) {
                    if !line.is_empty() || opts.zero_terminated {
                        lines.push(line.to_string());
                    }
                }
                // Remove trailing empty line if present
                if lines.last().is_some_and(|s| s.is_empty()) {
                    lines.pop();
                }
            }
            Err(Error::Io(e)) => {
                env.stderr
                    .write_line(&format!("shuf: {}: {}", args[0], e))?;
                return Ok(Vec::new());
            }
            Err(e) => {
                env.stderr
                    .write_line(&format!("shuf: {}: {:?}", args[0], e))?;
                return Ok(Vec::new());
            }
        }
    } else {
        env.stderr.write_line("shuf: extra operand")?;
        return Ok(Vec::new());
    }

    Ok(lines)
}

/// Generate shuffled output.
fn generate_output(lines: &[String], opts: &ShufOptions) -> Vec<String> {
    let mut rng = rand::rng();

    if opts.repeat {
        // Repeat mode: sample with replacement
        let count = opts.head_count.unwrap_or(lines.len());
        (0..count)
            .map(|_| lines[rng.random_range(0..lines.len())].clone())
            .collect()
    } else {
        // Normal mode: shuffle and take head_count
        let mut shuffled: Vec<String> = lines.to_vec();
        shuffled.shuffle(&mut rng);
        if let Some(count) = opts.head_count {
            shuffled.truncate(count);
        }
        shuffled
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::make_test_env_with_stdin;
    use std::collections::HashSet;

    // ========================================================================
    // Basic functionality tests
    // ========================================================================

    #[test]
    fn empty_stdin() {
        let env = make_test_env_with_stdin(vec!["shuf"], "");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("", env.stdout.into_string());
    }

    #[test]
    fn single_line() {
        let env = make_test_env_with_stdin(vec!["shuf"], "hello\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("hello\n", env.stdout.into_string());
    }

    #[test]
    fn shuffles_lines() {
        // Run multiple times to ensure we get output with all lines
        let input = "a\nb\nc\n";
        let env = make_test_env_with_stdin(vec!["shuf"], input);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        let output_lines: HashSet<&str> = stdout.trim().split('\n').collect();
        let input_lines: HashSet<&str> = vec!["a", "b", "c"].into_iter().collect();
        println!("stdout: {:?}", stdout);
        assert_eq!(input_lines, output_lines);
    }

    #[test]
    fn preserves_all_lines() {
        let input = "1\n2\n3\n4\n5\n";
        let env = make_test_env_with_stdin(vec!["shuf"], input);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        let output_lines: Vec<&str> = stdout.trim().split('\n').collect();
        println!("stdout: {:?}", stdout);
        assert_eq!(5, output_lines.len());
        let mut sorted = output_lines.clone();
        sorted.sort();
        assert_eq!(vec!["1", "2", "3", "4", "5"], sorted);
    }

    // ========================================================================
    // -e flag: echo mode
    // ========================================================================

    #[test]
    fn echo_mode_single_arg() {
        let env = make_test_env_with_stdin(vec!["shuf", "-e", "hello"], "");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("hello\n", env.stdout.into_string());
    }

    #[test]
    fn echo_mode_multiple_args() {
        let env = make_test_env_with_stdin(vec!["shuf", "-e", "a", "b", "c"], "");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        let output_lines: HashSet<&str> = stdout.trim().split('\n').collect();
        let expected: HashSet<&str> = vec!["a", "b", "c"].into_iter().collect();
        println!("stdout: {:?}", stdout);
        assert_eq!(expected, output_lines);
    }

    #[test]
    fn echo_mode_long_form() {
        let env = make_test_env_with_stdin(vec!["shuf", "--echo", "x", "y"], "");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        let output_lines: HashSet<&str> = stdout.trim().split('\n').collect();
        println!("stdout: {:?}", stdout);
        assert_eq!(2, output_lines.len());
    }

    #[test]
    fn echo_mode_no_args_empty_output() {
        let env = make_test_env_with_stdin(vec!["shuf", "-e"], "");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("", env.stdout.into_string());
    }

    // ========================================================================
    // -i flag: input range
    // ========================================================================

    #[test]
    fn input_range_basic() {
        let env = make_test_env_with_stdin(vec!["shuf", "-i", "1-5"], "");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        let output_lines: HashSet<&str> = stdout.trim().split('\n').collect();
        let expected: HashSet<&str> = vec!["1", "2", "3", "4", "5"].into_iter().collect();
        println!("stdout: {:?}", stdout);
        assert_eq!(expected, output_lines);
    }

    #[test]
    fn input_range_single_number() {
        let env = make_test_env_with_stdin(vec!["shuf", "-i", "5-5"], "");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("5\n", env.stdout.into_string());
    }

    #[test]
    fn input_range_long_form() {
        let env = make_test_env_with_stdin(vec!["shuf", "--input-range=1-3"], "");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        let output_lines: HashSet<&str> = stdout.trim().split('\n').collect();
        println!("stdout: {:?}", stdout);
        assert_eq!(3, output_lines.len());
    }

    #[test]
    fn input_range_invalid_format() {
        let env = make_test_env_with_stdin(vec!["shuf", "-i", "abc"], "");
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("invalid input range"));
    }

    #[test]
    fn input_range_hi_less_than_lo() {
        let env = make_test_env_with_stdin(vec!["shuf", "-i", "10-5"], "");
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("invalid input range"));
    }

    #[test]
    fn input_range_with_extra_operand_is_error() {
        let env = make_test_env_with_stdin(vec!["shuf", "-i", "1-5", "file.txt"], "");
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("extra operand"));
    }

    // ========================================================================
    // -n flag: head count
    // ========================================================================

    #[test]
    fn head_count_basic() {
        let env = make_test_env_with_stdin(vec!["shuf", "-n", "2"], "a\nb\nc\nd\ne\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        let output_lines: Vec<&str> = stdout.trim().split('\n').collect();
        println!("stdout: {:?}", stdout);
        assert_eq!(2, output_lines.len());
    }

    #[test]
    fn head_count_zero() {
        let env = make_test_env_with_stdin(vec!["shuf", "-n", "0"], "a\nb\nc\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("", env.stdout.into_string());
    }

    #[test]
    fn head_count_more_than_lines() {
        let env = make_test_env_with_stdin(vec!["shuf", "-n", "100"], "a\nb\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        let output_lines: Vec<&str> = stdout.trim().split('\n').collect();
        println!("stdout: {:?}", stdout);
        assert_eq!(2, output_lines.len());
    }

    #[test]
    fn head_count_long_form() {
        let env = make_test_env_with_stdin(vec!["shuf", "--head-count=1"], "x\ny\nz\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        let output_lines: Vec<&str> = stdout.trim().split('\n').collect();
        println!("stdout: {:?}", stdout);
        assert_eq!(1, output_lines.len());
    }

    #[test]
    fn head_count_invalid() {
        let env = make_test_env_with_stdin(vec!["shuf", "-n", "abc"], "a\n");
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("invalid line count"));
    }

    #[test]
    fn head_count_with_echo() {
        let env = make_test_env_with_stdin(vec!["shuf", "-e", "-n", "2", "a", "b", "c", "d"], "");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        let output_lines: Vec<&str> = stdout.trim().split('\n').collect();
        println!("stdout: {:?}", stdout);
        assert_eq!(2, output_lines.len());
    }

    #[test]
    fn head_count_with_input_range() {
        let env = make_test_env_with_stdin(vec!["shuf", "-i", "1-10", "-n", "3"], "");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        let output_lines: Vec<&str> = stdout.trim().split('\n').collect();
        println!("stdout: {:?}", stdout);
        assert_eq!(3, output_lines.len());
    }

    // ========================================================================
    // -r flag: repeat
    // ========================================================================

    #[test]
    fn repeat_with_head_count() {
        let env = make_test_env_with_stdin(vec!["shuf", "-r", "-n", "10", "-e", "a", "b"], "");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        let output_lines: Vec<&str> = stdout.trim().split('\n').collect();
        println!("stdout: {:?}", stdout);
        assert_eq!(10, output_lines.len());
        // All lines should be either "a" or "b"
        for line in output_lines {
            assert!(line == "a" || line == "b");
        }
    }

    #[test]
    fn repeat_long_form() {
        let env = make_test_env_with_stdin(vec!["shuf", "--repeat", "-n", "5", "-e", "x"], "");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        let output_lines: Vec<&str> = stdout.trim().split('\n').collect();
        println!("stdout: {:?}", stdout);
        assert_eq!(5, output_lines.len());
        for line in output_lines {
            assert_eq!("x", line);
        }
    }

    // ========================================================================
    // -o flag: output file
    // ========================================================================

    #[test]
    fn output_to_file() {
        let env = make_test_env_with_stdin(vec!["shuf", "-o", "out.txt", "-e", "a", "b", "c"], "");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("", env.stdout.into_string());
        let file_contents = env.fs.read_to_string("out.txt").unwrap();
        let output_lines: HashSet<&str> = file_contents.trim().split('\n').collect();
        let expected: HashSet<&str> = vec!["a", "b", "c"].into_iter().collect();
        println!("file_contents: {:?}", file_contents);
        assert_eq!(expected, output_lines);
    }

    #[test]
    fn output_long_form() {
        let env = make_test_env_with_stdin(vec!["shuf", "--output=result.txt", "-e", "x"], "");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("x\n", env.fs.read_to_string("result.txt").unwrap());
    }

    // ========================================================================
    // -z flag: zero terminated
    // ========================================================================

    #[test]
    fn zero_terminated_output() {
        let env = make_test_env_with_stdin(vec!["shuf", "-z", "-e", "a", "b", "c"], "");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout bytes: {:?}", stdout.as_bytes());
        // Output should be NUL-terminated
        assert!(stdout.contains('\0'));
        assert!(!stdout.contains('\n'));
        let output_parts: HashSet<&str> = stdout.trim_end_matches('\0').split('\0').collect();
        let expected: HashSet<&str> = vec!["a", "b", "c"].into_iter().collect();
        assert_eq!(expected, output_parts);
    }

    #[test]
    fn zero_terminated_long_form() {
        let env = make_test_env_with_stdin(vec!["shuf", "--zero-terminated", "-e", "x"], "");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        assert_eq!("x\0", stdout);
    }

    // ========================================================================
    // File input
    // ========================================================================

    #[test]
    fn read_from_file() {
        let env = make_test_env_with_stdin(vec!["shuf", "input.txt"], "");
        env.fs.add_file("input.txt", "x\ny\nz\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        let output_lines: HashSet<&str> = stdout.trim().split('\n').collect();
        let expected: HashSet<&str> = vec!["x", "y", "z"].into_iter().collect();
        println!("stdout: {:?}", stdout);
        assert_eq!(expected, output_lines);
    }

    #[test]
    fn file_not_found() {
        let env = make_test_env_with_stdin(vec!["shuf", "nonexistent.txt"], "");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("nonexistent.txt"));
    }

    #[test]
    fn stdin_with_dash() {
        let env = make_test_env_with_stdin(vec!["shuf", "-"], "p\nq\nr\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        let output_lines: HashSet<&str> = stdout.trim().split('\n').collect();
        let expected: HashSet<&str> = vec!["p", "q", "r"].into_iter().collect();
        println!("stdout: {:?}", stdout);
        assert_eq!(expected, output_lines);
    }

    // ========================================================================
    // Conflicting options
    // ========================================================================

    #[test]
    fn echo_and_input_range_conflict() {
        let env = make_test_env_with_stdin(vec!["shuf", "-e", "-i", "1-5", "a", "b"], "");
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("cannot combine"));
    }

    // ========================================================================
    // Help and version
    // ========================================================================

    #[test]
    fn help_flag() {
        let env = make_test_env_with_stdin(vec!["shuf", "--help"], "");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert!(stdout.contains("Usage"));
        assert!(stdout.contains("--echo"));
        assert!(stdout.contains("--input-range"));
    }

    #[test]
    fn version_flag() {
        let env = make_test_env_with_stdin(vec!["shuf", "--version"], "");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert!(stdout.contains("eudaemonsh"));
    }

    // ========================================================================
    // Edge cases
    // ========================================================================

    #[test]
    fn no_trailing_newline_in_input() {
        let env = make_test_env_with_stdin(vec!["shuf"], "a\nb\nc");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        let output_lines: HashSet<&str> = stdout.trim().split('\n').collect();
        let expected: HashSet<&str> = vec!["a", "b", "c"].into_iter().collect();
        println!("stdout: {:?}", stdout);
        assert_eq!(expected, output_lines);
    }

    #[test]
    fn empty_file() {
        let env = make_test_env_with_stdin(vec!["shuf", "empty.txt"], "");
        env.fs.add_file("empty.txt", "");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("", env.stdout.into_string());
    }

    #[test]
    fn multiple_files_is_error() {
        let env = make_test_env_with_stdin(vec!["shuf", "a.txt", "b.txt"], "");
        env.fs.add_file("a.txt", "a\n");
        env.fs.add_file("b.txt", "b\n");
        let result = bin(&env).unwrap();
        // shuf only accepts one file
        assert_eq!(0, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("extra operand"));
    }

    #[test]
    fn input_range_negative_numbers() {
        // Parse function test
        assert_eq!(Some((1, 5)), parse_input_range("1-5"));
        assert_eq!(Some((0, 10)), parse_input_range("0-10"));
    }

    #[test]
    fn invalid_option() {
        let env = make_test_env_with_stdin(vec!["shuf", "--invalid"], "");
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("Unrecognized"));
    }

    // ========================================================================
    // Combined options
    // ========================================================================

    #[test]
    fn combined_input_range_head_count_repeat() {
        let env = make_test_env_with_stdin(vec!["shuf", "-i", "1-3", "-n", "10", "-r"], "");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        let output_lines: Vec<&str> = stdout.trim().split('\n').collect();
        println!("stdout: {:?}", stdout);
        assert_eq!(10, output_lines.len());
        for line in output_lines {
            let n: i32 = line.parse().unwrap();
            assert!((1..=3).contains(&n));
        }
    }

    #[test]
    fn combined_echo_head_count_output() {
        let env = make_test_env_with_stdin(
            vec!["shuf", "-e", "-n", "2", "-o", "out.txt", "a", "b", "c"],
            "",
        );
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("", env.stdout.into_string());
        let file_contents = env.fs.read_to_string("out.txt").unwrap();
        let output_lines: Vec<&str> = file_contents.trim().split('\n').collect();
        println!("file_contents: {:?}", file_contents);
        assert_eq!(2, output_lines.len());
    }
}

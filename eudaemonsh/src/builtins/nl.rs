//! The nl builtin: line numbering filter.

use getopts::Options;
use regex::Regex;

use crate::{Environment, Error, ExitCode, Filesystem, FsError, StdioIn, StdioOut};

/// The type of line numbering to apply.
#[derive(Clone, Debug, Default)]
enum NumberingType {
    /// Number all lines.
    All,
    /// Number only non-empty lines.
    #[default]
    NonEmpty,
    /// No line numbering.
    None,
    /// Number lines matching the given regex.
    Regex(Regex),
}

/// The format for line numbers.
#[derive(Clone, Copy, Debug, Default)]
enum NumberFormat {
    /// Left justified, leading zeros suppressed.
    Ln,
    /// Right justified, leading zeros suppressed.
    #[default]
    Rn,
    /// Right justified, leading zeros kept.
    Rz,
}

/// Logical page sections.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Section {
    Header,
    Body,
    Footer,
}

/// Options for the nl command.
#[derive(Clone, Debug)]
struct NlOptions {
    /// Numbering type for body lines.
    body_type: NumberingType,
    /// Numbering type for footer lines.
    footer_type: NumberingType,
    /// Numbering type for header lines.
    header_type: NumberingType,
    /// Delimiter characters (two chars).
    delimiter: (char, char),
    /// Increment value.
    increment: i64,
    /// Number of adjacent blank lines to consider as one (for -b a).
    blank_join: u32,
    /// Line number format.
    format: NumberFormat,
    /// Separator between line number and text.
    separator: String,
    /// Starting line number.
    start_num: i64,
    /// Width of line number field.
    width: usize,
    /// Whether to restart numbering at page delimiters.
    restart: bool,
}

impl Default for NlOptions {
    fn default() -> Self {
        Self {
            body_type: NumberingType::NonEmpty,
            footer_type: NumberingType::None,
            header_type: NumberingType::None,
            delimiter: ('\\', ':'),
            increment: 1,
            blank_join: 1,
            format: NumberFormat::Rn,
            separator: "\t".to_string(),
            start_num: 1,
            width: 6,
            restart: true,
        }
    }
}

/// State for processing nl output.
struct NlState {
    line_number: i64,
    section: Section,
    adjacent_blanks: u32,
}

impl NlState {
    fn new(start: i64) -> Self {
        Self {
            line_number: start,
            section: Section::Body,
            adjacent_blanks: 0,
        }
    }
}

fn build_options() -> Options {
    let mut opts = Options::new();
    opts.optopt(
        "b",
        "",
        "Specify which body lines are numbered (a, t, n, or pexpr)",
        "TYPE",
    );
    opts.optopt(
        "d",
        "",
        "Specify the delimiter characters for logical page sections",
        "DELIM",
    );
    opts.optopt(
        "f",
        "",
        "Specify which footer lines are numbered (a, t, n, or pexpr)",
        "TYPE",
    );
    opts.optopt(
        "h",
        "",
        "Specify which header lines are numbered (a, t, n, or pexpr)",
        "TYPE",
    );
    opts.optopt("i", "", "Specify the increment value", "INCR");
    opts.optopt(
        "l",
        "",
        "Number of adjacent blank lines to be considered as one",
        "NUM",
    );
    opts.optopt(
        "n",
        "",
        "Specify the line numbering format (ln, rn, or rz)",
        "FORMAT",
    );
    opts.optflag(
        "p",
        "",
        "Do not restart numbering at logical page delimiters",
    );
    opts.optopt(
        "s",
        "",
        "Specify the separator between line number and text",
        "SEP",
    );
    opts.optopt("v", "", "Specify the initial line number value", "STARTNUM");
    opts.optopt(
        "w",
        "",
        "Specify the width of the line number field",
        "WIDTH",
    );
    opts
}

/// Parse a numbering type specification.
fn parse_numbering_type(spec: &str, section_name: &str) -> Result<NumberingType, String> {
    if spec.is_empty() {
        return Err(format!("nl: empty {} line numbering type", section_name));
    }

    match spec.chars().next().unwrap() {
        'a' => Ok(NumberingType::All),
        't' => Ok(NumberingType::NonEmpty),
        'n' => Ok(NumberingType::None),
        'p' => {
            let pattern = &spec[1..];
            match Regex::new(pattern) {
                Ok(re) => Ok(NumberingType::Regex(re)),
                Err(e) => Err(format!("nl: {} expr: {} -- {}", section_name, e, pattern)),
            }
        }
        _ => Err(format!(
            "nl: illegal {} line numbering type -- {}",
            section_name, spec
        )),
    }
}

/// Parse the format specification.
fn parse_format(spec: &str) -> Result<NumberFormat, String> {
    match spec {
        "ln" => Ok(NumberFormat::Ln),
        "rn" => Ok(NumberFormat::Rn),
        "rz" => Ok(NumberFormat::Rz),
        _ => Err(format!("nl: illegal format -- {}", spec)),
    }
}

/// The nl builtin: number lines of files.
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
            env.stderr.write_line(&format!("nl: {}", e))?;
            return Ok(ExitCode::from(1));
        }
    };

    let mut nl_opts = NlOptions::default();

    // Parse -b (body type)
    if let Some(spec) = matches.opt_str("b") {
        match parse_numbering_type(&spec, "body") {
            Ok(t) => nl_opts.body_type = t,
            Err(e) => {
                env.stderr.write_line(&e)?;
                return Ok(ExitCode::from(1));
            }
        }
    }

    // Parse -f (footer type)
    if let Some(spec) = matches.opt_str("f") {
        match parse_numbering_type(&spec, "footer") {
            Ok(t) => nl_opts.footer_type = t,
            Err(e) => {
                env.stderr.write_line(&e)?;
                return Ok(ExitCode::from(1));
            }
        }
    }

    // Parse -h (header type)
    if let Some(spec) = matches.opt_str("h") {
        match parse_numbering_type(&spec, "header") {
            Ok(t) => nl_opts.header_type = t,
            Err(e) => {
                env.stderr.write_line(&e)?;
                return Ok(ExitCode::from(1));
            }
        }
    }

    // Parse -d (delimiter)
    if let Some(delim) = matches.opt_str("d") {
        let chars: Vec<char> = delim.chars().collect();
        if chars.is_empty() {
            // Empty delimiter - keep defaults
        } else if chars.len() == 1 {
            nl_opts.delimiter.0 = chars[0];
        } else if chars.len() == 2 {
            nl_opts.delimiter = (chars[0], chars[1]);
        } else {
            env.stderr
                .write_line(&format!("nl: invalid delim argument -- {}", delim))?;
            return Ok(ExitCode::from(1));
        }
    }

    // Parse -i (increment)
    if let Some(incr) = matches.opt_str("i") {
        match incr.parse::<i64>() {
            Ok(n) => nl_opts.increment = n,
            Err(_) => {
                env.stderr
                    .write_line(&format!("nl: invalid incr argument -- {}", incr))?;
                return Ok(ExitCode::from(1));
            }
        }
    }

    // Parse -l (blank join)
    if let Some(num) = matches.opt_str("l") {
        match num.parse::<u32>() {
            Ok(n) => nl_opts.blank_join = n,
            Err(_) => {
                env.stderr
                    .write_line(&format!("nl: invalid num argument -- {}", num))?;
                return Ok(ExitCode::from(1));
            }
        }
    }

    // Parse -n (format)
    if let Some(fmt) = matches.opt_str("n") {
        match parse_format(&fmt) {
            Ok(f) => nl_opts.format = f,
            Err(e) => {
                env.stderr.write_line(&e)?;
                return Ok(ExitCode::from(1));
            }
        }
    }

    // Parse -p (no restart)
    if matches.opt_present("p") {
        nl_opts.restart = false;
    }

    // Parse -s (separator)
    if let Some(sep) = matches.opt_str("s") {
        nl_opts.separator = sep;
    }

    // Parse -v (start number)
    if let Some(start) = matches.opt_str("v") {
        match start.parse::<i64>() {
            Ok(n) => nl_opts.start_num = n,
            Err(_) => {
                env.stderr
                    .write_line(&format!("nl: invalid startnum value -- {}", start))?;
                return Ok(ExitCode::from(1));
            }
        }
    }

    // Parse -w (width)
    if let Some(width) = matches.opt_str("w") {
        match width.parse::<usize>() {
            Ok(n) if n > 0 => nl_opts.width = n,
            Ok(_) => {
                env.stderr.write_line("nl: width argument must be > 0")?;
                return Ok(ExitCode::from(1));
            }
            Err(_) => {
                env.stderr
                    .write_line(&format!("nl: invalid width value -- {}", width))?;
                return Ok(ExitCode::from(1));
            }
        }
    }

    let mut state = NlState::new(nl_opts.start_num);
    let mut exit_code: i8 = 0;

    if matches.free.is_empty() || (matches.free.len() == 1 && matches.free[0] == "-") {
        nl_stdin(env, &nl_opts, &mut state)?;
    } else {
        for file in &matches.free {
            if file == "-" {
                nl_stdin(env, &nl_opts, &mut state)?;
            } else if let Err(code) = nl_file(env, file, &nl_opts, &mut state) {
                exit_code = code;
            }
        }
    }

    Ok(ExitCode::from(exit_code))
}

/// Process stdin through nl.
fn nl_stdin<SI, SO, SE, FS>(
    env: &Environment<SI, SO, SE, FS>,
    opts: &NlOptions,
    state: &mut NlState,
) -> Result<(), Error>
where
    SI: StdioIn,
    SO: StdioOut,
    SE: StdioOut,
    FS: Filesystem,
{
    while let Some(line) = env.stdin.read_line()? {
        process_line(env, &line, opts, state)?;
    }
    Ok(())
}

/// Process a file through nl.
fn nl_file<SI, SO, SE, FS>(
    env: &Environment<SI, SO, SE, FS>,
    path: &str,
    opts: &NlOptions,
    state: &mut NlState,
) -> Result<(), i8>
where
    SI: StdioIn,
    SO: StdioOut,
    SE: StdioOut,
    FS: Filesystem,
{
    match env.fs.read_to_string(path) {
        Ok(contents) => {
            for line in contents.lines() {
                if process_line(env, line, opts, state).is_err() {
                    return Err(1);
                }
            }
            Ok(())
        }
        Err(FsError::Io(e)) => {
            let _ = env.stderr.write_line(&format!("nl: {}: {}", path, e));
            Err(1)
        }
    }
}

/// Check if a line is a section delimiter and return the new section if so.
fn check_section_delimiter(line: &str, delimiter: (char, char)) -> Option<Section> {
    let delim_str: String = [delimiter.0, delimiter.1].iter().collect();

    // Check for header delimiter (three delimiter sequences)
    let header_delim = format!("{}{}{}", delim_str, delim_str, delim_str);
    if line == header_delim {
        return Some(Section::Header);
    }

    // Check for body delimiter (two delimiter sequences)
    let body_delim = format!("{}{}", delim_str, delim_str);
    if line == body_delim {
        return Some(Section::Body);
    }

    // Check for footer delimiter (one delimiter sequence)
    if line == delim_str {
        return Some(Section::Footer);
    }

    None
}

/// Determine if a line should be numbered based on the numbering type.
fn should_number(
    line: &str,
    numbering_type: &NumberingType,
    adjacent_blanks: u32,
    blank_join: u32,
) -> bool {
    match numbering_type {
        NumberingType::All => {
            // For blank lines with -b a, only number every blank_join-th blank
            if line.is_empty() {
                adjacent_blanks >= blank_join
            } else {
                true
            }
        }
        NumberingType::NonEmpty => !line.is_empty(),
        NumberingType::None => false,
        NumberingType::Regex(re) => re.is_match(line),
    }
}

/// Format a line number according to the options.
fn format_line_number(num: i64, width: usize, format: NumberFormat) -> String {
    match format {
        NumberFormat::Ln => format!("{:<width$}", num, width = width),
        NumberFormat::Rn => format!("{:>width$}", num, width = width),
        NumberFormat::Rz => format!("{:0>width$}", num, width = width),
    }
}

/// Process a single line.
fn process_line<SI, SO, SE, FS>(
    env: &Environment<SI, SO, SE, FS>,
    line: &str,
    opts: &NlOptions,
    state: &mut NlState,
) -> Result<(), Error>
where
    SI: StdioIn,
    SO: StdioOut,
    SE: StdioOut,
    FS: Filesystem,
{
    // Check for section delimiter
    if let Some(new_section) = check_section_delimiter(line, opts.delimiter) {
        state.section = new_section;
        state.adjacent_blanks = 0;
        if opts.restart {
            state.line_number = opts.start_num;
        }
        // Don't output section delimiter lines
        return Ok(());
    }

    // Track adjacent blank lines
    if line.is_empty() {
        state.adjacent_blanks += 1;
    } else {
        state.adjacent_blanks = 0;
    }

    // Get the numbering type for the current section
    let numbering_type = match state.section {
        Section::Header => &opts.header_type,
        Section::Body => &opts.body_type,
        Section::Footer => &opts.footer_type,
    };

    // Build output
    let output = if should_number(line, numbering_type, state.adjacent_blanks, opts.blank_join) {
        let num_str = format_line_number(state.line_number, opts.width, opts.format);
        state.line_number += opts.increment;
        // Reset blank counter after numbering
        if line.is_empty() {
            state.adjacent_blanks = 0;
        }
        format!("{}{}{}", num_str, opts.separator, line)
    } else {
        // Output width spaces instead of number
        format!(
            "{:width$}{}{}",
            "",
            opts.separator,
            line,
            width = opts.width
        )
    };

    Ok(env.stdout.write_line(&output)?)
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
        let env = make_test_env_with_stdin(vec!["nl"], "");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("", env.stdout.into_string());
    }

    #[test]
    fn single_line() {
        let env = make_test_env_with_stdin(vec!["nl"], "hello");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert_eq!("     1\thello\n", stdout);
    }

    #[test]
    fn multiple_lines() {
        let env = make_test_env_with_stdin(vec!["nl"], "one\ntwo\nthree");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert_eq!("     1\tone\n     2\ttwo\n     3\tthree\n", stdout);
    }

    #[test]
    fn default_skips_blank_lines() {
        let env = make_test_env_with_stdin(vec!["nl"], "one\n\nthree");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        // Default is -b t (non-empty lines only)
        assert_eq!("     1\tone\n      \t\n     2\tthree\n", stdout);
    }

    // ========================================================================
    // -b flag tests (body numbering type)
    // ========================================================================

    #[test]
    fn body_type_all() {
        let env = make_test_env_with_stdin(vec!["nl", "-ba"], "one\n\nthree");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert_eq!("     1\tone\n     2\t\n     3\tthree\n", stdout);
    }

    #[test]
    fn body_type_nonempty() {
        let env = make_test_env_with_stdin(vec!["nl", "-bt"], "one\n\nthree");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert_eq!("     1\tone\n      \t\n     2\tthree\n", stdout);
    }

    #[test]
    fn body_type_none() {
        let env = make_test_env_with_stdin(vec!["nl", "-bn"], "one\ntwo\nthree");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert_eq!("      \tone\n      \ttwo\n      \tthree\n", stdout);
    }

    #[test]
    fn body_type_regex() {
        let env = make_test_env_with_stdin(vec!["nl", "-bp^foo"], "foo bar\nbaz\nfoobar");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        // Only lines starting with "foo" are numbered
        assert_eq!("     1\tfoo bar\n      \tbaz\n     2\tfoobar\n", stdout);
    }

    // ========================================================================
    // -n flag tests (number format)
    // ========================================================================

    #[test]
    fn format_rn() {
        let env = make_test_env_with_stdin(vec!["nl", "-nrn"], "line");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert_eq!("     1\tline\n", stdout);
    }

    #[test]
    fn format_ln() {
        let env = make_test_env_with_stdin(vec!["nl", "-nln"], "line");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert_eq!("1     \tline\n", stdout);
    }

    #[test]
    fn format_rz() {
        let env = make_test_env_with_stdin(vec!["nl", "-nrz"], "line");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert_eq!("000001\tline\n", stdout);
    }

    // ========================================================================
    // -w flag tests (width)
    // ========================================================================

    #[test]
    fn width_10() {
        let env = make_test_env_with_stdin(vec!["nl", "-w10"], "line");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert_eq!("         1\tline\n", stdout);
    }

    #[test]
    fn width_2() {
        let env = make_test_env_with_stdin(vec!["nl", "-w2"], "line");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert_eq!(" 1\tline\n", stdout);
    }

    #[test]
    fn width_zero_invalid() {
        let env = make_test_env_with_stdin(vec!["nl", "-w0"], "line");
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("width"));
    }

    // ========================================================================
    // -s flag tests (separator)
    // ========================================================================

    #[test]
    fn separator_custom() {
        let env = make_test_env_with_stdin(vec!["nl", "-s->"], "line");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert_eq!("     1->line\n", stdout);
    }

    #[test]
    fn separator_space() {
        let env = make_test_env_with_stdin(vec!["nl", "-s "], "line");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert_eq!("     1 line\n", stdout);
    }

    // ========================================================================
    // -v flag tests (start number)
    // ========================================================================

    #[test]
    fn start_number_10() {
        let env = make_test_env_with_stdin(vec!["nl", "-v10"], "one\ntwo");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert_eq!("    10\tone\n    11\ttwo\n", stdout);
    }

    #[test]
    fn start_number_negative() {
        let env = make_test_env_with_stdin(vec!["nl", "-v-5"], "one\ntwo\nthree");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert_eq!("    -5\tone\n    -4\ttwo\n    -3\tthree\n", stdout);
    }

    // ========================================================================
    // -i flag tests (increment)
    // ========================================================================

    #[test]
    fn increment_2() {
        let env = make_test_env_with_stdin(vec!["nl", "-i2"], "one\ntwo\nthree");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert_eq!("     1\tone\n     3\ttwo\n     5\tthree\n", stdout);
    }

    #[test]
    fn increment_negative() {
        let env = make_test_env_with_stdin(vec!["nl", "-v10", "-i-1"], "one\ntwo\nthree");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert_eq!("    10\tone\n     9\ttwo\n     8\tthree\n", stdout);
    }

    // ========================================================================
    // -l flag tests (blank join)
    // ========================================================================

    #[test]
    fn blank_join_2() {
        let env = make_test_env_with_stdin(vec!["nl", "-ba", "-l2"], "one\n\n\nfour");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        // Only the second blank line gets numbered
        assert_eq!("     1\tone\n      \t\n     2\t\n     3\tfour\n", stdout);
    }

    // ========================================================================
    // Logical page section tests
    // ========================================================================

    #[test]
    fn header_section() {
        let env = make_test_env_with_stdin(
            vec!["nl", "-ha"],
            "\\:\\:\\:\nheader1\nheader2\n\\:\\:\nbody1",
        );
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        // Header lines numbered with -ha, then body resets
        assert!(stdout.contains("     1\theader1"));
        assert!(stdout.contains("     2\theader2"));
        assert!(stdout.contains("     1\tbody1"));
    }

    #[test]
    fn footer_section() {
        let env = make_test_env_with_stdin(vec!["nl", "-fa"], "body1\n\\:\nfooter1\nfooter2");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        // Footer lines numbered with -fa
        assert!(stdout.contains("     1\tbody1"));
        assert!(stdout.contains("     1\tfooter1"));
        assert!(stdout.contains("     2\tfooter2"));
    }

    #[test]
    fn no_restart_with_p() {
        let env = make_test_env_with_stdin(vec!["nl", "-p"], "body1\nbody2\n\\:\\:\nbody3\nbody4");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        // With -p, line numbers continue across section changes
        assert!(stdout.contains("     1\tbody1"));
        assert!(stdout.contains("     2\tbody2"));
        assert!(stdout.contains("     3\tbody3"));
        assert!(stdout.contains("     4\tbody4"));
    }

    // ========================================================================
    // -d flag tests (delimiter)
    // ========================================================================

    #[test]
    fn custom_delimiter() {
        let env = make_test_env_with_stdin(vec!["nl", "-d##"], "body1\n####\nbody2");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        // ## is body delimiter, so body2 restarts at 1
        assert!(stdout.contains("     1\tbody1"));
        assert!(stdout.contains("     1\tbody2"));
    }

    // ========================================================================
    // File tests
    // ========================================================================

    #[test]
    fn file_not_found() {
        let env = make_test_env_with_stdin(vec!["nl", "nonexistent.txt"], "");
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("nonexistent.txt"));
    }

    #[test]
    fn single_file() {
        let env = make_test_env_with_stdin(vec!["nl", "file.txt"], "");
        env.fs.add_file("file.txt", "line1\nline2\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert_eq!("     1\tline1\n     2\tline2\n", stdout);
    }

    #[test]
    fn multiple_files() {
        let env = make_test_env_with_stdin(vec!["nl", "a.txt", "b.txt"], "");
        env.fs.add_file("a.txt", "aaa\n");
        env.fs.add_file("b.txt", "bbb\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        // Line numbers continue across files
        assert_eq!("     1\taaa\n     2\tbbb\n", stdout);
    }

    #[test]
    fn stdin_dash() {
        let env = make_test_env_with_stdin(vec!["nl", "-"], "from stdin");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert_eq!("     1\tfrom stdin\n", stdout);
    }

    // ========================================================================
    // Error handling tests
    // ========================================================================

    #[test]
    fn invalid_format() {
        let env = make_test_env_with_stdin(vec!["nl", "-nxx"], "line");
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("illegal format"));
    }

    #[test]
    fn invalid_body_type() {
        let env = make_test_env_with_stdin(vec!["nl", "-bx"], "line");
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("illegal"));
        assert!(stderr.contains("body"));
    }

    #[test]
    fn invalid_regex() {
        let env = make_test_env_with_stdin(vec!["nl", "-bp["], "line");
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("body"));
        assert!(stderr.contains("expr"));
    }

    #[test]
    fn invalid_increment() {
        let env = make_test_env_with_stdin(vec!["nl", "-iabc"], "line");
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("invalid incr"));
    }

    // ========================================================================
    // Combined options tests
    // ========================================================================

    #[test]
    fn combined_rz_w4_v2_i2() {
        let env =
            make_test_env_with_stdin(vec!["nl", "-nrz", "-w4", "-v2", "-i2", "-s->"], "a\nb\nc");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert_eq!("0002->a\n0004->b\n0006->c\n", stdout);
    }

    // ========================================================================
    // format_line_number unit tests
    // ========================================================================

    #[test]
    fn format_line_number_rn() {
        assert_eq!("     1", format_line_number(1, 6, NumberFormat::Rn));
        assert_eq!("   123", format_line_number(123, 6, NumberFormat::Rn));
        assert_eq!("    -5", format_line_number(-5, 6, NumberFormat::Rn));
    }

    #[test]
    fn format_line_number_ln() {
        assert_eq!("1     ", format_line_number(1, 6, NumberFormat::Ln));
        assert_eq!("123   ", format_line_number(123, 6, NumberFormat::Ln));
        assert_eq!("-5    ", format_line_number(-5, 6, NumberFormat::Ln));
    }

    #[test]
    fn format_line_number_rz() {
        assert_eq!("000001", format_line_number(1, 6, NumberFormat::Rz));
        assert_eq!("000123", format_line_number(123, 6, NumberFormat::Rz));
        // Note: negative numbers with leading zeros look odd but match behavior
        assert_eq!("0000-5", format_line_number(-5, 6, NumberFormat::Rz));
    }

    // ========================================================================
    // check_section_delimiter unit tests
    // ========================================================================

    #[test]
    fn check_delimiter_header() {
        assert_eq!(
            Some(Section::Header),
            check_section_delimiter("\\:\\:\\:", ('\\', ':'))
        );
    }

    #[test]
    fn check_delimiter_body() {
        assert_eq!(
            Some(Section::Body),
            check_section_delimiter("\\:\\:", ('\\', ':'))
        );
    }

    #[test]
    fn check_delimiter_footer() {
        assert_eq!(
            Some(Section::Footer),
            check_section_delimiter("\\:", ('\\', ':'))
        );
    }

    #[test]
    fn check_delimiter_none() {
        assert_eq!(None, check_section_delimiter("hello", ('\\', ':')));
        assert_eq!(None, check_section_delimiter("\\", ('\\', ':')));
        assert_eq!(None, check_section_delimiter(":", ('\\', ':')));
    }

    #[test]
    fn check_delimiter_custom() {
        assert_eq!(
            Some(Section::Body),
            check_section_delimiter("####", ('#', '#'))
        );
    }
}

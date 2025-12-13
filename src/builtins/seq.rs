//! The seq utility: print sequences of numbers.

use getopts::Options;

use crate::{Environment, Error, ExitCode, Filesystem, Stderr, Stdin, Stdout};

/// Options for the seq command.
#[derive(Clone, Debug)]
struct SeqOptions {
    /// The format string for printing numbers.
    format: Option<String>,
    /// The separator between numbers (default is newline).
    separator: String,
    /// The terminator after the sequence (default is none).
    terminator: Option<String>,
    /// Whether to equalize widths with zero padding.
    equal_width: bool,
}

impl Default for SeqOptions {
    fn default() -> Self {
        Self {
            format: None,
            separator: "\n".to_string(),
            terminator: None,
            equal_width: false,
        }
    }
}

/// Process C-style escape sequences in a string.
fn unescape(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();

    while let Some(c) = chars.next() {
        if c != '\\' {
            result.push(c);
            continue;
        }

        match chars.next() {
            Some('a') => result.push('\x07'),
            Some('b') => result.push('\x08'),
            Some('e') => result.push('\x1b'),
            Some('f') => result.push('\x0c'),
            Some('n') => result.push('\n'),
            Some('r') => result.push('\r'),
            Some('t') => result.push('\t'),
            Some('v') => result.push('\x0b'),
            Some('\\') => result.push('\\'),
            Some('\'') => result.push('\''),
            Some('"') => result.push('"'),
            Some(c @ '0'..='7') => {
                let mut val = c.to_digit(8).unwrap();
                for _ in 0..2 {
                    if let Some(&next) = chars.peek() {
                        if let Some(d) = next.to_digit(8) {
                            val = val * 8 + d;
                            chars.next();
                        } else {
                            break;
                        }
                    }
                }
                result.push(char::from_u32(val).unwrap_or('\0'));
            }
            Some('x') => {
                let mut val = 0u32;
                let mut count = 0;
                while count < 2 {
                    if let Some(&next) = chars.peek() {
                        if let Some(d) = next.to_digit(16) {
                            val = val * 16 + d;
                            chars.next();
                            count += 1;
                        } else {
                            break;
                        }
                    } else {
                        break;
                    }
                }
                if count > 0 {
                    result.push(char::from_u32(val).unwrap_or('\0'));
                }
            }
            Some(c) => {
                result.push('\\');
                result.push(c);
            }
            None => result.push('\\'),
        }
    }

    result
}

/// Validate a printf-style format string for floating point output.
/// Returns true if the format string is valid (has exactly one floating point conversion).
fn valid_format(fmt: &str) -> bool {
    let mut conversions = 0u32;
    let mut chars = fmt.chars().peekable();

    while let Some(c) = chars.next() {
        if c != '%' {
            continue;
        }

        match chars.peek() {
            Some('%') => {
                chars.next();
                continue;
            }
            None => return false,
            _ => {}
        }

        // Skip flags: # 0 - + ' space
        while let Some(&c) = chars.peek() {
            if "#0- +'".contains(c) {
                chars.next();
            } else {
                break;
            }
        }

        // Skip field width
        while let Some(&c) = chars.peek() {
            if c.is_ascii_digit() {
                chars.next();
            } else {
                break;
            }
        }

        // Skip precision
        if chars.peek() == Some(&'.') {
            chars.next();
            while let Some(&c) = chars.peek() {
                if c.is_ascii_digit() {
                    chars.next();
                } else {
                    break;
                }
            }
        }

        // Check conversion specifier
        match chars.next() {
            Some('A' | 'a' | 'E' | 'e' | 'F' | 'f' | 'G' | 'g') => {
                conversions += 1;
            }
            _ => return false,
        }
    }

    conversions == 1
}

/// Parse a numeric string into f64.
fn parse_number(s: &str) -> Result<f64, String> {
    match s.parse::<f64>() {
        Ok(val) => {
            if val.is_nan() || val.is_infinite() {
                Err(format!("invalid floating point argument: {}", s))
            } else {
                Ok(if val == -0.0 { 0.0 } else { val })
            }
        }
        Err(_) => Err(format!("invalid floating point argument: {}", s)),
    }
}

/// Count decimal places in a number string.
fn decimal_places(s: &str) -> usize {
    if let Some(dot_pos) = s.find('.') {
        let after_dot = &s[dot_pos + 1..];
        after_dot.chars().take_while(|c| c.is_ascii_digit()).count()
    } else {
        0
    }
}

/// Format a number using the given format string.
fn format_number(fmt: &str, val: f64) -> String {
    let mut result = String::new();
    let mut chars = fmt.chars().peekable();

    while let Some(c) = chars.next() {
        if c != '%' {
            result.push(c);
            continue;
        }

        if chars.peek() == Some(&'%') {
            result.push('%');
            chars.next();
            continue;
        }

        // Parse the format specifier
        let mut spec = String::from("%");

        // Flags
        while let Some(&c) = chars.peek() {
            if "#0- +'".contains(c) {
                spec.push(c);
                chars.next();
            } else {
                break;
            }
        }

        // Width
        while let Some(&c) = chars.peek() {
            if c.is_ascii_digit() {
                spec.push(c);
                chars.next();
            } else {
                break;
            }
        }

        // Precision
        if chars.peek() == Some(&'.') {
            spec.push('.');
            chars.next();
            while let Some(&c) = chars.peek() {
                if c.is_ascii_digit() {
                    spec.push(c);
                    chars.next();
                } else {
                    break;
                }
            }
        }

        // Conversion specifier
        if let Some(conv) = chars.next() {
            spec.push(conv);
            let formatted = match conv {
                'e' | 'E' => format!("{:e}", val)
                    .replace('e', &conv.to_string())
                    .to_string(),
                'f' | 'F' => {
                    // Parse precision from spec
                    let precision = if let Some(dot_pos) = spec.find('.') {
                        spec[dot_pos + 1..spec.len() - 1]
                            .parse::<usize>()
                            .unwrap_or(6)
                    } else {
                        6
                    };
                    // Parse width from spec
                    let (width, zero_pad) = parse_width_and_pad(&spec);
                    let num_str = format!("{:.prec$}", val, prec = precision);
                    if let Some(w) = width {
                        if zero_pad {
                            format!("{:0>width$}", num_str, width = w)
                        } else {
                            format!("{:>width$}", num_str, width = w)
                        }
                    } else {
                        num_str
                    }
                }
                'g' | 'G' => {
                    // Parse precision from spec
                    let precision = if let Some(dot_pos) = spec.find('.') {
                        spec[dot_pos + 1..spec.len() - 1]
                            .parse::<usize>()
                            .unwrap_or(6)
                    } else {
                        6
                    };
                    let (width, zero_pad) = parse_width_and_pad(&spec);
                    let num_str = format_g(val, precision, conv == 'G');
                    if let Some(w) = width {
                        if zero_pad {
                            format!("{:0>width$}", num_str, width = w)
                        } else {
                            format!("{:>width$}", num_str, width = w)
                        }
                    } else {
                        num_str
                    }
                }
                'a' | 'A' => format!("{:e}", val), // Rust doesn't have %a, use %e
                _ => spec,
            };
            result.push_str(&formatted);
        }
    }

    result
}

/// Parse width and zero-pad flag from format spec like "%05.2f".
fn parse_width_and_pad(spec: &str) -> (Option<usize>, bool) {
    let s = &spec[1..]; // skip '%'
    let mut chars = s.chars().peekable();
    let mut zero_pad = false;

    // Skip flags, noting if '0' is present
    while let Some(&c) = chars.peek() {
        if c == '0' {
            zero_pad = true;
            chars.next();
        } else if "#- +'".contains(c) {
            chars.next();
        } else {
            break;
        }
    }

    // Parse width
    let mut width_str = String::new();
    while let Some(&c) = chars.peek() {
        if c.is_ascii_digit() {
            width_str.push(c);
            chars.next();
        } else {
            break;
        }
    }

    let width = if width_str.is_empty() {
        None
    } else {
        width_str.parse().ok()
    };

    (width, zero_pad)
}

/// Format a number in %g style (compact representation).
fn format_g(val: f64, precision: usize, uppercase: bool) -> String {
    if val == 0.0 {
        return "0".to_string();
    }

    let exp = val.abs().log10().floor() as i32;

    if exp < -4 || exp >= precision as i32 {
        // Use exponential notation
        let e_char = if uppercase { 'E' } else { 'e' };
        let mantissa = val / 10_f64.powi(exp);
        let formatted = format!("{:.prec$}", mantissa, prec = precision.saturating_sub(1));
        let trimmed = trim_trailing_zeros(&formatted);
        format!("{}{}{:+03}", trimmed, e_char, exp)
    } else {
        // Use fixed notation
        let decimal_digits = if exp >= 0 {
            precision.saturating_sub(exp as usize + 1)
        } else {
            precision + (-exp - 1) as usize
        };
        let formatted = format!("{:.prec$}", val, prec = decimal_digits);
        trim_trailing_zeros(&formatted)
    }
}

/// Trim trailing zeros after decimal point, and the decimal point if no decimals remain.
fn trim_trailing_zeros(s: &str) -> String {
    if !s.contains('.') {
        return s.to_string();
    }
    let trimmed = s.trim_end_matches('0');
    if let Some(stripped) = trimmed.strip_suffix('.') {
        stripped.to_string()
    } else {
        trimmed.to_string()
    }
}

/// Generate a format string based on the input numbers and options.
fn generate_format(first: f64, incr: f64, last: f64, equal_width: bool) -> String {
    if !equal_width {
        return "%g".to_string();
    }

    // Figure out the actual last value that will be printed
    let actual_last = if first > last {
        first - incr.abs() * ((first - last) / incr.abs()).floor()
    } else {
        first + incr.abs() * ((last - first) / incr.abs()).floor()
    };

    // Check if any values would print in exponential notation
    let first_str = format_g(first, 6, false);
    let incr_str = format_g(incr, 6, false);
    let last_str = format_g(actual_last, 6, false);

    let uses_exp = first_str.contains('e')
        || first_str.contains('E')
        || incr_str.contains('e')
        || incr_str.contains('E')
        || last_str.contains('e')
        || last_str.contains('E');

    // Calculate required precision
    let precision = decimal_places(&first_str)
        .max(decimal_places(&incr_str))
        .max(decimal_places(&last_str));

    // Calculate required width
    let first_width = integer_width(&first_str);
    let last_width = integer_width(&last_str);
    let max_width = first_width.max(last_width);

    if precision > 0 {
        let total_width = max_width + 1 + precision; // integer + '.' + decimals
        if uses_exp {
            format!("%0{}.{}e", total_width, precision)
        } else {
            format!("%0{}.{}f", total_width, precision)
        }
    } else if uses_exp {
        format!("%0{}e", max_width)
    } else {
        format!("%0{}g", max_width)
    }
}

/// Get the width of the integer part of a number string.
fn integer_width(s: &str) -> usize {
    if let Some(dot_pos) = s.find('.') {
        dot_pos
    } else if let Some(e_pos) = s.find(['e', 'E']) {
        e_pos
    } else {
        s.len()
    }
}

fn build_options() -> Options {
    let mut opts = Options::new();
    opts.optopt(
        "f",
        "format",
        "Use printf style floating-point format",
        "FORMAT",
    );
    opts.optopt("s", "separator", "String to separate numbers", "STRING");
    opts.optopt("t", "terminator", "String to terminate sequence", "STRING");
    opts.optflag(
        "w",
        "equal-width",
        "Equalize width by padding with leading zeroes",
    );
    opts
}

/// Check if a string looks like a number (including negative numbers).
fn is_numeric(s: &str) -> bool {
    s.parse::<f64>().is_ok()
}

/// The seq builtin: print sequences of numbers.
pub fn bin<SI, SO, SE, FS>(env: &Environment<SI, SO, SE, FS>) -> Result<ExitCode, Error>
where
    SI: Stdin,
    SO: Stdout,
    SE: Stderr,
    FS: Filesystem,
{
    // Find the first numeric argument - everything before it goes to getopts,
    // everything from it onwards is positional arguments.
    let args = &env.args[1..];
    let first_numeric = args
        .iter()
        .position(|a| is_numeric(a))
        .unwrap_or(args.len());

    let opts_def = build_options();

    let matches = match opts_def.parse(&args[..first_numeric]) {
        Ok(m) => m,
        Err(e) => {
            env.stderr.write_line(&format!("seq: {}", e))?;
            return Ok(ExitCode::from(1));
        }
    };

    let mut seq_opts = SeqOptions::default();

    if let Some(fmt) = matches.opt_str("f") {
        let unescaped = unescape(&fmt);
        if !valid_format(&unescaped) {
            env.stderr
                .write_line(&format!("seq: invalid format string: `{}'", fmt))?;
            return Ok(ExitCode::from(1));
        }
        seq_opts.format = Some(unescaped);
    }

    if let Some(sep) = matches.opt_str("s") {
        seq_opts.separator = unescape(&sep);
    }

    if let Some(term) = matches.opt_str("t") {
        seq_opts.terminator = Some(unescape(&term));
    }

    if matches.opt_present("w") && seq_opts.format.is_none() {
        seq_opts.equal_width = true;
    }

    // Positional arguments: any free args from getopts plus everything after first numeric
    let mut positional: Vec<&str> = matches.free.iter().map(|s| s.as_str()).collect();
    positional.extend(args[first_numeric..].iter().map(|s| s.as_str()));
    let args = positional;
    if args.is_empty() || args.len() > 3 {
        env.stderr.write_line(
            "usage: seq [-w] [-f format] [-s string] [-t string] [first [incr]] last",
        )?;
        return Ok(ExitCode::from(1));
    }

    let (first, incr, last) = match args.len() {
        1 => {
            let last = match parse_number(args[0]) {
                Ok(v) => v,
                Err(e) => {
                    env.stderr.write_line(&format!("seq: {}", e))?;
                    return Ok(ExitCode::from(1));
                }
            };
            (1.0, if 1.0 <= last { 1.0 } else { -1.0 }, last)
        }
        2 => {
            let first = match parse_number(args[0]) {
                Ok(v) => v,
                Err(e) => {
                    env.stderr.write_line(&format!("seq: {}", e))?;
                    return Ok(ExitCode::from(1));
                }
            };
            let last = match parse_number(args[1]) {
                Ok(v) => v,
                Err(e) => {
                    env.stderr.write_line(&format!("seq: {}", e))?;
                    return Ok(ExitCode::from(1));
                }
            };
            (first, if first <= last { 1.0 } else { -1.0 }, last)
        }
        3 => {
            let first = match parse_number(args[0]) {
                Ok(v) => v,
                Err(e) => {
                    env.stderr.write_line(&format!("seq: {}", e))?;
                    return Ok(ExitCode::from(1));
                }
            };
            let incr = match parse_number(args[1]) {
                Ok(v) => v,
                Err(e) => {
                    env.stderr.write_line(&format!("seq: {}", e))?;
                    return Ok(ExitCode::from(1));
                }
            };
            let last = match parse_number(args[2]) {
                Ok(v) => v,
                Err(e) => {
                    env.stderr.write_line(&format!("seq: {}", e))?;
                    return Ok(ExitCode::from(1));
                }
            };
            (first, incr, last)
        }
        _ => unreachable!(),
    };

    // Validate increment
    if incr == 0.0 {
        let direction = if first < last { "in" } else { "de" };
        env.stderr
            .write_line(&format!("seq: zero {}crement", direction))?;
        return Ok(ExitCode::from(1));
    }

    if incr < 0.0 && first < last {
        env.stderr.write_line("seq: needs positive increment")?;
        return Ok(ExitCode::from(1));
    }

    if incr > 0.0 && first > last {
        env.stderr.write_line("seq: needs negative decrement")?;
        return Ok(ExitCode::from(1));
    }

    // Generate or use format
    let fmt = seq_opts
        .format
        .clone()
        .unwrap_or_else(|| generate_format(first, incr, last, seq_opts.equal_width));

    // Generate the sequence
    let mut step = 0u64;
    let mut prev = first;
    let mut printed_first = false;

    loop {
        let cur = first + incr * step as f64;

        let should_print = if incr > 0.0 { cur <= last } else { cur >= last };

        if !should_print {
            break;
        }

        if printed_first {
            env.stdout.write_str(&seq_opts.separator)?;
        }

        env.stdout.write_str(&format_number(&fmt, cur))?;
        printed_first = true;
        prev = cur;
        step += 1;
    }

    // Check if we missed the last value due to floating point rounding
    let final_cur = first + incr * step as f64;
    let cur_str = format_number(&fmt, final_cur);
    let last_str = format_number(&fmt, last);
    let prev_str = format_number(&fmt, prev);

    if cur_str == last_str && cur_str != prev_str {
        env.stdout.write_str(&seq_opts.separator)?;
        env.stdout.write_str(&last_str)?;
    }

    // Print terminator if specified
    if let Some(ref term) = seq_opts.terminator {
        if printed_first {
            env.stdout.write_str(&seq_opts.separator)?;
        }
        env.stdout.write_str(term)?;
    }

    env.stdout.write_str("\n")?;

    Ok(ExitCode::from(0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MockFilesystem, StringStderr, StringStdin, StringStdout};

    fn make_env(
        args: Vec<&str>,
    ) -> Environment<StringStdin, StringStdout, StringStderr, MockFilesystem> {
        Environment {
            stdin: StringStdin::new(""),
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
    // Basic sequence tests
    // ========================================================================

    #[test]
    fn seq_1_to_3() {
        let env = make_env(vec!["seq", "3"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("1\n2\n3\n", env.stdout.into_string());
    }

    #[test]
    fn seq_1_to_5() {
        let env = make_env(vec!["seq", "5"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("1\n2\n3\n4\n5\n", env.stdout.into_string());
    }

    #[test]
    fn seq_2_to_5() {
        let env = make_env(vec!["seq", "2", "5"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("2\n3\n4\n5\n", env.stdout.into_string());
    }

    #[test]
    fn seq_1_2_5() {
        let env = make_env(vec!["seq", "1", "2", "5"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("1\n3\n5\n", env.stdout.into_string());
    }

    #[test]
    fn seq_descending() {
        let env = make_env(vec!["seq", "5", "1"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("5\n4\n3\n2\n1\n", env.stdout.into_string());
    }

    #[test]
    fn seq_descending_explicit_decrement() {
        let env = make_env(vec!["seq", "5", "-1", "1"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("5\n4\n3\n2\n1\n", env.stdout.into_string());
    }

    #[test]
    fn seq_descending_step_2() {
        let env = make_env(vec!["seq", "10", "-2", "1"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("10\n8\n6\n4\n2\n", env.stdout.into_string());
    }

    // ========================================================================
    // Floating point tests
    // ========================================================================

    #[test]
    fn seq_float_increment() {
        let env = make_env(vec!["seq", "1", "0.5", "3"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("1\n1.5\n2\n2.5\n3\n", env.stdout.into_string());
    }

    #[test]
    fn seq_float_start() {
        let env = make_env(vec!["seq", "0.5", "3"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("0.5\n1.5\n2.5\n", env.stdout.into_string());
    }

    #[test]
    fn seq_negative_start() {
        let env = make_env(vec!["seq", "-3", "0"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("-3\n-2\n-1\n0\n", env.stdout.into_string());
    }

    #[test]
    fn seq_negative_to_positive() {
        let env = make_env(vec!["seq", "-2", "2"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("-2\n-1\n0\n1\n2\n", env.stdout.into_string());
    }

    // ========================================================================
    // Format string tests (-f)
    // ========================================================================

    #[test]
    fn seq_format_f() {
        let env = make_env(vec!["seq", "-f", "%.2f", "1", "0.5", "2"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("1.00\n1.50\n2.00\n", env.stdout.into_string());
    }

    #[test]
    fn seq_format_with_text() {
        let env = make_env(vec!["seq", "-f", "Item %g", "1", "3"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("Item 1\nItem 2\nItem 3\n", env.stdout.into_string());
    }

    #[test]
    fn seq_format_percent_percent() {
        let env = make_env(vec!["seq", "-f", "%g%%", "1", "3"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("1%\n2%\n3%\n", env.stdout.into_string());
    }

    #[test]
    fn seq_invalid_format_no_conversion() {
        let env = make_env(vec!["seq", "-f", "no conversion", "1", "3"]);
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("invalid format string"));
    }

    #[test]
    fn seq_invalid_format_integer_conversion() {
        let env = make_env(vec!["seq", "-f", "%d", "1", "3"]);
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("invalid format string"));
    }

    // ========================================================================
    // Separator tests (-s)
    // ========================================================================

    #[test]
    fn seq_separator_comma() {
        let env = make_env(vec!["seq", "-s", ",", "1", "5"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("1,2,3,4,5\n", env.stdout.into_string());
    }

    #[test]
    fn seq_separator_arrow() {
        let env = make_env(vec!["seq", "-s", "-->", "1", "3"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("1-->2-->3\n", env.stdout.into_string());
    }

    #[test]
    fn seq_separator_tab_escape() {
        let env = make_env(vec!["seq", "-s", "\\t", "1", "3"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("1\t2\t3\n", env.stdout.into_string());
    }

    #[test]
    fn seq_separator_empty() {
        let env = make_env(vec!["seq", "-s", "", "1", "5"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("12345\n", env.stdout.into_string());
    }

    // ========================================================================
    // Terminator tests (-t)
    // ========================================================================

    #[test]
    fn seq_terminator() {
        let env = make_env(vec!["seq", "-t", "[end]", "1", "3"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("1\n2\n3\n[end]\n", env.stdout.into_string());
    }

    #[test]
    fn seq_terminator_with_separator() {
        let env = make_env(vec!["seq", "-s", "-->", "-t", "[end]", "1", "3"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("1-->2-->3-->[end]\n", env.stdout.into_string());
    }

    // ========================================================================
    // Equal width tests (-w)
    // ========================================================================

    #[test]
    fn seq_equal_width() {
        let env = make_env(vec!["seq", "-w", "8", "10"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("08\n09\n10\n", env.stdout.into_string());
    }

    #[test]
    fn seq_equal_width_three_digits() {
        let env = make_env(vec!["seq", "-w", "98", "101"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("098\n099\n100\n101\n", env.stdout.into_string());
    }

    #[test]
    fn seq_equal_width_decimals() {
        let env = make_env(vec!["seq", "-w", "0", ".05", ".1"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let output = env.stdout.into_string();
        println!("output: {:?}", output);
        assert!(output.contains("0.00"));
        assert!(output.contains("0.05"));
        assert!(output.contains("0.10"));
    }

    #[test]
    fn seq_w_ignored_with_f() {
        let env = make_env(vec!["seq", "-w", "-f", "%g", "8", "10"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        // -w is ignored when -f is specified
        assert_eq!("8\n9\n10\n", env.stdout.into_string());
    }

    // ========================================================================
    // Error cases
    // ========================================================================

    #[test]
    fn seq_no_arguments() {
        let env = make_env(vec!["seq"]);
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("usage:"));
    }

    #[test]
    fn seq_too_many_arguments() {
        let env = make_env(vec!["seq", "1", "2", "3", "4"]);
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("usage:"));
    }

    #[test]
    fn seq_zero_increment() {
        let env = make_env(vec!["seq", "1", "0", "5"]);
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("zero increment"));
    }

    #[test]
    fn seq_zero_decrement() {
        let env = make_env(vec!["seq", "5", "0", "1"]);
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("zero decrement"));
    }

    #[test]
    fn seq_needs_positive_increment() {
        let env = make_env(vec!["seq", "1", "-1", "5"]);
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("needs positive increment"));
    }

    #[test]
    fn seq_needs_negative_decrement() {
        let env = make_env(vec!["seq", "5", "1", "1"]);
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("needs negative decrement"));
    }

    #[test]
    fn seq_invalid_number() {
        let env = make_env(vec!["seq", "abc"]);
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("invalid floating point argument"));
    }

    // ========================================================================
    // Edge cases
    // ========================================================================

    #[test]
    fn seq_single_value() {
        let env = make_env(vec!["seq", "1", "1"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("1\n", env.stdout.into_string());
    }

    #[test]
    fn seq_same_start_end() {
        let env = make_env(vec!["seq", "5", "5"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("5\n", env.stdout.into_string());
    }

    #[test]
    fn seq_large_step_past_end() {
        let env = make_env(vec!["seq", "1", "10", "5"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("1\n", env.stdout.into_string());
    }

    #[test]
    fn seq_zero() {
        let env = make_env(vec!["seq", "0"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        // seq 0 starts at 1 (default) and goes to 0 with increment -1
        assert_eq!("1\n0\n", env.stdout.into_string());
    }

    #[test]
    fn seq_negative_only() {
        let env = make_env(vec!["seq", "-5", "-3"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("-5\n-4\n-3\n", env.stdout.into_string());
    }

    // ========================================================================
    // Escape sequence tests
    // ========================================================================

    #[test]
    fn unescape_newline() {
        assert_eq!("\n", unescape("\\n"));
    }

    #[test]
    fn unescape_tab() {
        assert_eq!("\t", unescape("\\t"));
    }

    #[test]
    fn unescape_backslash() {
        assert_eq!("\\", unescape("\\\\"));
    }

    #[test]
    fn unescape_octal() {
        assert_eq!("A", unescape("\\101"));
    }

    #[test]
    fn unescape_hex() {
        assert_eq!("A", unescape("\\x41"));
    }

    #[test]
    fn unescape_mixed() {
        assert_eq!("a\tb\nc", unescape("a\\tb\\nc"));
    }

    // ========================================================================
    // Format validation tests
    // ========================================================================

    #[test]
    fn valid_format_g() {
        assert!(valid_format("%g"));
    }

    #[test]
    fn valid_format_f() {
        assert!(valid_format("%f"));
    }

    #[test]
    fn valid_format_e() {
        assert!(valid_format("%e"));
    }

    #[test]
    fn valid_format_with_width() {
        assert!(valid_format("%10.2f"));
    }

    #[test]
    fn valid_format_with_flags() {
        assert!(valid_format("%+010.2f"));
    }

    #[test]
    fn valid_format_with_text() {
        assert!(valid_format("value: %g units"));
    }

    #[test]
    fn valid_format_with_percent_percent() {
        assert!(valid_format("%% %g %%"));
    }

    #[test]
    fn invalid_format_integer() {
        assert!(!valid_format("%d"));
    }

    #[test]
    fn invalid_format_string() {
        assert!(!valid_format("%s"));
    }

    #[test]
    fn invalid_format_no_conversion() {
        assert!(!valid_format("no conversion"));
    }

    #[test]
    fn invalid_format_two_conversions() {
        assert!(!valid_format("%g %g"));
    }
}

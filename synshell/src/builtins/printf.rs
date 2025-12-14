use crate::{Environment, Error, ExitCode, Filesystem, Stderr, Stdin, Stdout};

/// The printf builtin: format and print data.
///
/// SYNOPSIS
///     printf format [arguments ...]
///
/// The printf utility formats and prints its arguments under control of the format string.
pub fn bin<SI, SO, SE, FS>(env: &Environment<SI, SO, SE, FS>) -> Result<ExitCode, Error>
where
    SI: Stdin,
    SO: Stdout,
    SE: Stderr,
    FS: Filesystem,
{
    // Skip argv[0] ("printf") and handle "--"
    let args: Vec<&str> = env.args.iter().skip(1).map(|s| s.as_str()).collect();

    // Handle "--" to end option processing
    let args = if args.first() == Some(&"--") {
        &args[1..]
    } else {
        &args[..]
    };

    if args.is_empty() {
        env.stderr
            .write_line("printf: usage: printf format [arguments ...]")?;
        return Ok(ExitCode::from(1));
    }

    let format = args[0];
    let arguments: Vec<&str> = args[1..].to_vec();

    let mut state = PrintfState::new(&arguments);
    let mut rval: i8 = 0;

    // Process format string, reusing it for all arguments
    loop {
        state.start_iteration();
        let result = process_format(env, format, &mut state)?;
        if result != 0 {
            rval = result;
        }

        // If we've consumed all arguments, we're done
        if !state.has_remaining_args() {
            break;
        }

        // Safety check: if we made no progress this iteration, break to avoid infinite loop
        if !state.made_progress() {
            break;
        }
    }

    Ok(ExitCode::from(rval))
}

/// State for tracking argument consumption during printf.
struct PrintfState<'a> {
    args: &'a [&'a str],
    index: usize,
    iteration_start_index: usize,
}

impl<'a> PrintfState<'a> {
    fn new(args: &'a [&'a str]) -> Self {
        Self {
            args,
            index: 0,
            iteration_start_index: 0,
        }
    }

    fn start_iteration(&mut self) {
        self.iteration_start_index = self.index;
    }

    fn made_progress(&self) -> bool {
        self.index > self.iteration_start_index
    }

    fn has_remaining_args(&self) -> bool {
        self.index < self.args.len()
    }

    fn next_arg(&mut self) -> Option<&'a str> {
        if self.index < self.args.len() {
            let arg = self.args[self.index];
            self.index += 1;
            Some(arg)
        } else {
            None
        }
    }

    fn get_string(&mut self) -> &'a str {
        self.next_arg().unwrap_or("")
    }

    fn get_char(&mut self) -> char {
        let s = self.get_string();
        s.chars().next().unwrap_or('\0')
    }

    fn get_int(&mut self) -> Result<i64, String> {
        let s = self.next_arg().unwrap_or("0");
        parse_int(s)
    }

    fn get_uint(&mut self) -> Result<u64, String> {
        let s = self.next_arg().unwrap_or("0");
        parse_uint(s)
    }

    fn get_float(&mut self) -> Result<f64, String> {
        let s = self.next_arg().unwrap_or("0");
        parse_float(s)
    }
}

/// Parse an integer argument, supporting:
/// - Decimal, octal (0), hex (0x)
/// - Leading + or -
/// - Character constants like 'A or "A
fn parse_int(s: &str) -> Result<i64, String> {
    if s.is_empty() {
        return Ok(0);
    }

    // Handle character constants
    if s.starts_with('\'') || s.starts_with('"') {
        let ch = s.chars().nth(1).unwrap_or('\0');
        return Ok(ch as i64);
    }

    // Handle sign
    let (negative, s) = if let Some(rest) = s.strip_prefix('-') {
        (true, rest)
    } else if let Some(rest) = s.strip_prefix('+') {
        (false, rest)
    } else {
        (false, s)
    };

    let value = if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        i64::from_str_radix(hex, 16).map_err(|e| format!("{}: {}", s, e))?
    } else if s.starts_with('0') && s.len() > 1 && s.chars().skip(1).all(|c| c.is_ascii_digit()) {
        i64::from_str_radix(s, 8).map_err(|e| format!("{}: {}", s, e))?
    } else {
        s.parse::<i64>().map_err(|e| format!("{}: {}", s, e))?
    };

    Ok(if negative { -value } else { value })
}

/// Parse an unsigned integer argument.
fn parse_uint(s: &str) -> Result<u64, String> {
    if s.is_empty() {
        return Ok(0);
    }

    // Handle character constants
    if s.starts_with('\'') || s.starts_with('"') {
        let ch = s.chars().nth(1).unwrap_or('\0');
        return Ok(ch as u64);
    }

    // Skip leading +
    let s = s.strip_prefix('+').unwrap_or(s);

    if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        u64::from_str_radix(hex, 16).map_err(|e| format!("{}: {}", s, e))
    } else if s.starts_with('0') && s.len() > 1 && s.chars().skip(1).all(|c| c.is_ascii_digit()) {
        u64::from_str_radix(s, 8).map_err(|e| format!("{}: {}", s, e))
    } else {
        s.parse::<u64>().map_err(|e| format!("{}: {}", s, e))
    }
}

/// Parse a floating point argument.
fn parse_float(s: &str) -> Result<f64, String> {
    if s.is_empty() {
        return Ok(0.0);
    }

    // Handle character constants
    if s.starts_with('\'') || s.starts_with('"') {
        let ch = s.chars().nth(1).unwrap_or('\0');
        return Ok(ch as u32 as f64);
    }

    s.parse::<f64>().map_err(|e| format!("{}: {}", s, e))
}

/// Process escape sequences in a string (for format string and %b).
/// Returns the processed string and whether \c was encountered.
fn process_escapes(s: &str, is_format: bool) -> (String, bool) {
    let mut result = String::new();
    let mut chars = s.chars().peekable();

    while let Some(c) = chars.next() {
        if c != '\\' {
            result.push(c);
            continue;
        }

        match chars.next() {
            None => {
                result.push('\\');
            }
            Some('\\') => result.push('\\'),
            Some('\'') => result.push('\''),
            Some('a') => result.push('\x07'), // bell
            Some('b') => result.push('\x08'), // backspace
            Some('c') if !is_format => return (result, true), // stop output
            Some('c') => result.push('c'),    // in format string, \c is literal
            Some('f') => result.push('\x0C'), // form feed
            Some('n') => result.push('\n'),
            Some('r') => result.push('\r'),
            Some('t') => result.push('\t'),
            Some('v') => result.push('\x0B'), // vertical tab
            Some(d) if d.is_ascii_digit() => {
                // Octal: in format string, \NNN; in %b, \0NNN
                let mut value: u32 = 0;
                let mut count = 0;
                let max_digits = if !is_format && d == '0' { 4 } else { 3 };

                // First digit
                if d != '0' || is_format {
                    value = d.to_digit(8).unwrap_or(0);
                    count = 1;
                }

                // Remaining digits
                while count < max_digits {
                    match chars.peek() {
                        Some(&c) if ('0'..='7').contains(&c) => {
                            value = value * 8 + c.to_digit(8).unwrap();
                            chars.next();
                            count += 1;
                        }
                        _ => break,
                    }
                }

                if value <= 0x7F {
                    result.push(value as u8 as char);
                } else {
                    result.push(char::from_u32(value).unwrap_or('\u{FFFD}'));
                }
            }
            Some(other) => {
                result.push(other);
            }
        }
    }

    (result, false)
}

/// Process the format string and output.
fn process_format<SI, SO, SE, FS>(
    env: &Environment<SI, SO, SE, FS>,
    format: &str,
    state: &mut PrintfState,
) -> Result<i8, Error>
where
    SI: Stdin,
    SO: Stdout,
    SE: Stderr,
    FS: Filesystem,
{
    let (format, _) = process_escapes(format, true);
    let mut chars = format.chars().peekable();
    let mut rval: i8 = 0;

    while let Some(c) = chars.next() {
        if c != '%' {
            env.stdout.write_str(&c.to_string())?;
            continue;
        }

        // Look at next character
        match chars.peek() {
            None => {
                // Trailing %, treat as literal
                env.stdout.write_str("%")?;
            }
            Some('%') => {
                // %%
                chars.next();
                env.stdout.write_str("%")?;
            }
            Some(_) => {
                // Format specifier
                match process_format_spec(env, &mut chars, state) {
                    Ok(FormatResult::Continue) => {}
                    Ok(FormatResult::Stop) => return Ok(rval),
                    Err(e) => {
                        env.stderr.write_line(&format!("printf: {}", e))?;
                        rval = 1;
                    }
                }
            }
        }
    }

    Ok(rval)
}

enum FormatResult {
    Continue,
    Stop, // For \c in %b
}

/// Process a single format specifier.
fn process_format_spec<SI, SO, SE, FS>(
    env: &Environment<SI, SO, SE, FS>,
    chars: &mut std::iter::Peekable<std::str::Chars>,
    state: &mut PrintfState,
) -> Result<FormatResult, String>
where
    SI: Stdin,
    SO: Stdout,
    SE: Stderr,
    FS: Filesystem,
{
    // Parse flags
    let mut flags = Flags::default();
    loop {
        match chars.peek() {
            Some('#') => {
                flags.alternate = true;
                chars.next();
            }
            Some('-') => {
                flags.left_justify = true;
                chars.next();
            }
            Some('+') => {
                flags.plus_sign = true;
                chars.next();
            }
            Some(' ') => {
                flags.space_sign = true;
                chars.next();
            }
            Some('0') => {
                flags.zero_pad = true;
                chars.next();
            }
            Some('\'') => {
                // Grouping flag (ignored in this implementation)
                chars.next();
            }
            _ => break,
        }
    }

    // Left justify overrides zero padding
    if flags.left_justify {
        flags.zero_pad = false;
    }
    // Plus sign overrides space sign
    if flags.plus_sign {
        flags.space_sign = false;
    }

    // Parse width
    let width = if chars.peek() == Some(&'*') {
        chars.next();
        let w = state.get_int().map_err(|e| format!("width: {}", e))?;
        if w < 0 {
            flags.left_justify = true;
            flags.zero_pad = false;
            (-w) as usize
        } else {
            w as usize
        }
    } else {
        parse_number(chars)
    };

    // Parse precision
    let precision = if chars.peek() == Some(&'.') {
        chars.next();
        if chars.peek() == Some(&'*') {
            chars.next();
            let p = state.get_int().map_err(|e| format!("precision: {}", e))?;
            Some(p.max(0) as usize)
        } else {
            Some(parse_number(chars))
        }
    } else {
        None
    };

    // Skip length modifier (L for long double)
    if chars.peek() == Some(&'L') {
        chars.next();
    }

    // Get conversion character
    let conv = chars.next().ok_or("missing format character")?;

    // Format based on conversion character
    let output = match conv {
        's' => {
            let s = state.get_string();
            format_string(s, width, precision, &flags)
        }
        'b' => {
            let s = state.get_string();
            let (processed, stop) = process_escapes(s, false);
            let output = format_string(&processed, width, precision, &flags);
            env.stdout
                .write_str(&output)
                .map_err(|e| format!("{:?}", e))?;
            if stop {
                return Ok(FormatResult::Stop);
            }
            return Ok(FormatResult::Continue);
        }
        'c' => {
            let c = state.get_char();
            if c == '\0' {
                String::new()
            } else {
                format_char(c, width, &flags)
            }
        }
        'd' | 'i' => {
            let n = state
                .get_int()
                .map_err(|e| format!("{}: expected numeric value", e))?;
            format_signed(n, 10, width, precision, &flags, false)
        }
        'o' => {
            let n = state
                .get_uint()
                .map_err(|e| format!("{}: expected numeric value", e))?;
            format_unsigned(n, 8, width, precision, &flags, false)
        }
        'u' => {
            let n = state
                .get_uint()
                .map_err(|e| format!("{}: expected numeric value", e))?;
            format_unsigned(n, 10, width, precision, &flags, false)
        }
        'x' => {
            let n = state
                .get_uint()
                .map_err(|e| format!("{}: expected numeric value", e))?;
            format_unsigned(n, 16, width, precision, &flags, false)
        }
        'X' => {
            let n = state
                .get_uint()
                .map_err(|e| format!("{}: expected numeric value", e))?;
            format_unsigned(n, 16, width, precision, &flags, true)
        }
        'f' | 'F' => {
            let n = state
                .get_float()
                .map_err(|e| format!("{}: expected numeric value", e))?;
            format_float_f(n, width, precision, &flags, conv == 'F')
        }
        'e' | 'E' => {
            let n = state
                .get_float()
                .map_err(|e| format!("{}: expected numeric value", e))?;
            format_float_e(n, width, precision, &flags, conv == 'E')
        }
        'g' | 'G' => {
            let n = state
                .get_float()
                .map_err(|e| format!("{}: expected numeric value", e))?;
            format_float_g(n, width, precision, &flags, conv == 'G')
        }
        'a' | 'A' => {
            let n = state
                .get_float()
                .map_err(|e| format!("{}: expected numeric value", e))?;
            format_float_a(n, width, precision, &flags, conv == 'A')
        }
        _ => return Err(format!("illegal format character {}", conv)),
    };

    env.stdout
        .write_str(&output)
        .map_err(|e| format!("{:?}", e))?;
    Ok(FormatResult::Continue)
}

/// Parse a number from the character stream.
fn parse_number(chars: &mut std::iter::Peekable<std::str::Chars>) -> usize {
    let mut n = 0usize;
    while let Some(&c) = chars.peek() {
        if c.is_ascii_digit() {
            n = n
                .saturating_mul(10)
                .saturating_add(c.to_digit(10).unwrap() as usize);
            chars.next();
        } else {
            break;
        }
    }
    n
}

#[derive(Default)]
struct Flags {
    alternate: bool,    // #
    left_justify: bool, // -
    plus_sign: bool,    // +
    space_sign: bool,   // ' '
    zero_pad: bool,     // 0
}

fn format_string(s: &str, width: usize, precision: Option<usize>, flags: &Flags) -> String {
    let s = if let Some(prec) = precision {
        if prec < s.len() { &s[..prec] } else { s }
    } else {
        s
    };

    if width > s.len() {
        if flags.left_justify {
            format!("{:<width$}", s, width = width)
        } else {
            format!("{:>width$}", s, width = width)
        }
    } else {
        s.to_string()
    }
}

fn format_char(c: char, width: usize, flags: &Flags) -> String {
    if width > 1 {
        if flags.left_justify {
            format!("{:<width$}", c, width = width)
        } else {
            format!("{:>width$}", c, width = width)
        }
    } else {
        c.to_string()
    }
}

fn format_signed(
    n: i64,
    radix: u32,
    width: usize,
    precision: Option<usize>,
    flags: &Flags,
    upper: bool,
) -> String {
    let abs_n = n.unsigned_abs();
    let mut digits = format_digits(abs_n, radix, upper);

    // Apply precision (minimum digits)
    if let Some(prec) = precision {
        if digits.len() < prec {
            digits = format!("{:0>width$}", digits, width = prec);
        } else if prec == 0 && n == 0 {
            digits = String::new();
        }
    }

    // Determine sign
    let sign = if n < 0 {
        "-"
    } else if flags.plus_sign {
        "+"
    } else if flags.space_sign {
        " "
    } else {
        ""
    };

    let total_len = sign.len() + digits.len();

    if width > total_len {
        if flags.left_justify {
            format!("{}{}{}", sign, digits, " ".repeat(width - total_len))
        } else if flags.zero_pad && precision.is_none() {
            let padded: String = format!("{:0>width$}", digits, width = width - sign.len());
            format!("{}{}", sign, padded)
        } else {
            format!("{}{}{}", " ".repeat(width - total_len), sign, digits)
        }
    } else {
        format!("{}{}", sign, digits)
    }
}

fn format_unsigned(
    n: u64,
    radix: u32,
    width: usize,
    precision: Option<usize>,
    flags: &Flags,
    upper: bool,
) -> String {
    let mut digits = format_digits(n, radix, upper);

    // Apply precision (minimum digits)
    if let Some(prec) = precision {
        if digits.len() < prec {
            digits = format!("{:0>width$}", digits, width = prec);
        } else if prec == 0 && n == 0 {
            digits = String::new();
        }
    }

    // Alternate form
    let prefix = if flags.alternate && n != 0 {
        match radix {
            8 => {
                // For octal, ensure leading zero
                if !digits.starts_with('0') {
                    digits = format!("0{}", digits);
                }
                ""
            }
            16 => {
                if upper {
                    "0X"
                } else {
                    "0x"
                }
            }
            _ => "",
        }
    } else {
        ""
    };

    let total_len = prefix.len() + digits.len();

    if width > total_len {
        if flags.left_justify {
            format!("{}{}{}", prefix, digits, " ".repeat(width - total_len))
        } else if flags.zero_pad && precision.is_none() {
            let padded: String = format!("{:0>width$}", digits, width = width - prefix.len());
            format!("{}{}", prefix, padded)
        } else {
            format!("{}{}{}", " ".repeat(width - total_len), prefix, digits)
        }
    } else {
        format!("{}{}", prefix, digits)
    }
}

fn format_digits(mut n: u64, radix: u32, upper: bool) -> String {
    if n == 0 {
        return "0".to_string();
    }

    let digit_chars: &[u8] = if upper {
        b"0123456789ABCDEF"
    } else {
        b"0123456789abcdef"
    };

    let mut digits = Vec::new();
    while n > 0 {
        digits.push(digit_chars[(n % radix as u64) as usize] as char);
        n /= radix as u64;
    }
    digits.reverse();
    digits.into_iter().collect()
}

fn format_float_f(
    n: f64,
    width: usize,
    precision: Option<usize>,
    flags: &Flags,
    upper: bool,
) -> String {
    let prec = precision.unwrap_or(6);

    // Handle special values
    if n.is_nan() {
        let s = if upper { "NAN" } else { "nan" };
        return pad_string(s, width, flags);
    }
    if n.is_infinite() {
        let s = if n.is_sign_positive() {
            if upper { "INF" } else { "inf" }
        } else if upper {
            "-INF"
        } else {
            "-inf"
        };
        return pad_string(s, width, flags);
    }

    let formatted = format!("{:.prec$}", n, prec = prec);
    apply_float_flags(&formatted, n, width, prec, flags)
}

fn format_float_e(
    n: f64,
    width: usize,
    precision: Option<usize>,
    flags: &Flags,
    upper: bool,
) -> String {
    let prec = precision.unwrap_or(6);

    // Handle special values
    if n.is_nan() {
        let s = if upper { "NAN" } else { "nan" };
        return pad_string(s, width, flags);
    }
    if n.is_infinite() {
        let s = if n.is_sign_positive() {
            if upper { "INF" } else { "inf" }
        } else if upper {
            "-INF"
        } else {
            "-inf"
        };
        return pad_string(s, width, flags);
    }

    let formatted = if upper {
        format!("{:.prec$E}", n, prec = prec)
    } else {
        format!("{:.prec$e}", n, prec = prec)
    };
    apply_float_flags(&formatted, n, width, prec, flags)
}

fn format_float_g(
    n: f64,
    width: usize,
    precision: Option<usize>,
    flags: &Flags,
    upper: bool,
) -> String {
    let prec = precision.unwrap_or(6).max(1);

    // Handle special values
    if n.is_nan() {
        let s = if upper { "NAN" } else { "nan" };
        return pad_string(s, width, flags);
    }
    if n.is_infinite() {
        let s = if n.is_sign_positive() {
            if upper { "INF" } else { "inf" }
        } else if upper {
            "-INF"
        } else {
            "-inf"
        };
        return pad_string(s, width, flags);
    }

    // Use %e or %f depending on exponent
    let exp = if n == 0.0 {
        0
    } else {
        n.abs().log10().floor() as i32
    };

    let formatted = if exp < -4 || exp >= prec as i32 {
        if upper {
            format!("{:.prec$E}", n, prec = prec.saturating_sub(1))
        } else {
            format!("{:.prec$e}", n, prec = prec.saturating_sub(1))
        }
    } else {
        let decimal_places = (prec as i32 - 1 - exp).max(0) as usize;
        format!("{:.prec$}", n, prec = decimal_places)
    };

    // Remove trailing zeros unless # flag
    let formatted = if !flags.alternate {
        remove_trailing_zeros(&formatted)
    } else {
        formatted
    };

    apply_float_flags(&formatted, n, width, prec, flags)
}

fn format_float_a(
    n: f64,
    width: usize,
    precision: Option<usize>,
    flags: &Flags,
    upper: bool,
) -> String {
    // Handle special values
    if n.is_nan() {
        let s = if upper { "NAN" } else { "nan" };
        return pad_string(s, width, flags);
    }
    if n.is_infinite() {
        let s = if n.is_sign_positive() {
            if upper { "INF" } else { "inf" }
        } else if upper {
            "-INF"
        } else {
            "-inf"
        };
        return pad_string(s, width, flags);
    }

    // Implement hex float formatting manually
    let formatted = format_hex_float(n, precision, upper);
    apply_float_flags(&formatted, n, width, precision.unwrap_or(6), flags)
}

/// Format a float in hexadecimal scientific notation (%a / %A).
fn format_hex_float(n: f64, precision: Option<usize>, upper: bool) -> String {
    if n == 0.0 {
        let sign = if n.is_sign_negative() { "-" } else { "" };
        let prec = precision.unwrap_or(0);
        let prefix = if upper { "0X" } else { "0x" };
        let p = if upper { "P" } else { "p" };
        if prec > 0 {
            return format!("{}{}0.{:0<prec$}{}+0", sign, prefix, "", p, prec = prec);
        } else {
            return format!("{}{}0{}+0", sign, prefix, p);
        }
    }

    let sign = if n.is_sign_negative() { "-" } else { "" };
    let n = n.abs();

    // Get bits of the float
    let bits = n.to_bits();
    let exp_bits = ((bits >> 52) & 0x7FF) as i64;
    let mantissa_bits = bits & 0xFFFFFFFFFFFFF;

    // Calculate actual exponent
    let exp = if exp_bits == 0 {
        // Subnormal
        -1022
    } else {
        exp_bits - 1023
    };

    // Format mantissa in hex
    let leading = if exp_bits == 0 { 0u64 } else { 1u64 };

    // Convert to hex digits
    let hex_chars: &[u8] = if upper {
        b"0123456789ABCDEF"
    } else {
        b"0123456789abcdef"
    };

    let prefix = if upper { "0X" } else { "0x" };
    let p = if upper { "P" } else { "p" };

    // Build the hex fraction
    let mut hex_digits = String::new();
    let mut m = mantissa_bits;
    for _ in 0..13 {
        // 52 bits / 4 = 13 hex digits
        let digit = ((m >> 48) & 0xF) as usize;
        hex_digits.push(hex_chars[digit] as char);
        m <<= 4;
    }

    // Trim trailing zeros unless precision specified
    let hex_digits = match precision {
        Some(prec) => {
            if prec == 0 {
                String::new()
            } else if hex_digits.len() > prec {
                hex_digits[..prec].to_string()
            } else {
                format!("{:0<width$}", hex_digits, width = prec)
            }
        }
        None => hex_digits.trim_end_matches('0').to_string(),
    };

    let exp_sign = if exp >= 0 { "+" } else { "" };

    if hex_digits.is_empty() {
        format!("{}{}{}{}{}{}", sign, prefix, leading, p, exp_sign, exp)
    } else {
        format!(
            "{}{}{}.{}{}{}{}",
            sign, prefix, leading, hex_digits, p, exp_sign, exp
        )
    }
}

fn remove_trailing_zeros(s: &str) -> String {
    // Find if there's an exponent
    let (mantissa, exp) = if let Some(pos) = s.find(['e', 'E']) {
        (&s[..pos], Some(&s[pos..]))
    } else {
        (s, None)
    };

    // Remove trailing zeros after decimal point
    let mantissa = if mantissa.contains('.') {
        mantissa.trim_end_matches('0').trim_end_matches('.')
    } else {
        mantissa
    };

    match exp {
        Some(e) => format!("{}{}", mantissa, e),
        None => mantissa.to_string(),
    }
}

fn apply_float_flags(formatted: &str, n: f64, width: usize, _prec: usize, flags: &Flags) -> String {
    // Determine sign prefix
    let sign = if formatted.starts_with('-') {
        ""
    } else if flags.plus_sign {
        "+"
    } else if flags.space_sign {
        " "
    } else {
        ""
    };

    // For alternate form, ensure decimal point is present
    let formatted =
        if flags.alternate && !formatted.contains('.') && !n.is_nan() && !n.is_infinite() {
            format!("{}.", formatted)
        } else {
            formatted.to_string()
        };

    let total = sign.len() + formatted.len();

    if width > total {
        if flags.left_justify {
            format!("{}{}{}", sign, formatted, " ".repeat(width - total))
        } else if flags.zero_pad {
            // Find where to insert zeros (after sign, before digits)
            if let Some(stripped) = formatted.strip_prefix('-') {
                format!("-{:0>width$}", stripped, width = width - 1)
            } else {
                format!("{}{:0>width$}", sign, formatted, width = width - sign.len())
            }
        } else {
            format!("{}{}{}", " ".repeat(width - total), sign, formatted)
        }
    } else {
        format!("{}{}", sign, formatted)
    }
}

fn pad_string(s: &str, width: usize, flags: &Flags) -> String {
    if width > s.len() {
        if flags.left_justify {
            format!("{:<width$}", s, width = width)
        } else {
            format!("{:>width$}", s, width = width)
        }
    } else {
        s.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::make_test_env;

    // ========================================================================
    // Basic tests
    // ========================================================================

    #[test]
    fn no_args_shows_usage() {
        let env = make_test_env(vec!["printf"]);
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("usage"));
    }

    #[test]
    fn simple_string() {
        let env = make_test_env(vec!["printf", "hello"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("hello", env.stdout.into_string());
    }

    #[test]
    fn string_with_newline() {
        let env = make_test_env(vec!["printf", "hello\\n"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("hello\n", env.stdout.into_string());
    }

    #[test]
    fn percent_percent() {
        let env = make_test_env(vec!["printf", "%%"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("%", env.stdout.into_string());
    }

    #[test]
    fn double_dash() {
        let env = make_test_env(vec!["printf", "--", "-d\\n"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("-d\n", env.stdout.into_string());
    }

    // ========================================================================
    // Escape sequences
    // ========================================================================

    #[test]
    fn escape_bell() {
        let env = make_test_env(vec!["printf", "\\a"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("\x07", env.stdout.into_string());
    }

    #[test]
    fn escape_backspace() {
        let env = make_test_env(vec!["printf", "\\b"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("\x08", env.stdout.into_string());
    }

    #[test]
    fn escape_tab() {
        let env = make_test_env(vec!["printf", "\\t"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("\t", env.stdout.into_string());
    }

    #[test]
    fn escape_carriage_return() {
        let env = make_test_env(vec!["printf", "\\r"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("\r", env.stdout.into_string());
    }

    #[test]
    fn escape_octal() {
        let env = make_test_env(vec!["printf", "\\101"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("A", env.stdout.into_string());
    }

    // ========================================================================
    // %s format
    // ========================================================================

    #[test]
    fn format_s_simple() {
        let env = make_test_env(vec!["printf", "%s", "hello"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("hello", env.stdout.into_string());
    }

    #[test]
    fn format_s_with_newline() {
        let env = make_test_env(vec!["printf", "%s\\n", "hello"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("hello\n", env.stdout.into_string());
    }

    #[test]
    fn format_s_width() {
        let env = make_test_env(vec!["printf", "%-5s", "abc"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("abc  ", env.stdout.into_string());
    }

    #[test]
    fn format_s_precision() {
        let env = make_test_env(vec!["printf", "%.3s", "abcdef"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("abc", env.stdout.into_string());
    }

    #[test]
    fn format_s_width_and_precision() {
        let env = make_test_env(vec!["printf", "%.3s,%-5s\\n", "abcd", "abc"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("abc,abc  \n", env.stdout.into_string());
    }

    #[test]
    fn format_s_missing_arg() {
        let env = make_test_env(vec!["printf", "%s"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("", env.stdout.into_string());
    }

    // ========================================================================
    // %c format
    // ========================================================================

    #[test]
    fn format_c_simple() {
        let env = make_test_env(vec!["printf", "%c", "abc"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("a", env.stdout.into_string());
    }

    #[test]
    fn format_c_empty() {
        let env = make_test_env(vec!["printf", "%c", ""]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("", env.stdout.into_string());
    }

    // ========================================================================
    // %b format
    // ========================================================================

    #[test]
    fn format_b_with_escapes() {
        let env = make_test_env(vec!["printf", "%b", "abc\\ndef"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("abc\ndef", env.stdout.into_string());
    }

    #[test]
    fn format_b_with_c_escape() {
        let env = make_test_env(vec!["printf", "abc%b%b", "def\\n", "\\cghi"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("abcdef\n", env.stdout.into_string());
    }

    #[test]
    fn format_b_octal_escape() {
        let env = make_test_env(vec!["printf", "%b", "\\0101"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("A", env.stdout.into_string());
    }

    // ========================================================================
    // %d format
    // ========================================================================

    #[test]
    fn format_d_simple() {
        let env = make_test_env(vec!["printf", "%d", "42"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("42", env.stdout.into_string());
    }

    #[test]
    fn format_d_negative() {
        let env = make_test_env(vec!["printf", "%d", "-42"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("-42", env.stdout.into_string());
    }

    #[test]
    fn format_d_width() {
        let env = make_test_env(vec!["printf", "%5d", "123"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("  123", env.stdout.into_string());
    }

    #[test]
    fn format_d_zero_pad() {
        let env = make_test_env(vec!["printf", "%05d", "123"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("00123", env.stdout.into_string());
    }

    #[test]
    fn format_d_precision() {
        let env = make_test_env(vec!["printf", "%.5d", "123"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("00123", env.stdout.into_string());
    }

    #[test]
    fn format_d_plus_sign() {
        let env = make_test_env(vec!["printf", "%+d\\n%d\\n%d\\n", "1", "-2", "13"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("+1\n-2\n13\n", env.stdout.into_string());
    }

    #[test]
    fn format_d_complex() {
        let env = make_test_env(vec![
            "printf",
            "%d,%5d,%.5d,%0*d,%.*d\\n",
            "123",
            "123",
            "123",
            "5",
            "123",
            "5",
            "123",
        ]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("123,  123,00123,00123,00123\n", env.stdout.into_string());
    }

    #[test]
    fn format_d_missing_arg() {
        let env = make_test_env(vec!["printf", "%d"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("0", env.stdout.into_string());
    }

    #[test]
    fn format_d_char_constant() {
        // '"abc' should give ASCII value of 'a'
        let env = make_test_env(vec!["printf", "%d", "\"a"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("97", env.stdout.into_string());
    }

    // ========================================================================
    // %o, %u, %x, %X formats
    // ========================================================================

    #[test]
    fn format_o_simple() {
        let env = make_test_env(vec!["printf", "%o", "8"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("10", env.stdout.into_string());
    }

    #[test]
    fn format_u_simple() {
        let env = make_test_env(vec!["printf", "%u", "42"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("42", env.stdout.into_string());
    }

    #[test]
    fn format_x_simple() {
        let env = make_test_env(vec!["printf", "%x", "255"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("ff", env.stdout.into_string());
    }

    #[test]
    fn format_big_x_simple() {
        let env = make_test_env(vec!["printf", "%X", "255"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("FF", env.stdout.into_string());
    }

    #[test]
    fn format_x_alternate() {
        let env = make_test_env(vec!["printf", "%#x", "255"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("0xff", env.stdout.into_string());
    }

    // ========================================================================
    // %f format
    // ========================================================================

    #[test]
    fn format_f_simple() {
        let env = make_test_env(vec!["printf", "%f", "42.25"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("42.250000", env.stdout.into_string());
    }

    #[test]
    fn format_f_precision() {
        let env = make_test_env(vec!["printf", "%.2f", "31.7456"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("31.75", env.stdout.into_string());
    }

    #[test]
    fn format_f_width() {
        let env = make_test_env(vec!["printf", "%-8.3f", "-42.25"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("-42.250 ", env.stdout.into_string());
    }

    #[test]
    fn format_f_complex() {
        let env = make_test_env(vec![
            "printf",
            "%f,%-8.3f,%f,%f\\n",
            "+42.25",
            "-42.25",
            "inf",
            "nan",
        ]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("42.250000,-42.250 ,inf,nan\n", env.stdout.into_string());
    }

    #[test]
    fn format_f_missing_arg() {
        let env = make_test_env(vec!["printf", "%f"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("0.000000", env.stdout.into_string());
    }

    // ========================================================================
    // Format reuse
    // ========================================================================

    #[test]
    fn format_reuse() {
        let env = make_test_env(vec!["printf", "%%%s\\n", "abc", "def", "ghi", "jkl"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("%abc\n%def\n%ghi\n%jkl\n", env.stdout.into_string());
    }

    #[test]
    fn format_reuse_with_sign() {
        let env = make_test_env(vec!["printf", "%+d\\n", "1", "-2", "13"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("+1\n-2\n+13\n", env.stdout.into_string());
    }

    #[test]
    fn missing_args_use_defaults() {
        let env = make_test_env(vec!["printf", "%d,%f,%c,%s\\n"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("0,0.000000,,\n", env.stdout.into_string());
    }

    // ========================================================================
    // Zero handling (from regression tests)
    // ========================================================================

    #[test]
    fn zero_handling_u_u() {
        let env = make_test_env(vec!["printf", "%u%u\\n", "15"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("150\n", env.stdout.into_string());
    }

    #[test]
    fn zero_handling_d_d() {
        let env = make_test_env(vec!["printf", "%d%d\\n", "15"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("150\n", env.stdout.into_string());
    }

    // ========================================================================
    // Width from argument (*)
    // ========================================================================

    #[test]
    fn width_from_arg() {
        let env = make_test_env(vec!["printf", "%*d", "5", "42"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("   42", env.stdout.into_string());
    }

    #[test]
    fn precision_from_arg() {
        let env = make_test_env(vec!["printf", "%.*d", "5", "42"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("00042", env.stdout.into_string());
    }

    #[test]
    fn negative_width_left_justifies() {
        let env = make_test_env(vec!["printf", "%*d", "-5", "42"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("42   ", env.stdout.into_string());
    }

    // ========================================================================
    // %b width (from regression)
    // ========================================================================

    #[test]
    fn format_b_width() {
        let env = make_test_env(vec!["printf", "%8.2b", "a\\nb\\n"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        // Precision .2 limits to 2 chars, width 8 pads
        assert_eq!("      a\n", env.stdout.into_string());
    }

    // ========================================================================
    // Input parsing
    // ========================================================================

    #[test]
    fn parse_hex_input() {
        let env = make_test_env(vec!["printf", "%d", "0xff"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("255", env.stdout.into_string());
    }

    #[test]
    fn parse_octal_input() {
        let env = make_test_env(vec!["printf", "%d", "010"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("8", env.stdout.into_string());
    }
}

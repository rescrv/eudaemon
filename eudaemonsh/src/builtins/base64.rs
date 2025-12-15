//! The base64 builtin: encode or decode data using Base64 encoding (RFC 4648).

use getopts::Options;

use crate::{Environment, Error, ExitCode, Filesystem, FsError, Stderr, Stdin, Stdout};

/// Standard Base64 alphabet (RFC 4648).
const BASE64_ALPHABET: &[u8; 64] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Build a decoding table from the alphabet.
fn build_decode_table() -> [i8; 256] {
    let mut table = [-1i8; 256];
    for (i, &byte) in BASE64_ALPHABET.iter().enumerate() {
        table[byte as usize] = i as i8;
    }
    table
}

/// Encode bytes to Base64 string.
fn encode_base64(input: &[u8]) -> String {
    let mut output = String::with_capacity(input.len().div_ceil(3) * 4);

    for chunk in input.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
        let b2 = chunk.get(2).copied().unwrap_or(0) as u32;

        let triple = (b0 << 16) | (b1 << 8) | b2;

        output.push(BASE64_ALPHABET[(triple >> 18) as usize & 0x3F] as char);
        output.push(BASE64_ALPHABET[(triple >> 12) as usize & 0x3F] as char);

        if chunk.len() > 1 {
            output.push(BASE64_ALPHABET[(triple >> 6) as usize & 0x3F] as char);
        } else {
            output.push('=');
        }

        if chunk.len() > 2 {
            output.push(BASE64_ALPHABET[triple as usize & 0x3F] as char);
        } else {
            output.push('=');
        }
    }

    output
}

/// Decode Base64 string to bytes. Returns None on invalid input.
fn decode_base64(input: &str) -> Option<Vec<u8>> {
    let table = build_decode_table();
    let mut output = Vec::with_capacity(input.len() * 3 / 4);

    // Filter out whitespace and collect valid characters
    let chars: Vec<u8> = input
        .bytes()
        .filter(|&b| !b.is_ascii_whitespace())
        .collect();

    if chars.is_empty() {
        return Some(output);
    }

    // Process in chunks of 4
    let mut i = 0;
    while i < chars.len() {
        // Get up to 4 characters
        let c0 = chars.get(i)?;
        let c1 = chars.get(i + 1)?;
        let c2 = chars.get(i + 2).copied().unwrap_or(b'=');
        let c3 = chars.get(i + 3).copied().unwrap_or(b'=');

        // Decode each character
        let v0 = table[*c0 as usize];
        let v1 = table[*c1 as usize];

        if v0 < 0 || v1 < 0 {
            return None;
        }

        // First byte
        output.push(((v0 as u8) << 2) | ((v1 as u8) >> 4));

        // Second byte (if not padding)
        if c2 != b'=' {
            let v2 = table[c2 as usize];
            if v2 < 0 {
                return None;
            }
            output.push(((v1 as u8) << 4) | ((v2 as u8) >> 2));

            // Third byte (if not padding)
            if c3 != b'=' {
                let v3 = table[c3 as usize];
                if v3 < 0 {
                    return None;
                }
                output.push(((v2 as u8) << 6) | (v3 as u8));
            }
        }

        i += 4;
    }

    Some(output)
}

/// Wrap a string at the specified column width.
fn wrap_output(s: &str, wrap: usize) -> String {
    if wrap == 0 {
        return s.to_string();
    }

    let mut output = String::with_capacity(s.len() + s.len() / wrap);
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && i % wrap == 0 {
            output.push('\n');
        }
        output.push(ch);
    }
    output
}

fn build_options() -> Options {
    let mut opts = Options::new();
    opts.optflag("d", "decode", "Decode data.");
    opts.optflag(
        "i",
        "ignore-garbage",
        "When decoding, ignore non-alphabet characters.",
    );
    opts.optopt(
        "w",
        "wrap",
        "Wrap encoded lines at COLS character (default 76). Use 0 to disable.",
        "COLS",
    );
    opts.optflag("h", "help", "Display this help and exit.");
    opts
}

/// The base64 builtin: encode or decode data using Base64 encoding.
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
            env.stderr.write_line(&format!("base64: {}", e))?;
            return Ok(ExitCode::from(1));
        }
    };

    if matches.opt_present("h") {
        env.stdout.write_line("Usage: base64 [OPTION]... [FILE]")?;
        env.stdout
            .write_line("Base64 encode or decode FILE, or standard input, to standard output.")?;
        env.stdout.write_line("")?;
        env.stdout
            .write_line("  -d, --decode          decode data")?;
        env.stdout
            .write_line("  -i, --ignore-garbage  when decoding, ignore non-alphabet characters")?;
        env.stdout.write_line(
            "  -w, --wrap=COLS       wrap encoded lines after COLS character (default 76)",
        )?;
        env.stdout
            .write_line("                        use 0 to disable line wrapping")?;
        env.stdout
            .write_line("  -h, --help            display this help and exit")?;
        return Ok(ExitCode::from(0));
    }

    let decode = matches.opt_present("d");
    let ignore_garbage = matches.opt_present("i");
    let wrap_cols: usize = match matches.opt_str("w") {
        Some(s) => match s.parse() {
            Ok(n) => n,
            Err(_) => {
                env.stderr
                    .write_line(&format!("base64: invalid wrap size: '{}'", s))?;
                return Ok(ExitCode::from(1));
            }
        },
        None => 76,
    };

    // Get input from file or stdin
    let input = if matches.free.is_empty() || matches.free[0] == "-" {
        // Read from stdin
        let mut lines = Vec::new();
        while let Some(line) = env.stdin.read_line()? {
            lines.push(line);
        }
        lines.join("\n")
    } else {
        // Read from file
        match env.fs.read_to_string(&matches.free[0]) {
            Ok(contents) => contents,
            Err(FsError::Io(e)) => {
                env.stderr
                    .write_line(&format!("base64: {}: {}", matches.free[0], e))?;
                return Ok(ExitCode::from(1));
            }
        }
    };

    if decode {
        // Decode mode
        let filtered_input = if ignore_garbage {
            input
                .chars()
                .filter(|c| {
                    c.is_ascii_alphanumeric()
                        || *c == '+'
                        || *c == '/'
                        || *c == '='
                        || c.is_ascii_whitespace()
                })
                .collect::<String>()
        } else {
            input
        };

        match decode_base64(&filtered_input) {
            Some(decoded) => {
                // Output decoded bytes as string (assuming UTF-8 or raw output)
                match String::from_utf8(decoded) {
                    Ok(s) => env.stdout.write_str(&s)?,
                    Err(e) => {
                        // Output raw bytes as lossy UTF-8
                        env.stdout
                            .write_str(&String::from_utf8_lossy(e.as_bytes()))?;
                    }
                }
            }
            None => {
                env.stderr.write_line("base64: invalid input")?;
                return Ok(ExitCode::from(1));
            }
        }
    } else {
        // Encode mode
        let encoded = encode_base64(input.as_bytes());
        let wrapped = wrap_output(&encoded, wrap_cols);
        env.stdout.write_line(&wrapped)?;
    }

    Ok(ExitCode::from(0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TestFilesystemExt;
    use crate::test_utils::make_test_env_with_stdin;

    // ========================================================================
    // encode_base64 unit tests
    // ========================================================================

    #[test]
    fn encode_empty() {
        assert_eq!("", encode_base64(b""));
    }

    #[test]
    fn encode_single_char() {
        assert_eq!("YQ==", encode_base64(b"a"));
    }

    #[test]
    fn encode_two_chars() {
        assert_eq!("YWI=", encode_base64(b"ab"));
    }

    #[test]
    fn encode_three_chars() {
        assert_eq!("YWJj", encode_base64(b"abc"));
    }

    #[test]
    fn encode_hello_world() {
        assert_eq!("SGVsbG8gV29ybGQ=", encode_base64(b"Hello World"));
    }

    #[test]
    fn encode_man() {
        // Classic test vector from RFC 4648
        assert_eq!("TWFu", encode_base64(b"Man"));
    }

    #[test]
    fn encode_ma() {
        assert_eq!("TWE=", encode_base64(b"Ma"));
    }

    #[test]
    fn encode_m() {
        assert_eq!("TQ==", encode_base64(b"M"));
    }

    // ========================================================================
    // decode_base64 unit tests
    // ========================================================================

    #[test]
    fn decode_empty() {
        assert_eq!(Some(vec![]), decode_base64(""));
    }

    #[test]
    fn decode_single_char() {
        assert_eq!(Some(b"a".to_vec()), decode_base64("YQ=="));
    }

    #[test]
    fn decode_two_chars() {
        assert_eq!(Some(b"ab".to_vec()), decode_base64("YWI="));
    }

    #[test]
    fn decode_three_chars() {
        assert_eq!(Some(b"abc".to_vec()), decode_base64("YWJj"));
    }

    #[test]
    fn decode_hello_world() {
        assert_eq!(
            Some(b"Hello World".to_vec()),
            decode_base64("SGVsbG8gV29ybGQ=")
        );
    }

    #[test]
    fn decode_with_whitespace() {
        assert_eq!(
            Some(b"Hello World".to_vec()),
            decode_base64("SGVs bG8g V29y bGQ=")
        );
    }

    #[test]
    fn decode_with_newlines() {
        assert_eq!(
            Some(b"Hello World".to_vec()),
            decode_base64("SGVsbG8g\nV29ybGQ=")
        );
    }

    #[test]
    fn decode_invalid_char() {
        assert_eq!(None, decode_base64("SGVs!G8="));
    }

    #[test]
    fn decode_man() {
        assert_eq!(Some(b"Man".to_vec()), decode_base64("TWFu"));
    }

    #[test]
    fn decode_ma() {
        assert_eq!(Some(b"Ma".to_vec()), decode_base64("TWE="));
    }

    #[test]
    fn decode_m() {
        assert_eq!(Some(b"M".to_vec()), decode_base64("TQ=="));
    }

    // ========================================================================
    // wrap_output unit tests
    // ========================================================================

    #[test]
    fn wrap_zero_disabled() {
        assert_eq!("abcdefghij", wrap_output("abcdefghij", 0));
    }

    #[test]
    fn wrap_at_4() {
        assert_eq!("abcd\nefgh\nij", wrap_output("abcdefghij", 4));
    }

    #[test]
    fn wrap_longer_than_input() {
        assert_eq!("abc", wrap_output("abc", 10));
    }

    #[test]
    fn wrap_exact_multiple() {
        assert_eq!("abcd\nefgh", wrap_output("abcdefgh", 4));
    }

    // ========================================================================
    // Integration tests - encoding
    // ========================================================================

    #[test]
    fn encode_stdin_simple() {
        let env = make_test_env_with_stdin(vec!["base64"], "Hello");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("SGVsbG8=\n", env.stdout.into_string());
    }

    #[test]
    fn encode_stdin_empty() {
        let env = make_test_env_with_stdin(vec!["base64"], "");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("\n", env.stdout.into_string());
    }

    #[test]
    fn encode_file() {
        let env = make_test_env_with_stdin(vec!["base64", "input.txt"], "");
        env.fs.add_file("input.txt", "Hello");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("SGVsbG8=\n", env.stdout.into_string());
    }

    #[test]
    fn encode_file_not_found() {
        let env = make_test_env_with_stdin(vec!["base64", "nonexistent.txt"], "");
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("nonexistent.txt"));
    }

    #[test]
    fn encode_with_wrap_0() {
        // Generate a long string that would normally wrap
        let input = "a".repeat(100);
        let env = make_test_env_with_stdin(vec!["base64", "-w", "0"], &input);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let output = env.stdout.into_string();
        println!("output: {:?}", output);
        // Should be one line (plus trailing newline)
        assert!(!output.trim().contains('\n'));
    }

    #[test]
    fn encode_with_wrap_20() {
        let input = "a".repeat(30);
        let env = make_test_env_with_stdin(vec!["base64", "-w", "20"], &input);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let output = env.stdout.into_string();
        println!("output: {:?}", output);
        // Each line should be at most 20 characters
        for line in output.trim().lines() {
            assert!(line.len() <= 20, "line too long: {}", line);
        }
    }

    #[test]
    fn encode_with_invalid_wrap() {
        let env = make_test_env_with_stdin(vec!["base64", "-w", "abc"], "Hello");
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("invalid wrap size"));
    }

    // ========================================================================
    // Integration tests - decoding
    // ========================================================================

    #[test]
    fn decode_stdin_simple() {
        let env = make_test_env_with_stdin(vec!["base64", "-d"], "SGVsbG8=");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("Hello", env.stdout.into_string());
    }

    #[test]
    fn decode_stdin_empty() {
        let env = make_test_env_with_stdin(vec!["base64", "-d"], "");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("", env.stdout.into_string());
    }

    #[test]
    fn decode_file() {
        let env = make_test_env_with_stdin(vec!["base64", "-d", "input.txt"], "");
        env.fs.add_file("input.txt", "SGVsbG8=");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("Hello", env.stdout.into_string());
    }

    #[test]
    fn decode_invalid_input() {
        let env = make_test_env_with_stdin(vec!["base64", "-d"], "!!!invalid!!!");
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("invalid input"));
    }

    #[test]
    fn decode_with_ignore_garbage() {
        let env = make_test_env_with_stdin(vec!["base64", "-d", "-i"], "SGVs!!!bG8=");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("Hello", env.stdout.into_string());
    }

    #[test]
    fn decode_stdin_with_whitespace() {
        let env = make_test_env_with_stdin(vec!["base64", "-d"], "SGVs\nbG8=");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("Hello", env.stdout.into_string());
    }

    // ========================================================================
    // Help flag
    // ========================================================================

    #[test]
    fn help_flag() {
        let env = make_test_env_with_stdin(vec!["base64", "-h"], "");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert!(stdout.contains("Usage:"));
        assert!(stdout.contains("--decode"));
    }

    // ========================================================================
    // Round-trip tests
    // ========================================================================

    #[test]
    fn roundtrip_hello() {
        let original = b"Hello, World!";
        let encoded = encode_base64(original);
        let decoded = decode_base64(&encoded).unwrap();
        assert_eq!(original.to_vec(), decoded);
    }

    #[test]
    fn roundtrip_binary() {
        let original: Vec<u8> = (0..=255).collect();
        let encoded = encode_base64(&original);
        let decoded = decode_base64(&encoded).unwrap();
        assert_eq!(original, decoded);
    }

    #[test]
    fn roundtrip_empty() {
        let original = b"";
        let encoded = encode_base64(original);
        let decoded = decode_base64(&encoded).unwrap();
        assert_eq!(original.to_vec(), decoded);
    }

    // ========================================================================
    // Edge cases
    // ========================================================================

    #[test]
    fn dash_means_stdin() {
        let env = make_test_env_with_stdin(vec!["base64", "-"], "Hello");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("SGVsbG8=\n", env.stdout.into_string());
    }

    #[test]
    fn illegal_option() {
        let env = make_test_env_with_stdin(vec!["base64", "-x"], "");
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("Unrecognized option"));
    }

    #[test]
    fn long_option_decode() {
        let env = make_test_env_with_stdin(vec!["base64", "--decode"], "SGVsbG8=");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("Hello", env.stdout.into_string());
    }

    #[test]
    fn long_option_wrap() {
        let input = "a".repeat(30);
        let env = make_test_env_with_stdin(vec!["base64", "--wrap=10"], &input);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let output = env.stdout.into_string();
        for line in output.trim().lines() {
            assert!(line.len() <= 10, "line too long: {}", line);
        }
    }
}

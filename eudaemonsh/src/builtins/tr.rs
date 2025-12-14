//! The tr utility: translate, squeeze, and/or delete characters.

use std::collections::HashSet;

use getopts::Options;

use crate::{Environment, Error, ExitCode, Filesystem, Stderr, Stdin, Stdout};

/// Options for the tr command.
#[derive(Clone, Debug, Default)]
struct TrOptions {
    /// Complement the set of characters in string1 (-C/-c).
    complement: bool,
    /// Delete characters in string1 (-d).
    delete: bool,
    /// Squeeze repeated output characters (-s).
    squeeze: bool,
}

/// A parsed character set from a tr string specification.
#[derive(Clone, Debug)]
struct CharSet {
    /// The characters in order (for translation mapping).
    chars: Vec<char>,
}

impl CharSet {
    /// Create a new empty character set.
    fn new() -> Self {
        Self { chars: Vec::new() }
    }

    /// Parse a tr string specification into a character set.
    fn parse(spec: &str, is_string2: bool) -> Result<Self, String> {
        let mut cs = CharSet::new();
        let chars: Vec<char> = spec.chars().collect();
        let mut i = 0;

        while i < chars.len() {
            match chars[i] {
                '\\' => {
                    i += 1;
                    if i >= chars.len() {
                        cs.chars.push('\\');
                        break;
                    }
                    let (ch, advance) = parse_backslash(&chars[i..]);
                    cs.chars.push(ch);
                    i += advance;
                }
                '[' => {
                    if i + 1 < chars.len() && chars[i + 1] == ':' {
                        let (class_chars, advance) = parse_class(&chars[i..])?;
                        cs.chars.extend(class_chars);
                        i += advance;
                    } else if i + 1 < chars.len() && chars[i + 1] == '=' {
                        let (equiv_chars, advance) = parse_equiv(&chars[i..])?;
                        cs.chars.extend(equiv_chars);
                        i += advance;
                    } else if is_string2 {
                        let (repeat_char, count, advance) = parse_repeat(&chars[i..])?;
                        for _ in 0..count {
                            cs.chars.push(repeat_char);
                        }
                        i += advance;
                    } else {
                        cs.chars.push('[');
                        i += 1;
                    }
                }
                c => {
                    if i + 2 < chars.len() && chars[i + 1] == '-' && chars[i + 2] != ']' {
                        let start = c;
                        let end = chars[i + 2];
                        if start <= end {
                            for ch in start..=end {
                                cs.chars.push(ch);
                            }
                        } else {
                            cs.chars.push(c);
                            cs.chars.push('-');
                            cs.chars.push(end);
                        }
                        i += 3;
                    } else {
                        cs.chars.push(c);
                        i += 1;
                    }
                }
            }
        }

        Ok(cs)
    }

    /// Get a HashSet of the characters for membership testing.
    fn to_set(&self) -> HashSet<char> {
        self.chars.iter().copied().collect()
    }

    /// Complement the character set (all chars not in this set, ASCII only for simplicity).
    fn complement(&self) -> Self {
        let set = self.to_set();
        let mut chars = Vec::new();
        for c in 0u8..=127 {
            let ch = c as char;
            if !set.contains(&ch) {
                chars.push(ch);
            }
        }
        Self { chars }
    }
}

/// Parse a backslash escape sequence.
/// Returns (character, number of chars consumed after backslash).
fn parse_backslash(chars: &[char]) -> (char, usize) {
    if chars.is_empty() {
        return ('\\', 0);
    }

    let mut i = 0;
    let first = chars[i];

    if first.is_ascii_digit() && first <= '7' {
        let mut val = 0u32;
        let mut count = 0;
        while i < chars.len() && count < 3 {
            let c = chars[i];
            if c.is_ascii_digit() && c <= '7' {
                val = val * 8 + (c as u32 - '0' as u32);
                i += 1;
                count += 1;
            } else {
                break;
            }
        }
        return (char::from_u32(val).unwrap_or('\0'), i);
    }

    let ch = match first {
        'a' => '\x07',
        'b' => '\x08',
        'f' => '\x0C',
        'n' => '\n',
        'r' => '\r',
        't' => '\t',
        'v' => '\x0B',
        '\\' => '\\',
        c => c,
    };
    (ch, 1)
}

/// Parse a character class like [:alpha:].
/// Returns (characters in class, number of chars consumed).
fn parse_class(chars: &[char]) -> Result<(Vec<char>, usize), String> {
    if chars.len() < 4 || chars[0] != '[' || chars[1] != ':' {
        return Err("invalid character class".to_string());
    }

    let close_pos = chars
        .iter()
        .enumerate()
        .skip(2)
        .find(|(i, _)| *i + 1 < chars.len() && chars[*i] == ':' && chars[*i + 1] == ']')
        .map(|(i, _)| i);

    let close_pos = close_pos.ok_or_else(|| "unterminated character class".to_string())?;

    let class_name: String = chars[2..close_pos].iter().collect();
    let class_chars = get_class_chars(&class_name)?;

    Ok((class_chars, close_pos + 2))
}

/// Get the characters in a character class.
fn get_class_chars(class_name: &str) -> Result<Vec<char>, String> {
    let mut chars = Vec::new();

    match class_name {
        "alnum" => {
            for c in 'a'..='z' {
                chars.push(c);
            }
            for c in 'A'..='Z' {
                chars.push(c);
            }
            for c in '0'..='9' {
                chars.push(c);
            }
        }
        "alpha" => {
            for c in 'a'..='z' {
                chars.push(c);
            }
            for c in 'A'..='Z' {
                chars.push(c);
            }
        }
        "blank" => {
            chars.push(' ');
            chars.push('\t');
        }
        "cntrl" => {
            for c in 0u8..32 {
                chars.push(c as char);
            }
            chars.push(127 as char);
        }
        "digit" => {
            for c in '0'..='9' {
                chars.push(c);
            }
        }
        "graph" => {
            for c in '!'..='~' {
                chars.push(c);
            }
        }
        "lower" => {
            for c in 'a'..='z' {
                chars.push(c);
            }
        }
        "print" => {
            for c in ' '..='~' {
                chars.push(c);
            }
        }
        "punct" => {
            for c in '!'..='/' {
                chars.push(c);
            }
            for c in ':'..='@' {
                chars.push(c);
            }
            for c in '['..='`' {
                chars.push(c);
            }
            for c in '{'..='~' {
                chars.push(c);
            }
        }
        "space" => {
            chars.push(' ');
            chars.push('\t');
            chars.push('\n');
            chars.push('\r');
            chars.push('\x0B');
            chars.push('\x0C');
        }
        "upper" => {
            for c in 'A'..='Z' {
                chars.push(c);
            }
        }
        "xdigit" => {
            for c in '0'..='9' {
                chars.push(c);
            }
            for c in 'a'..='f' {
                chars.push(c);
            }
            for c in 'A'..='F' {
                chars.push(c);
            }
        }
        _ => return Err(format!("unknown class: {}", class_name)),
    }

    Ok(chars)
}

/// Parse an equivalence class like [=e=].
/// Returns (characters in equivalence class, number of chars consumed).
fn parse_equiv(chars: &[char]) -> Result<(Vec<char>, usize), String> {
    if chars.len() < 4 || chars[0] != '[' || chars[1] != '=' {
        return Err("invalid equivalence class".to_string());
    }

    let close_pos = chars
        .iter()
        .enumerate()
        .skip(2)
        .find(|(i, _)| *i + 1 < chars.len() && chars[*i] == '=' && chars[*i + 1] == ']')
        .map(|(i, _)| i);

    let close_pos = close_pos.ok_or_else(|| "unterminated equivalence class".to_string())?;

    let equiv_char = if close_pos > 2 {
        chars[2]
    } else {
        return Err("empty equivalence class".to_string());
    };

    Ok((vec![equiv_char], close_pos + 2))
}

/// Parse a repeat specification like [c*n] or [c*].
/// Returns (character, count, number of chars consumed).
/// A count of 0 means "infinite" (extend to length of string1).
fn parse_repeat(chars: &[char]) -> Result<(char, usize, usize), String> {
    if chars.is_empty() || chars[0] != '[' {
        return Err("invalid repeat specification".to_string());
    }

    let close_pos = chars
        .iter()
        .position(|&c| c == ']')
        .ok_or_else(|| "unterminated repeat specification".to_string())?;

    if close_pos < 3 {
        return Err("invalid repeat specification".to_string());
    }

    let star_pos = chars[1..close_pos]
        .iter()
        .position(|&c| c == '*')
        .map(|p| p + 1);

    let star_pos = star_pos.ok_or_else(|| "invalid repeat specification".to_string())?;

    let repeat_char = if chars[1] == '\\' && star_pos > 2 {
        let (ch, _) = parse_backslash(&chars[2..star_pos]);
        ch
    } else {
        chars[1]
    };

    let count_str: String = chars[star_pos + 1..close_pos].iter().collect();
    let count = if count_str.is_empty() {
        0
    } else if count_str.starts_with('0') {
        usize::from_str_radix(&count_str, 8).map_err(|_| "illegal sequence count".to_string())?
    } else {
        count_str
            .parse()
            .map_err(|_| "illegal sequence count".to_string())?
    };

    Ok((repeat_char, count, close_pos + 1))
}

fn build_options() -> Options {
    let mut opts = Options::new();
    opts.optflag(
        "C",
        "",
        "Complement the set of characters in string1 (character set)",
    );
    opts.optflag(
        "c",
        "",
        "Complement the set of values in string1 (byte values)",
    );
    opts.optflag("d", "", "Delete characters in string1 from the input");
    opts.optflag(
        "s",
        "",
        "Squeeze multiple occurrences of characters in the last operand",
    );
    opts.optflag("u", "", "Guarantee unbuffered output");
    opts
}

/// The tr builtin: translate, squeeze, and/or delete characters.
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
            env.stderr.write_line(&format!("tr: {}", e))?;
            return Ok(ExitCode::from(1));
        }
    };

    let mut tr_opts = TrOptions::default();

    if matches.opt_present("C") || matches.opt_present("c") {
        tr_opts.complement = true;
    }

    if matches.opt_present("d") {
        tr_opts.delete = true;
    }

    if matches.opt_present("s") {
        tr_opts.squeeze = true;
    }

    match matches.free.len() {
        0 => {
            env.stderr.write_line("tr: missing operand")?;
            return Ok(ExitCode::from(1));
        }
        1 => {
            if !tr_opts.delete && !tr_opts.squeeze {
                env.stderr.write_line("tr: missing operand after string1")?;
                return Ok(ExitCode::from(1));
            }
        }
        2 => {}
        _ => {
            env.stderr.write_line("tr: extra operand")?;
            return Ok(ExitCode::from(1));
        }
    }

    let string1 = &matches.free[0];
    let string2 = matches.free.get(1).map(|s| s.as_str());

    let set1 = match CharSet::parse(string1, false) {
        Ok(s) => s,
        Err(e) => {
            env.stderr.write_line(&format!("tr: {}", e))?;
            return Ok(ExitCode::from(1));
        }
    };

    let set1 = if tr_opts.complement {
        set1.complement()
    } else {
        set1
    };

    let set2 = if let Some(s2) = string2 {
        match CharSet::parse(s2, true) {
            Ok(s) => Some(s),
            Err(e) => {
                env.stderr.write_line(&format!("tr: {}", e))?;
                return Ok(ExitCode::from(1));
            }
        }
    } else {
        None
    };

    if tr_opts.delete && tr_opts.squeeze {
        tr_delete_squeeze(env, &set1, set2.as_ref().unwrap())
    } else if tr_opts.delete {
        tr_delete(env, &set1)
    } else if tr_opts.squeeze && string2.is_none() {
        tr_squeeze_only(env, &set1)
    } else {
        tr_translate(env, &set1, set2.as_ref().unwrap(), tr_opts.squeeze)
    }
}

/// Delete characters in set1 from input.
fn tr_delete<SI, SO, SE, FS>(
    env: &Environment<SI, SO, SE, FS>,
    set1: &CharSet,
) -> Result<ExitCode, Error>
where
    SI: Stdin,
    SO: Stdout,
    SE: Stderr,
    FS: Filesystem,
{
    let delete_set = set1.to_set();

    while let Some(line) = env.stdin.read_line()? {
        let mut output = String::new();
        for ch in line.chars() {
            if !delete_set.contains(&ch) {
                output.push(ch);
            }
        }
        env.stdout.write_line(&output)?;
    }

    Ok(ExitCode::from(0))
}

/// Delete characters in set1 and squeeze characters in set2.
fn tr_delete_squeeze<SI, SO, SE, FS>(
    env: &Environment<SI, SO, SE, FS>,
    set1: &CharSet,
    set2: &CharSet,
) -> Result<ExitCode, Error>
where
    SI: Stdin,
    SO: Stdout,
    SE: Stderr,
    FS: Filesystem,
{
    let delete_set = set1.to_set();
    let squeeze_set = set2.to_set();

    while let Some(line) = env.stdin.read_line()? {
        let mut output = String::new();
        let mut last_ch: Option<char> = None;

        for ch in line.chars() {
            if delete_set.contains(&ch) {
                continue;
            }
            if last_ch == Some(ch) && squeeze_set.contains(&ch) {
                continue;
            }
            output.push(ch);
            last_ch = Some(ch);
        }
        env.stdout.write_line(&output)?;
    }

    Ok(ExitCode::from(0))
}

/// Squeeze characters in set1 only (no translation).
fn tr_squeeze_only<SI, SO, SE, FS>(
    env: &Environment<SI, SO, SE, FS>,
    set1: &CharSet,
) -> Result<ExitCode, Error>
where
    SI: Stdin,
    SO: Stdout,
    SE: Stderr,
    FS: Filesystem,
{
    let squeeze_set = set1.to_set();

    while let Some(line) = env.stdin.read_line()? {
        let mut output = String::new();
        let mut last_ch: Option<char> = None;

        for ch in line.chars() {
            if last_ch == Some(ch) && squeeze_set.contains(&ch) {
                continue;
            }
            output.push(ch);
            last_ch = Some(ch);
        }
        env.stdout.write_line(&output)?;
    }

    Ok(ExitCode::from(0))
}

/// Translate characters from set1 to set2, optionally squeezing.
fn tr_translate<SI, SO, SE, FS>(
    env: &Environment<SI, SO, SE, FS>,
    set1: &CharSet,
    set2: &CharSet,
    squeeze: bool,
) -> Result<ExitCode, Error>
where
    SI: Stdin,
    SO: Stdout,
    SE: Stderr,
    FS: Filesystem,
{
    let mut map = std::collections::HashMap::new();
    let set2_chars = &set2.chars;
    let last_set2_char = set2_chars.last().copied().unwrap_or('\0');

    for (i, &c1) in set1.chars.iter().enumerate() {
        let c2 = set2_chars.get(i).copied().unwrap_or(last_set2_char);
        map.insert(c1, c2);
    }

    let squeeze_set: HashSet<char> = if squeeze {
        set2.to_set()
    } else {
        HashSet::new()
    };

    while let Some(line) = env.stdin.read_line()? {
        let mut output = String::new();
        let mut last_ch: Option<char> = None;

        for ch in line.chars() {
            let out_ch = map.get(&ch).copied().unwrap_or(ch);
            if squeeze && last_ch == Some(out_ch) && squeeze_set.contains(&out_ch) {
                continue;
            }
            output.push(out_ch);
            last_ch = Some(out_ch);
        }
        env.stdout.write_line(&output)?;
    }

    Ok(ExitCode::from(0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::make_test_env_with_stdin;

    // ========================================================================
    // Basic translation tests
    // ========================================================================

    #[test]
    fn translate_single_chars() {
        let env = make_test_env_with_stdin(vec!["tr", "abc", "xyz"], "aabbcc");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("xxyyzz\n", env.stdout.into_string());
    }

    #[test]
    fn translate_range() {
        let env = make_test_env_with_stdin(vec!["tr", "a-z", "A-Z"], "hello");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("HELLO\n", env.stdout.into_string());
    }

    #[test]
    fn translate_mixed() {
        let env = make_test_env_with_stdin(vec!["tr", "aeiou", "12345"], "hello world");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("h2ll4 w4rld\n", env.stdout.into_string());
    }

    // ========================================================================
    // FreeBSD regression tests
    // ========================================================================

    #[test]
    fn regress_00_translate_abcde_12345() {
        let input = "quick brown\nfox jumped\nover the lazy\ndog";
        let env = make_test_env_with_stdin(vec!["tr", "abcde", "12345"], input);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let output = env.stdout.into_string();
        println!("output: {:?}", output);
        assert!(output.contains("qui3k"));
        assert!(output.contains("2rown"));
    }

    #[test]
    fn regress_01_translate_12345_abcde() {
        let input = "qui3k 2rown\nfox jump54\nov5r th5 l1zy\n4og";
        let env = make_test_env_with_stdin(vec!["tr", "12345", "abcde"], input);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let output = env.stdout.into_string();
        println!("output: {:?}", output);
        assert!(output.contains("quick"));
        assert!(output.contains("brown"));
    }

    #[test]
    fn regress_02_delete_aceg() {
        let input = "quick brown\nfox jumped\nover the lazy\ndog";
        let env = make_test_env_with_stdin(vec!["tr", "-d", "aceg"], input);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let output = env.stdout.into_string();
        println!("output: {:?}", output);
        assert!(output.contains("quik"));
        assert!(output.contains("jumpd"));
        assert!(!output.contains("a"));
        assert!(!output.contains("c"));
        assert!(!output.contains("e"));
        assert!(!output.contains("g"));
    }

    #[test]
    fn regress_03_lower_to_upper() {
        let input = "quick brown\nfox jumped\nover the lazy\ndog";
        let env = make_test_env_with_stdin(vec!["tr", "[:lower:]", "[:upper:]"], input);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let output = env.stdout.into_string();
        println!("output: {:?}", output);
        assert!(output.contains("QUICK BROWN"));
        assert!(output.contains("FOX JUMPED"));
        assert!(output.contains("DOG"));
    }

    #[test]
    fn regress_04_alpha_to_dot() {
        let input = "quick brown\nfox jumped\nover the lazy\ndog";
        let env = make_test_env_with_stdin(vec!["tr", "[:alpha:]", "."], input);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let output = env.stdout.into_string();
        println!("output: {:?}", output);
        assert!(output.contains("..... ....."));
        assert!(output.contains("... ......"));
    }

    #[test]
    fn regress_06_digit_to_question() {
        let input = "100 bottles of beer";
        let env = make_test_env_with_stdin(vec!["tr", "[:digit:]", "?"], input);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let output = env.stdout.into_string();
        println!("output: {:?}", output);
        assert!(output.contains("??? bottles of beer"));
    }

    #[test]
    fn regress_07_alnum_to_hash() {
        let input = "100 bottles of beer";
        let env = make_test_env_with_stdin(vec!["tr", "[:alnum:]", "#"], input);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let output = env.stdout.into_string();
        println!("output: {:?}", output);
        assert!(output.contains("### ####### ## ####"));
    }

    // ========================================================================
    // Delete mode tests (-d)
    // ========================================================================

    #[test]
    fn delete_single_char() {
        let env = make_test_env_with_stdin(vec!["tr", "-d", "a"], "abracadabra");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("brcdbr\n", env.stdout.into_string());
    }

    #[test]
    fn delete_multiple_chars() {
        let env = make_test_env_with_stdin(vec!["tr", "-d", "aeiou"], "hello world");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("hll wrld\n", env.stdout.into_string());
    }

    #[test]
    fn delete_range() {
        let env = make_test_env_with_stdin(vec!["tr", "-d", "a-z"], "Hello World 123");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("H W 123\n", env.stdout.into_string());
    }

    #[test]
    fn delete_complement() {
        let env = make_test_env_with_stdin(vec!["tr", "-cd", "a-z"], "Hello World 123");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("elloorld\n", env.stdout.into_string());
    }

    // ========================================================================
    // Squeeze mode tests (-s)
    // ========================================================================

    #[test]
    fn squeeze_only() {
        let env = make_test_env_with_stdin(vec!["tr", "-s", "o"], "foooobar");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("fobar\n", env.stdout.into_string());
    }

    #[test]
    fn squeeze_multiple() {
        let env = make_test_env_with_stdin(vec!["tr", "-s", "ab"], "aaabbbccc");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("abccc\n", env.stdout.into_string());
    }

    #[test]
    fn squeeze_with_translate() {
        let env = make_test_env_with_stdin(vec!["tr", "-s", "a-z", "A-Z"], "hello   world");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("HELO   WORLD\n", env.stdout.into_string());
    }

    // ========================================================================
    // Delete and squeeze combined (-ds)
    // ========================================================================

    #[test]
    fn delete_and_squeeze() {
        let env = make_test_env_with_stdin(vec!["tr", "-ds", "aeiou", " "], "hello   world");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("hll wrld\n", env.stdout.into_string());
    }

    // ========================================================================
    // Escape sequence tests
    // ========================================================================

    #[test]
    fn escape_newline() {
        let env = make_test_env_with_stdin(vec!["tr", "\\n", "X"], "hello");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("hello\n", env.stdout.into_string());
    }

    #[test]
    fn escape_tab() {
        let env = make_test_env_with_stdin(vec!["tr", "\\t", " "], "hello\tworld");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("hello world\n", env.stdout.into_string());
    }

    #[test]
    fn escape_octal() {
        let env = make_test_env_with_stdin(vec!["tr", "\\141", "X"], "abc");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("Xbc\n", env.stdout.into_string());
    }

    // ========================================================================
    // Character class tests
    // ========================================================================

    #[test]
    fn class_digit() {
        let env = make_test_env_with_stdin(vec!["tr", "-d", "[:digit:]"], "abc123def456");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("abcdef\n", env.stdout.into_string());
    }

    #[test]
    fn class_lower() {
        let env = make_test_env_with_stdin(vec!["tr", "-d", "[:lower:]"], "Hello World");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("H W\n", env.stdout.into_string());
    }

    #[test]
    fn class_upper() {
        let env = make_test_env_with_stdin(vec!["tr", "-d", "[:upper:]"], "Hello World");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("ello orld\n", env.stdout.into_string());
    }

    #[test]
    fn class_space() {
        let env = make_test_env_with_stdin(vec!["tr", "-d", "[:space:]"], "hello world\tfoo");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("helloworldfoo\n", env.stdout.into_string());
    }

    #[test]
    fn class_xdigit() {
        let env = make_test_env_with_stdin(vec!["tr", "-cd", "[:xdigit:]"], "0xDEADBEEF!");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("0DEADBEEF\n", env.stdout.into_string());
    }

    // ========================================================================
    // Error handling tests
    // ========================================================================

    #[test]
    fn error_no_operand() {
        let env = make_test_env_with_stdin(vec!["tr"], "");
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("missing operand"));
    }

    #[test]
    fn error_single_operand_without_flags() {
        let env = make_test_env_with_stdin(vec!["tr", "abc"], "");
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("missing operand"));
    }

    #[test]
    fn error_unknown_class() {
        let env = make_test_env_with_stdin(vec!["tr", "[:bogus:]", "x"], "hello");
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("unknown class"));
    }

    // ========================================================================
    // Multi-line tests
    // ========================================================================

    #[test]
    fn multi_line_translate() {
        let env = make_test_env_with_stdin(vec!["tr", "a-z", "A-Z"], "hello\nworld\nfoo");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("HELLO\nWORLD\nFOO\n", env.stdout.into_string());
    }

    #[test]
    fn multi_line_delete() {
        let env = make_test_env_with_stdin(vec!["tr", "-d", "aeiou"], "hello\nworld");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("hll\nwrld\n", env.stdout.into_string());
    }

    // ========================================================================
    // Edge cases
    // ========================================================================

    #[test]
    fn empty_input() {
        let env = make_test_env_with_stdin(vec!["tr", "a", "b"], "");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("", env.stdout.into_string());
    }

    #[test]
    fn no_matching_chars() {
        let env = make_test_env_with_stdin(vec!["tr", "xyz", "123"], "hello");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("hello\n", env.stdout.into_string());
    }

    #[test]
    fn string2_shorter_than_string1() {
        let env = make_test_env_with_stdin(vec!["tr", "abc", "x"], "aabbcc");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("xxxxxx\n", env.stdout.into_string());
    }

    #[test]
    fn string2_longer_than_string1() {
        let env = make_test_env_with_stdin(vec!["tr", "ab", "xyz"], "aabbcc");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("xxyycc\n", env.stdout.into_string());
    }
}

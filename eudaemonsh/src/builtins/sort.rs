//! The sort builtin: sort lines of text files.

use std::cmp::Ordering;

use getopts::Options;

use crate::{Environment, Error, ExitCode, Filesystem, FsError, StdioIn, StdioOut};

/// Modifiers that can be applied to a sort key or globally.
#[derive(Clone, Copy, Debug, Default)]
struct KeyModifiers {
    /// Ignore leading blanks.
    ignore_leading_blanks: bool,
    /// Dictionary order: only blanks and alphanumeric characters.
    dictionary_order: bool,
    /// Case-insensitive comparison.
    ignore_case: bool,
    /// Ignore non-printable characters.
    ignore_nonprinting: bool,
    /// Numeric sort.
    numeric: bool,
    /// General numeric sort (floating point).
    general_numeric: bool,
    /// Human-readable numeric sort (e.g., 1K, 2M).
    human_numeric: bool,
    /// Month sort.
    month: bool,
    /// Reverse sort order.
    reverse: bool,
    /// Version sort.
    version: bool,
}

/// A sort key specification from -k option.
#[derive(Clone, Debug)]
struct KeySpec {
    /// Starting field (1-indexed).
    start_field: usize,
    /// Starting character within field (1-indexed, 0 means entire field).
    start_char: usize,
    /// Ending field (1-indexed, 0 means end of line).
    end_field: usize,
    /// Ending character within field (1-indexed, 0 means end of field).
    end_char: usize,
    /// Modifiers for this key.
    modifiers: KeyModifiers,
}

impl Default for KeySpec {
    fn default() -> Self {
        Self {
            start_field: 1,
            start_char: 1,
            end_field: 0,
            end_char: 0,
            modifiers: KeyModifiers::default(),
        }
    }
}

/// Options for the sort command.
#[derive(Clone, Debug, Default)]
struct SortOptions {
    /// Global modifiers applied when no key-specific modifiers exist.
    global_modifiers: KeyModifiers,
    /// Sort key specifications.
    keys: Vec<KeySpec>,
    /// Field separator character.
    field_separator: Option<char>,
    /// Output file (None means stdout).
    output_file: Option<String>,
    /// Check if input is sorted.
    check: bool,
    /// Silent check (no output on disorder).
    check_silent: bool,
    /// Merge only (assume inputs are pre-sorted).
    #[allow(dead_code)]
    merge: bool,
    /// Unique: suppress duplicate keys.
    unique: bool,
    /// Stable sort.
    stable: bool,
    /// Use NUL as line terminator.
    zero_terminated: bool,
}

fn build_options() -> Options {
    let mut opts = Options::new();
    // Ordering options
    opts.optflag("b", "ignore-leading-blanks", "Ignore leading blanks.");
    opts.optflag(
        "d",
        "dictionary-order",
        "Consider only blanks and alphanumeric characters.",
    );
    opts.optflag("f", "ignore-case", "Fold lower case to upper case.");
    opts.optflag(
        "g",
        "general-numeric-sort",
        "Compare according to general numerical value.",
    );
    opts.optflag(
        "h",
        "human-numeric-sort",
        "Compare human readable numbers (e.g., 2K 1G).",
    );
    opts.optflag("i", "ignore-nonprinting", "Consider only printable chars.");
    opts.optflag(
        "M",
        "month-sort",
        "Compare (unknown) < 'JAN' < ... < 'DEC'.",
    );
    opts.optflag(
        "n",
        "numeric-sort",
        "Compare according to string numerical value.",
    );
    opts.optflag("R", "random-sort", "Shuffle, but group identical keys.");
    opts.optflag("r", "reverse", "Reverse the result of comparisons.");
    opts.optflag(
        "V",
        "version-sort",
        "Natural sort of (version) numbers within text.",
    );

    // Other options
    opts.optflag("c", "check", "Check for sorted input; do not sort.");
    opts.optflag(
        "C",
        "check=silent",
        "Like -c, but do not report first bad line.",
    );
    opts.optmulti(
        "k",
        "key",
        "Sort via a key; KEYDEF gives location and type.",
        "KEYDEF",
    );
    opts.optflag("m", "merge", "Merge already sorted files; do not sort.");
    opts.optopt(
        "o",
        "output",
        "Write result to FILE instead of stdout.",
        "FILE",
    );
    opts.optflag(
        "s",
        "stable",
        "Stabilize sort by disabling last-resort comparison.",
    );
    opts.optopt(
        "t",
        "field-separator",
        "Use SEP instead of non-blank to blank transition.",
        "SEP",
    );
    opts.optflag(
        "u",
        "unique",
        "With -c, check for strict ordering; without -c, output only the first of an equal run.",
    );
    opts.optflag(
        "z",
        "zero-terminated",
        "Line delimiter is NUL, not newline.",
    );
    opts.optflag("", "help", "Display this help and exit.");
    opts.optflag("", "version", "Output version information and exit.");
    opts
}

/// Parse key modifiers from a string suffix (e.g., "2.3n" -> modifiers from "n").
fn parse_modifiers(s: &str) -> KeyModifiers {
    let mut mods = KeyModifiers::default();
    for c in s.chars() {
        match c {
            'b' => mods.ignore_leading_blanks = true,
            'd' => mods.dictionary_order = true,
            'f' => mods.ignore_case = true,
            'g' => mods.general_numeric = true,
            'h' => mods.human_numeric = true,
            'i' => mods.ignore_nonprinting = true,
            'M' => mods.month = true,
            'n' => mods.numeric = true,
            'r' => mods.reverse = true,
            'V' => mods.version = true,
            _ => {}
        }
    }
    mods
}

/// Parse a field.char specification like "2" or "2.3" or "2.3n".
fn parse_field_spec(s: &str) -> (usize, usize, KeyModifiers) {
    let mut field = 0usize;
    let mut char_pos = 0usize;
    let mut modifier_start = 0;

    // Find where digits end for field number
    let mut chars = s.char_indices().peekable();
    while let Some(&(i, c)) = chars.peek() {
        if c.is_ascii_digit() {
            field = field * 10 + (c as usize - '0' as usize);
            modifier_start = i + 1;
            chars.next();
        } else {
            break;
        }
    }

    // Check for .char specification
    if let Some(&(_, '.')) = chars.peek() {
        chars.next();
        while let Some(&(i, c)) = chars.peek() {
            if c.is_ascii_digit() {
                char_pos = char_pos * 10 + (c as usize - '0' as usize);
                modifier_start = i + 1;
                chars.next();
            } else {
                break;
            }
        }
    }

    let modifiers = parse_modifiers(&s[modifier_start..]);
    (field, char_pos, modifiers)
}

/// Parse a -k key specification like "1", "1,2", "1.2,3.4", "2n", "1,2nr".
fn parse_key_spec(spec: &str, global_mods: &KeyModifiers) -> Result<KeySpec, String> {
    let parts: Vec<&str> = spec.splitn(2, ',').collect();

    let (start_field, start_char, start_mods) = parse_field_spec(parts[0]);

    let (end_field, end_char, end_mods) = if parts.len() > 1 {
        parse_field_spec(parts[1])
    } else {
        (0, 0, KeyModifiers::default())
    };

    if start_field == 0 {
        return Err(format!("invalid key specification: {}", spec));
    }

    // Merge modifiers: key-specific modifiers override global
    let mut modifiers = *global_mods;
    let has_start_mods = has_any_modifier(&start_mods);
    let has_end_mods = has_any_modifier(&end_mods);

    if has_start_mods || has_end_mods {
        modifiers = merge_modifiers(&start_mods, &end_mods);
    }

    Ok(KeySpec {
        start_field,
        start_char: if start_char == 0 { 1 } else { start_char },
        end_field,
        end_char,
        modifiers,
    })
}

fn has_any_modifier(mods: &KeyModifiers) -> bool {
    mods.ignore_leading_blanks
        || mods.dictionary_order
        || mods.ignore_case
        || mods.ignore_nonprinting
        || mods.numeric
        || mods.general_numeric
        || mods.human_numeric
        || mods.month
        || mods.reverse
        || mods.version
}

fn merge_modifiers(a: &KeyModifiers, b: &KeyModifiers) -> KeyModifiers {
    KeyModifiers {
        ignore_leading_blanks: a.ignore_leading_blanks || b.ignore_leading_blanks,
        dictionary_order: a.dictionary_order || b.dictionary_order,
        ignore_case: a.ignore_case || b.ignore_case,
        ignore_nonprinting: a.ignore_nonprinting || b.ignore_nonprinting,
        numeric: a.numeric || b.numeric,
        general_numeric: a.general_numeric || b.general_numeric,
        human_numeric: a.human_numeric || b.human_numeric,
        month: a.month || b.month,
        reverse: a.reverse || b.reverse,
        version: a.version || b.version,
    }
}

/// The sort builtin: sort lines of text files.
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
            env.stderr.write_line(&format!("sort: {}", e))?;
            return Ok(ExitCode::from(2));
        }
    };

    if matches.opt_present("help") {
        env.stdout.write_line("Usage: sort [OPTION]... [FILE]...")?;
        env.stdout
            .write_line("Write sorted concatenation of all FILE(s) to standard output.")?;
        return Ok(ExitCode::from(0));
    }

    if matches.opt_present("version") {
        env.stdout.write_line("sort (eudaemonsh) 1.0")?;
        return Ok(ExitCode::from(0));
    }

    // Build global modifiers from command-line flags
    let global_modifiers = KeyModifiers {
        ignore_leading_blanks: matches.opt_present("b"),
        dictionary_order: matches.opt_present("d"),
        ignore_case: matches.opt_present("f"),
        ignore_nonprinting: matches.opt_present("i"),
        numeric: matches.opt_present("n"),
        general_numeric: matches.opt_present("g"),
        human_numeric: matches.opt_present("h"),
        month: matches.opt_present("M"),
        reverse: matches.opt_present("r"),
        version: matches.opt_present("V"),
    };

    // Parse key specifications
    let mut keys = Vec::new();
    for key_str in matches.opt_strs("k") {
        match parse_key_spec(&key_str, &global_modifiers) {
            Ok(key) => keys.push(key),
            Err(e) => {
                env.stderr.write_line(&format!("sort: {}", e))?;
                return Ok(ExitCode::from(2));
            }
        }
    }

    // Parse field separator
    let field_separator = if let Some(sep) = matches.opt_str("t") {
        if sep.is_empty() {
            env.stderr.write_line("sort: empty field separator")?;
            return Ok(ExitCode::from(2));
        }
        Some(sep.chars().next().unwrap())
    } else {
        None
    };

    let opts = SortOptions {
        global_modifiers,
        keys,
        field_separator,
        output_file: matches.opt_str("o"),
        check: matches.opt_present("c"),
        check_silent: matches.opt_present("C"),
        merge: matches.opt_present("m"),
        unique: matches.opt_present("u"),
        stable: matches.opt_present("s"),
        zero_terminated: matches.opt_present("z"),
    };

    // Read input
    let input = read_input(env, &matches.free, opts.zero_terminated)?;

    // Check mode
    if opts.check || opts.check_silent {
        return check_sorted(env, &input, &opts);
    }

    // Sort the lines
    let output = sort_lines(&input, &opts);

    // Write output
    if let Some(ref output_file) = opts.output_file {
        if let Err(e) = env.fs.write_string(output_file, &output) {
            env.stderr
                .write_line(&format!("sort: {}: {:?}", output_file, e))?;
            return Ok(ExitCode::from(2));
        }
    } else {
        env.stdout.write_str(&output)?;
    }

    Ok(ExitCode::from(0))
}

/// Read input from files or stdin.
fn read_input<SI, SO, SE, FS>(
    env: &Environment<SI, SO, SE, FS>,
    files: &[String],
    zero_terminated: bool,
) -> Result<String, Error>
where
    SI: StdioIn,
    SO: StdioOut,
    SE: StdioOut,
    FS: Filesystem,
{
    let mut contents = String::new();

    if files.is_empty() || (files.len() == 1 && files[0] == "-") {
        // Read from stdin
        while let Some(line) = env.stdin.read_line()? {
            contents.push_str(&line);
            if zero_terminated {
                contents.push('\0');
            } else {
                contents.push('\n');
            }
        }
    } else {
        for file in files {
            if file == "-" {
                while let Some(line) = env.stdin.read_line()? {
                    contents.push_str(&line);
                    if zero_terminated {
                        contents.push('\0');
                    } else {
                        contents.push('\n');
                    }
                }
            } else {
                match env.fs.read_to_string(file) {
                    Ok(file_contents) => {
                        contents.push_str(&file_contents);
                        if !file_contents.ends_with('\n') && !zero_terminated {
                            contents.push('\n');
                        }
                    }
                    Err(FsError::Io(e)) => {
                        env.stderr.write_line(&format!("sort: {}: {}", file, e))?;
                    }
                }
            }
        }
    }

    Ok(contents)
}

/// Split a line into fields based on separator.
fn split_fields(line: &str, separator: Option<char>) -> Vec<&str> {
    match separator {
        Some(sep) => line.split(sep).collect(),
        None => {
            // Default: fields are separated by runs of blanks
            let mut fields = Vec::new();
            let mut in_field = false;
            let mut start = 0;
            for (i, c) in line.char_indices() {
                if c.is_whitespace() {
                    if in_field {
                        fields.push(&line[start..i]);
                        in_field = false;
                    }
                } else if !in_field {
                    start = i;
                    in_field = true;
                }
            }
            if in_field {
                fields.push(&line[start..]);
            }
            if fields.is_empty() {
                fields.push(line);
            }
            fields
        }
    }
}

/// Extract the sort key from a line based on key specification.
fn extract_key(line: &str, key: &KeySpec, separator: Option<char>) -> String {
    let fields = split_fields(line, separator);

    if key.start_field > fields.len() {
        return String::new();
    }

    let start_field_idx = key.start_field - 1;
    let end_field_idx = if key.end_field == 0 {
        fields.len() - 1
    } else {
        (key.end_field - 1).min(fields.len() - 1)
    };

    if start_field_idx > end_field_idx {
        return String::new();
    }

    let mut result = String::new();

    for (i, field_idx) in (start_field_idx..=end_field_idx).enumerate() {
        let field = fields[field_idx];

        let start_char = if i == 0 && key.start_char > 1 {
            (key.start_char - 1).min(field.len())
        } else {
            0
        };

        let end_char = if field_idx == end_field_idx && key.end_char > 0 {
            key.end_char.min(field.len())
        } else {
            field.len()
        };

        if start_char < end_char {
            if !result.is_empty() && separator.is_none() {
                result.push(' ');
            } else if !result.is_empty() {
                result.push(separator.unwrap_or(' '));
            }
            result.push_str(&field[start_char..end_char]);
        }
    }

    result
}

/// Apply modifiers to a key for comparison.
fn apply_modifiers(key: &str, mods: &KeyModifiers) -> String {
    let mut result = key.to_string();

    if mods.ignore_leading_blanks {
        result = result.trim_start().to_string();
    }

    if mods.dictionary_order {
        result = result
            .chars()
            .filter(|c| c.is_alphanumeric() || c.is_whitespace())
            .collect();
    }

    if mods.ignore_nonprinting {
        result = result.chars().filter(|c| !c.is_control()).collect();
    }

    if mods.ignore_case {
        result = result.to_uppercase();
    }

    result
}

/// Parse a numeric value from a string.
fn parse_numeric(s: &str) -> f64 {
    let s = s.trim();
    s.parse::<f64>().unwrap_or(0.0)
}

/// Parse a human-readable numeric value (e.g., "1K", "2M", "3G").
fn parse_human_numeric(s: &str) -> f64 {
    let s = s.trim();
    if s.is_empty() {
        return 0.0;
    }

    let (num_str, suffix) = if let Some(pos) = s.find(|c: char| c.is_alphabetic()) {
        (&s[..pos], &s[pos..])
    } else {
        (s, "")
    };

    let base: f64 = num_str.parse().unwrap_or(0.0);

    let multiplier = match suffix.chars().next() {
        Some('k') | Some('K') => 1024.0,
        Some('M') => 1024.0 * 1024.0,
        Some('G') => 1024.0 * 1024.0 * 1024.0,
        Some('T') => 1024.0 * 1024.0 * 1024.0 * 1024.0,
        Some('P') => 1024.0 * 1024.0 * 1024.0 * 1024.0 * 1024.0,
        Some('E') => 1024.0 * 1024.0 * 1024.0 * 1024.0 * 1024.0 * 1024.0,
        _ => 1.0,
    };

    base * multiplier
}

/// Parse month name and return ordering value (1-12, 0 for unknown).
fn parse_month(s: &str) -> u8 {
    let s = s.trim().to_uppercase();
    let prefix = if s.len() >= 3 { &s[..3] } else { &s };

    match prefix {
        "JAN" => 1,
        "FEB" => 2,
        "MAR" => 3,
        "APR" => 4,
        "MAY" => 5,
        "JUN" => 6,
        "JUL" => 7,
        "AUG" => 8,
        "SEP" => 9,
        "OCT" => 10,
        "NOV" => 11,
        "DEC" => 12,
        _ => 0,
    }
}

/// Compare two version strings.
fn compare_version(a: &str, b: &str) -> Ordering {
    let mut a_chars = a.chars().peekable();
    let mut b_chars = b.chars().peekable();

    loop {
        let a_is_digit = a_chars.peek().is_some_and(|c| c.is_ascii_digit());
        let b_is_digit = b_chars.peek().is_some_and(|c| c.is_ascii_digit());

        if a_is_digit && b_is_digit {
            // Compare numeric parts
            let mut a_num = String::new();
            let mut b_num = String::new();

            while let Some(&c) = a_chars.peek() {
                if c.is_ascii_digit() {
                    a_num.push(c);
                    a_chars.next();
                } else {
                    break;
                }
            }

            while let Some(&c) = b_chars.peek() {
                if c.is_ascii_digit() {
                    b_num.push(c);
                    b_chars.next();
                } else {
                    break;
                }
            }

            // Compare numerically (ignoring leading zeros)
            let a_val: u64 = a_num.parse().unwrap_or(0);
            let b_val: u64 = b_num.parse().unwrap_or(0);

            match a_val.cmp(&b_val) {
                Ordering::Equal => {
                    // When numeric values are equal, more leading zeros comes first.
                    // Longer string means more leading zeros for the same numeric value.
                    match b_num.len().cmp(&a_num.len()) {
                        Ordering::Equal => continue,
                        other => return other,
                    }
                }
                other => return other,
            }
        } else {
            // Compare non-numeric parts character by character
            match (a_chars.next(), b_chars.next()) {
                (None, None) => return Ordering::Equal,
                (None, Some(_)) => return Ordering::Less,
                (Some(_), None) => return Ordering::Greater,
                (Some(a_c), Some(b_c)) => {
                    if a_c != b_c {
                        return a_c.cmp(&b_c);
                    }
                }
            }
        }
    }
}

/// Compare two keys with modifiers.
fn compare_keys(a: &str, b: &str, mods: &KeyModifiers) -> Ordering {
    if mods.numeric {
        let a_num = parse_numeric(a);
        let b_num = parse_numeric(b);
        return a_num.partial_cmp(&b_num).unwrap_or(Ordering::Equal);
    }

    if mods.general_numeric {
        let a_num = parse_numeric(a);
        let b_num = parse_numeric(b);
        return a_num.partial_cmp(&b_num).unwrap_or(Ordering::Equal);
    }

    if mods.human_numeric {
        let a_num = parse_human_numeric(a);
        let b_num = parse_human_numeric(b);
        return a_num.partial_cmp(&b_num).unwrap_or(Ordering::Equal);
    }

    if mods.month {
        let a_month = parse_month(a);
        let b_month = parse_month(b);
        return a_month.cmp(&b_month);
    }

    if mods.version {
        return compare_version(a, b);
    }

    // Default: lexicographic comparison
    a.cmp(b)
}

/// Compare two lines using all key specifications.
fn compare_lines(a: &str, b: &str, opts: &SortOptions) -> Ordering {
    compare_lines_impl(a, b, opts, false)
}

/// Compare two lines for uniqueness (keys only, no tiebreaker).
fn compare_lines_for_unique(a: &str, b: &str, opts: &SortOptions) -> Ordering {
    compare_lines_impl(a, b, opts, true)
}

/// Implementation of line comparison.
fn compare_lines_impl(a: &str, b: &str, opts: &SortOptions, keys_only: bool) -> Ordering {
    if opts.keys.is_empty() {
        // No keys specified: use entire line
        let a_key = apply_modifiers(a, &opts.global_modifiers);
        let b_key = apply_modifiers(b, &opts.global_modifiers);

        let cmp = compare_keys(&a_key, &b_key, &opts.global_modifiers);
        return if opts.global_modifiers.reverse {
            cmp.reverse()
        } else {
            cmp
        };
    }

    // Compare using each key in order
    for key in &opts.keys {
        let a_key = extract_key(a, key, opts.field_separator);
        let b_key = extract_key(b, key, opts.field_separator);

        let a_key = apply_modifiers(&a_key, &key.modifiers);
        let b_key = apply_modifiers(&b_key, &key.modifiers);

        let cmp = compare_keys(&a_key, &b_key, &key.modifiers);

        if cmp != Ordering::Equal {
            return if key.modifiers.reverse {
                cmp.reverse()
            } else {
                cmp
            };
        }
    }

    // All keys equal: use full line as tiebreaker (unless stable sort or keys_only mode)
    if opts.stable || keys_only {
        Ordering::Equal
    } else {
        a.cmp(b)
    }
}

/// Sort the lines.
fn sort_lines(input: &str, opts: &SortOptions) -> String {
    let terminator = if opts.zero_terminated { '\0' } else { '\n' };

    let mut lines: Vec<&str> = input.split(terminator).collect();

    // Remove empty last element if input ended with terminator
    if lines.last().is_some_and(|s| s.is_empty()) {
        lines.pop();
    }

    // Sort with stable sort if requested, otherwise use default sort
    if opts.stable || opts.unique {
        lines.sort_by(|a, b| compare_lines(a, b, opts));
    } else {
        lines.sort_by(|a, b| compare_lines(a, b, opts));
    }

    // Handle unique flag
    if opts.unique {
        lines.dedup_by(|a, b| compare_lines_for_unique(a, b, opts) == Ordering::Equal);
    }

    // Build output
    let mut output = String::new();
    for line in lines {
        output.push_str(line);
        output.push(terminator);
    }

    output
}

/// Check if input is sorted.
fn check_sorted<SI, SO, SE, FS>(
    env: &Environment<SI, SO, SE, FS>,
    input: &str,
    opts: &SortOptions,
) -> Result<ExitCode, Error>
where
    SI: StdioIn,
    SO: StdioOut,
    SE: StdioOut,
    FS: Filesystem,
{
    let terminator = if opts.zero_terminated { '\0' } else { '\n' };
    let lines: Vec<&str> = input.split(terminator).filter(|s| !s.is_empty()).collect();

    let mut prev_line: Option<&str> = None;

    for (i, line) in lines.iter().enumerate() {
        if let Some(prev) = prev_line {
            let cmp = compare_lines(prev, line, opts);
            let is_disorder = if opts.unique {
                cmp != Ordering::Less
            } else {
                cmp == Ordering::Greater
            };

            if is_disorder {
                if !opts.check_silent {
                    env.stderr.write_line(&format!(
                        "sort: -:{}:{}: disorder: {}",
                        i + 1,
                        1,
                        line
                    ))?;
                }
                return Ok(ExitCode::from(1));
            }
        }
        prev_line = Some(line);
    }

    Ok(ExitCode::from(0))
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
    fn basic_sort() {
        let env = make_test_env_with_stdin(vec!["sort"], "banana\napple\ncherry\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert_eq!("apple\nbanana\ncherry\n", stdout);
    }

    #[test]
    fn empty_input() {
        let env = make_test_env_with_stdin(vec!["sort"], "");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("", env.stdout.into_string());
    }

    #[test]
    fn single_line() {
        let env = make_test_env_with_stdin(vec!["sort"], "hello\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("hello\n", env.stdout.into_string());
    }

    #[test]
    fn already_sorted() {
        let env = make_test_env_with_stdin(vec!["sort"], "a\nb\nc\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("a\nb\nc\n", env.stdout.into_string());
    }

    #[test]
    fn reverse_sorted() {
        let env = make_test_env_with_stdin(vec!["sort"], "c\nb\na\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("a\nb\nc\n", env.stdout.into_string());
    }

    #[test]
    fn duplicate_lines() {
        let env = make_test_env_with_stdin(vec!["sort"], "b\na\nb\na\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("a\na\nb\nb\n", env.stdout.into_string());
    }

    // ========================================================================
    // -r flag: reverse sort
    // ========================================================================

    #[test]
    fn reverse() {
        let env = make_test_env_with_stdin(vec!["sort", "-r"], "apple\nbanana\ncherry\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert_eq!("cherry\nbanana\napple\n", stdout);
    }

    #[test]
    fn reverse_long_form() {
        let env = make_test_env_with_stdin(vec!["sort", "--reverse"], "a\nb\nc\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("c\nb\na\n", env.stdout.into_string());
    }

    // ========================================================================
    // -n flag: numeric sort
    // ========================================================================

    #[test]
    fn numeric() {
        let env = make_test_env_with_stdin(vec!["sort", "-n"], "10\n2\n1\n20\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert_eq!("1\n2\n10\n20\n", stdout);
    }

    #[test]
    fn numeric_long_form() {
        let env = make_test_env_with_stdin(vec!["sort", "--numeric-sort"], "10\n2\n1\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("1\n2\n10\n", env.stdout.into_string());
    }

    #[test]
    fn numeric_negative() {
        let env = make_test_env_with_stdin(vec!["sort", "-n"], "5\n-3\n0\n-10\n10\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert_eq!("-10\n-3\n0\n5\n10\n", stdout);
    }

    #[test]
    fn numeric_with_text() {
        let env = make_test_env_with_stdin(vec!["sort", "-n"], "10\nabc\n2\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        // Non-numeric strings are treated as 0
        assert_eq!("abc\n2\n10\n", stdout);
    }

    // ========================================================================
    // -f flag: ignore case
    // ========================================================================

    #[test]
    fn ignore_case() {
        let env = make_test_env_with_stdin(vec!["sort", "-f"], "Banana\napple\nCherry\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert_eq!("apple\nBanana\nCherry\n", stdout);
    }

    #[test]
    fn ignore_case_long_form() {
        let env = make_test_env_with_stdin(vec!["sort", "--ignore-case"], "B\na\nC\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("a\nB\nC\n", env.stdout.into_string());
    }

    // ========================================================================
    // -b flag: ignore leading blanks
    // ========================================================================

    #[test]
    fn ignore_leading_blanks() {
        let env = make_test_env_with_stdin(vec!["sort", "-b"], "  b\na\n   c\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert_eq!("a\n  b\n   c\n", stdout);
    }

    // ========================================================================
    // -u flag: unique
    // ========================================================================

    #[test]
    fn unique() {
        let env = make_test_env_with_stdin(vec!["sort", "-u"], "b\na\nb\na\nc\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert_eq!("a\nb\nc\n", stdout);
    }

    #[test]
    fn unique_long_form() {
        let env = make_test_env_with_stdin(vec!["sort", "--unique"], "a\na\nb\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("a\nb\n", env.stdout.into_string());
    }

    // ========================================================================
    // -k flag: key specification
    // ========================================================================

    #[test]
    fn key_second_field() {
        let env = make_test_env_with_stdin(vec!["sort", "-k2"], "1 b\n2 a\n3 c\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert_eq!("2 a\n1 b\n3 c\n", stdout);
    }

    #[test]
    fn key_numeric_second_field() {
        let env = make_test_env_with_stdin(vec!["sort", "-k2n"], "a 10\nb 2\nc 1\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert_eq!("c 1\nb 2\na 10\n", stdout);
    }

    #[test]
    fn key_with_range() {
        let env = make_test_env_with_stdin(vec!["sort", "-k1,1"], "ab x\naa y\nac z\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        // Sorts by first field only, but tiebreaker uses full line
        assert_eq!("aa y\nab x\nac z\n", stdout);
    }

    // ========================================================================
    // -t flag: field separator
    // ========================================================================

    #[test]
    fn field_separator() {
        let env = make_test_env_with_stdin(vec!["sort", "-t:", "-k2"], "a:b\nc:a\ne:c\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert_eq!("c:a\na:b\ne:c\n", stdout);
    }

    #[test]
    fn field_separator_comma() {
        let env = make_test_env_with_stdin(vec!["sort", "-t,", "-k2n"], "a,10\nb,2\nc,1\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert_eq!("c,1\nb,2\na,10\n", stdout);
    }

    // ========================================================================
    // -c flag: check sorted
    // ========================================================================

    #[test]
    fn check_sorted_ok() {
        let env = make_test_env_with_stdin(vec!["sort", "-c"], "a\nb\nc\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
    }

    #[test]
    fn check_sorted_fail() {
        let env = make_test_env_with_stdin(vec!["sort", "-c"], "b\na\nc\n");
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("disorder"));
    }

    #[test]
    fn check_sorted_silent_fail() {
        let env = make_test_env_with_stdin(vec!["sort", "-C"], "b\na\nc\n");
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        assert_eq!("", env.stderr.into_string());
    }

    // ========================================================================
    // -h flag: human numeric sort
    // ========================================================================

    #[test]
    fn human_numeric() {
        let env = make_test_env_with_stdin(vec!["sort", "-h"], "1G\n1K\n1M\n1\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert_eq!("1\n1K\n1M\n1G\n", stdout);
    }

    // ========================================================================
    // -M flag: month sort
    // ========================================================================

    #[test]
    fn month_sort() {
        let env = make_test_env_with_stdin(vec!["sort", "-M"], "Mar\nJan\nFeb\nDec\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert_eq!("Jan\nFeb\nMar\nDec\n", stdout);
    }

    #[test]
    fn month_sort_case_insensitive() {
        let env = make_test_env_with_stdin(vec!["sort", "-M"], "mar\nJAN\nfeb\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert_eq!("JAN\nfeb\nmar\n", stdout);
    }

    // ========================================================================
    // -V flag: version sort
    // ========================================================================

    #[test]
    fn version_sort() {
        let env = make_test_env_with_stdin(vec!["sort", "-V"], "1.10\n1.2\n1.1\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert_eq!("1.1\n1.2\n1.10\n", stdout);
    }

    #[test]
    fn version_sort_with_prefix() {
        let env = make_test_env_with_stdin(vec!["sort", "-V"], "v2.0\nv1.10\nv1.2\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert_eq!("v1.2\nv1.10\nv2.0\n", stdout);
    }

    // ========================================================================
    // -o flag: output file
    // ========================================================================

    #[test]
    fn output_file() {
        let env = make_test_env_with_stdin(vec!["sort", "-o", "output.txt"], "c\na\nb\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("", env.stdout.into_string());
        assert_eq!("a\nb\nc\n", env.fs.read_to_string("output.txt").unwrap());
    }

    // ========================================================================
    // File input
    // ========================================================================

    #[test]
    fn read_from_file() {
        let env = make_test_env_with_stdin(vec!["sort", "input.txt"], "");
        env.fs.add_file("input.txt", "c\na\nb\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("a\nb\nc\n", env.stdout.into_string());
    }

    #[test]
    fn file_not_found() {
        let env = make_test_env_with_stdin(vec!["sort", "nonexistent.txt"], "");
        let result = bin(&env).unwrap();
        // sort continues on file errors, returning success but with error message
        assert_eq!(0, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("nonexistent.txt"));
    }

    #[test]
    fn multiple_files() {
        let env = make_test_env_with_stdin(vec!["sort", "a.txt", "b.txt"], "");
        env.fs.add_file("a.txt", "c\na\n");
        env.fs.add_file("b.txt", "d\nb\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert_eq!("a\nb\nc\nd\n", stdout);
    }

    // ========================================================================
    // Combined options
    // ========================================================================

    #[test]
    fn numeric_reverse() {
        let env = make_test_env_with_stdin(vec!["sort", "-nr"], "1\n10\n2\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert_eq!("10\n2\n1\n", stdout);
    }

    #[test]
    fn key_reverse() {
        let env = make_test_env_with_stdin(vec!["sort", "-k2r"], "a 1\nb 3\nc 2\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert_eq!("b 3\nc 2\na 1\n", stdout);
    }

    // ========================================================================
    // Helper function tests
    // ========================================================================

    #[test]
    fn split_fields_whitespace() {
        let fields = split_fields("  a   b   c  ", None);
        println!("fields: {:?}", fields);
        assert_eq!(vec!["a", "b", "c"], fields);
    }

    #[test]
    fn split_fields_separator() {
        let fields = split_fields("a:b:c", Some(':'));
        println!("fields: {:?}", fields);
        assert_eq!(vec!["a", "b", "c"], fields);
    }

    #[test]
    fn split_fields_empty_between() {
        let fields = split_fields("a::c", Some(':'));
        println!("fields: {:?}", fields);
        assert_eq!(vec!["a", "", "c"], fields);
    }

    #[test]
    fn parse_human_numeric_basic() {
        assert_eq!(1024.0, parse_human_numeric("1K"));
        assert_eq!(1024.0, parse_human_numeric("1k"));
        assert_eq!(1048576.0, parse_human_numeric("1M"));
        assert_eq!(1073741824.0, parse_human_numeric("1G"));
        assert_eq!(100.0, parse_human_numeric("100"));
    }

    #[test]
    fn parse_month_basic() {
        assert_eq!(1, parse_month("JAN"));
        assert_eq!(1, parse_month("jan"));
        assert_eq!(1, parse_month("January"));
        assert_eq!(12, parse_month("DEC"));
        assert_eq!(0, parse_month("invalid"));
    }

    #[test]
    fn compare_version_basic() {
        assert_eq!(Ordering::Less, compare_version("1.2", "1.10"));
        assert_eq!(Ordering::Equal, compare_version("1.2", "1.2"));
        assert_eq!(Ordering::Greater, compare_version("1.10", "1.2"));
        assert_eq!(Ordering::Less, compare_version("a1", "a2"));
    }

    // ========================================================================
    // Edge cases
    // ========================================================================

    #[test]
    fn no_trailing_newline() {
        let env = make_test_env_with_stdin(vec!["sort"], "b\na");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert_eq!("a\nb\n", stdout);
    }

    #[test]
    fn empty_lines() {
        let env = make_test_env_with_stdin(vec!["sort"], "b\n\na\n\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert_eq!("\n\na\nb\n", stdout);
    }

    #[test]
    fn help_flag() {
        let env = make_test_env_with_stdin(vec!["sort", "--help"], "");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert!(stdout.contains("Usage"));
    }

    #[test]
    fn version_flag() {
        let env = make_test_env_with_stdin(vec!["sort", "--version"], "");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert!(stdout.contains("eudaemonsh"));
    }

    // ========================================================================
    // Additional numeric sort tests
    // ========================================================================

    #[test]
    fn numeric_with_leading_spaces() {
        let env = make_test_env_with_stdin(vec!["sort", "-n"], "  10\n 2\n1\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert_eq!("1\n 2\n  10\n", stdout);
    }

    #[test]
    fn numeric_with_decimals() {
        let env = make_test_env_with_stdin(vec!["sort", "-n"], "1.5\n1.25\n1.1\n2\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert_eq!("1.1\n1.25\n1.5\n2\n", stdout);
    }

    #[test]
    fn numeric_all_zeros() {
        let env = make_test_env_with_stdin(vec!["sort", "-n"], "0\n0\n0\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("0\n0\n0\n", env.stdout.into_string());
    }

    #[test]
    fn numeric_large_numbers() {
        let env = make_test_env_with_stdin(vec!["sort", "-n"], "1000000\n100\n10000\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("100\n10000\n1000000\n", env.stdout.into_string());
    }

    // ========================================================================
    // Additional key specification tests
    // ========================================================================

    #[test]
    fn key_third_field() {
        let env = make_test_env_with_stdin(vec!["sort", "-k3"], "a b c\nd e a\ng h b\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert_eq!("d e a\ng h b\na b c\n", stdout);
    }

    #[test]
    fn key_beyond_fields_empty() {
        let env = make_test_env_with_stdin(vec!["sort", "-k5"], "a b\nc d\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        // Both have empty key, so tiebreaker by full line
        assert_eq!("a b\nc d\n", stdout);
    }

    #[test]
    fn key_with_char_position() {
        let env = make_test_env_with_stdin(vec!["sort", "-k1.2"], "abc\naaa\nabc\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        // Sorts by second character onwards: "bc", "aa", "bc"
        assert_eq!("aaa\nabc\nabc\n", stdout);
    }

    #[test]
    fn multiple_keys() {
        let env = make_test_env_with_stdin(vec!["sort", "-k1,1", "-k2n"], "a 10\na 2\nb 1\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        // First by field 1, then numerically by field 2
        assert_eq!("a 2\na 10\nb 1\n", stdout);
    }

    #[test]
    fn multiple_keys_with_reverse() {
        let env = make_test_env_with_stdin(vec!["sort", "-k1,1", "-k2nr"], "a 10\na 2\nb 1\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        // First by field 1, then reverse numeric by field 2
        assert_eq!("a 10\na 2\nb 1\n", stdout);
    }

    #[test]
    fn key_invalid_zero_field() {
        let env = make_test_env_with_stdin(vec!["sort", "-k0"], "a\nb\n");
        let result = bin(&env).unwrap();
        assert_eq!(2, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("invalid"));
    }

    // ========================================================================
    // Additional field separator tests
    // ========================================================================

    #[test]
    fn field_separator_tab() {
        let env = make_test_env_with_stdin(vec!["sort", "-t\t", "-k2"], "a\tb\nc\ta\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert_eq!("c\ta\na\tb\n", stdout);
    }

    #[test]
    fn field_separator_empty_fields() {
        let env = make_test_env_with_stdin(vec!["sort", "-t:", "-k3"], "a::c\nb::a\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert_eq!("b::a\na::c\n", stdout);
    }

    #[test]
    fn field_separator_empty_error() {
        let env = make_test_env_with_stdin(vec!["sort", "-t", ""], "a\n");
        let result = bin(&env).unwrap();
        assert_eq!(2, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("empty"));
    }

    // ========================================================================
    // Additional unique tests
    // ========================================================================

    #[test]
    fn unique_case_insensitive() {
        let env = make_test_env_with_stdin(vec!["sort", "-uf"], "A\na\nB\nb\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        // Should keep only first of each case-insensitive duplicate
        assert_eq!("A\nB\n", stdout);
    }

    #[test]
    fn unique_numeric() {
        let env = make_test_env_with_stdin(vec!["sort", "-un"], "10\n2\n10\n2\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert_eq!("2\n10\n", stdout);
    }

    #[test]
    fn unique_by_key() {
        let env = make_test_env_with_stdin(vec!["sort", "-u", "-k1,1"], "a 1\na 2\nb 1\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        // Unique by first field: "a 1" and "a 2" have same key "a", keep first
        assert_eq!("a 1\nb 1\n", stdout);
    }

    // ========================================================================
    // Additional check sorted tests
    // ========================================================================

    #[test]
    fn check_sorted_numeric() {
        let env = make_test_env_with_stdin(vec!["sort", "-cn"], "1\n2\n10\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
    }

    #[test]
    fn check_sorted_numeric_fail() {
        let env = make_test_env_with_stdin(vec!["sort", "-cn"], "1\n10\n2\n");
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
    }

    #[test]
    fn check_sorted_with_duplicates() {
        let env = make_test_env_with_stdin(vec!["sort", "-c"], "a\na\nb\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
    }

    #[test]
    fn check_sorted_unique_with_duplicates() {
        let env = make_test_env_with_stdin(vec!["sort", "-cu"], "a\na\nb\n");
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("disorder"));
    }

    #[test]
    fn check_sorted_reverse() {
        let env = make_test_env_with_stdin(vec!["sort", "-cr"], "c\nb\na\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
    }

    #[test]
    fn check_sorted_reverse_fail() {
        let env = make_test_env_with_stdin(vec!["sort", "-cr"], "a\nb\nc\n");
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
    }

    // ========================================================================
    // Additional human numeric tests
    // ========================================================================

    #[test]
    fn human_numeric_mixed() {
        let env = make_test_env_with_stdin(vec!["sort", "-h"], "500\n1K\n2K\n100\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert_eq!("100\n500\n1K\n2K\n", stdout);
    }

    #[test]
    fn human_numeric_terabytes() {
        let env = make_test_env_with_stdin(vec!["sort", "-h"], "1T\n1G\n1M\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert_eq!("1M\n1G\n1T\n", stdout);
    }

    #[test]
    fn human_numeric_with_decimals() {
        let env = make_test_env_with_stdin(vec!["sort", "-h"], "1.5K\n1K\n2K\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert_eq!("1K\n1.5K\n2K\n", stdout);
    }

    // ========================================================================
    // Additional month sort tests
    // ========================================================================

    #[test]
    fn month_sort_all_months() {
        let env = make_test_env_with_stdin(
            vec!["sort", "-M"],
            "Jul\nJan\nMar\nNov\nMay\nSep\nFeb\nApr\nJun\nAug\nOct\nDec\n",
        );
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert_eq!(
            "Jan\nFeb\nMar\nApr\nMay\nJun\nJul\nAug\nSep\nOct\nNov\nDec\n",
            stdout
        );
    }

    #[test]
    fn month_sort_unknown_first() {
        let env = make_test_env_with_stdin(vec!["sort", "-M"], "Jan\nFoo\nFeb\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        // Unknown months sort before known months
        assert_eq!("Foo\nJan\nFeb\n", stdout);
    }

    #[test]
    fn month_sort_full_names() {
        let env = make_test_env_with_stdin(vec!["sort", "-M"], "March\nJanuary\nFebruary\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert_eq!("January\nFebruary\nMarch\n", stdout);
    }

    // ========================================================================
    // Additional version sort tests
    // ========================================================================

    #[test]
    fn version_sort_complex() {
        let env = make_test_env_with_stdin(vec!["sort", "-V"], "1.0.0\n1.0.10\n1.0.2\n1.0.1\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert_eq!("1.0.0\n1.0.1\n1.0.2\n1.0.10\n", stdout);
    }

    #[test]
    fn version_sort_with_suffix() {
        let env = make_test_env_with_stdin(vec!["sort", "-V"], "1.0-beta\n1.0-alpha\n1.0\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert_eq!("1.0\n1.0-alpha\n1.0-beta\n", stdout);
    }

    #[test]
    fn version_sort_leading_zeros() {
        let env = make_test_env_with_stdin(vec!["sort", "-V"], "1.01\n1.1\n1.001\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        // Leading zeros are ignored in numeric comparison
        assert_eq!("1.001\n1.01\n1.1\n", stdout);
    }

    // ========================================================================
    // Additional dictionary order tests
    // ========================================================================

    #[test]
    fn dictionary_order() {
        let env = make_test_env_with_stdin(vec!["sort", "-d"], "a-b\nab\na b\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        // Only blanks and alphanumeric; hyphen is ignored
        assert_eq!("a b\na-b\nab\n", stdout);
    }

    #[test]
    fn dictionary_order_with_numbers() {
        let env = make_test_env_with_stdin(vec!["sort", "-d"], "a1\na-1\na 1\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        // Dictionary order: "a1" -> "a1", "a-1" -> "a1", "a 1" -> "a 1"
        // "a 1" < "a1" (space < '1'), then "a-1" and "a1" both map to "a1", tiebreaker by original
        assert_eq!("a 1\na1\na-1\n", stdout);
    }

    // ========================================================================
    // Additional ignore nonprinting tests
    // ========================================================================

    #[test]
    fn ignore_nonprinting() {
        let env = make_test_env_with_stdin(vec!["sort", "-i"], "a\x01b\nab\nac\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        // Control char is ignored, so "a\x01b" becomes "ab"
        assert_eq!("a\x01b\nab\nac\n", stdout);
    }

    // ========================================================================
    // Stable sort tests
    // ========================================================================

    #[test]
    fn stable_sort() {
        let env = make_test_env_with_stdin(vec!["sort", "-s", "-k1,1"], "a 2\na 1\nb 1\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        // With stable sort, original order preserved for equal keys
        assert_eq!("a 2\na 1\nb 1\n", stdout);
    }

    #[test]
    fn stable_sort_preserves_order() {
        let env = make_test_env_with_stdin(vec!["sort", "-s", "-k1,1"], "a 3\na 1\na 2\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        // All have same key "a", so original order preserved
        assert_eq!("a 3\na 1\na 2\n", stdout);
    }

    // ========================================================================
    // File and stdin combination tests
    // ========================================================================

    #[test]
    fn stdin_with_dash() {
        let env = make_test_env_with_stdin(vec!["sort", "-"], "c\na\nb\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("a\nb\nc\n", env.stdout.into_string());
    }

    #[test]
    fn stdin_and_file_combined() {
        let env = make_test_env_with_stdin(vec!["sort", "-", "file.txt"], "c\na\n");
        env.fs.add_file("file.txt", "d\nb\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert_eq!("a\nb\nc\nd\n", stdout);
    }

    #[test]
    fn multiple_files_interleaved() {
        let env = make_test_env_with_stdin(vec!["sort", "a.txt", "b.txt", "c.txt"], "");
        env.fs.add_file("a.txt", "z\n");
        env.fs.add_file("b.txt", "a\n");
        env.fs.add_file("c.txt", "m\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("a\nm\nz\n", env.stdout.into_string());
    }

    // ========================================================================
    // Edge cases and boundary conditions
    // ========================================================================

    #[test]
    fn very_long_line() {
        let long_line = "a".repeat(10000);
        let input = format!("{}\nb\n", long_line);
        let env = make_test_env_with_stdin(vec!["sort"], &input);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        assert!(stdout.starts_with(&long_line));
    }

    #[test]
    fn unicode_characters() {
        let env = make_test_env_with_stdin(vec!["sort"], "café\nカフェ\nalpha\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        // Lexicographic by byte value
        assert_eq!("alpha\ncafé\nカフェ\n", stdout);
    }

    #[test]
    fn whitespace_only_lines() {
        let env = make_test_env_with_stdin(vec!["sort"], "   \n\t\n \n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        // Tab comes before space in ASCII
        assert_eq!("\t\n \n   \n", stdout);
    }

    #[test]
    fn single_character_lines() {
        let env = make_test_env_with_stdin(vec!["sort"], "z\na\nm\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("a\nm\nz\n", env.stdout.into_string());
    }

    #[test]
    fn lines_with_only_numbers() {
        let env = make_test_env_with_stdin(vec!["sort"], "3\n1\n2\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        // Lexicographic sort, not numeric
        assert_eq!("1\n2\n3\n", env.stdout.into_string());
    }

    #[test]
    fn lexicographic_vs_numeric() {
        let env = make_test_env_with_stdin(vec!["sort"], "9\n10\n100\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        // Lexicographic: "10" < "100" < "9"
        assert_eq!("10\n100\n9\n", env.stdout.into_string());
    }

    // ========================================================================
    // Error handling tests
    // ========================================================================

    #[test]
    fn invalid_option() {
        let env = make_test_env_with_stdin(vec!["sort", "--invalid-option"], "a\n");
        let result = bin(&env).unwrap();
        assert_eq!(2, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("invalid") || stderr.contains("Unrecognized"));
    }

    #[test]
    fn output_file_error() {
        let env = make_test_env_with_stdin(vec!["sort", "-o", "/some/path/output.txt"], "b\na\n");
        env.fs.mkdir_all("/some/path").unwrap();
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!(
            "a\nb\n",
            env.fs.read_to_string("/some/path/output.txt").unwrap()
        );
        println!("Wrote sorted output to /some/path/output.txt");
    }

    // ========================================================================
    // Combined complex options
    // ========================================================================

    #[test]
    fn numeric_unique_reverse() {
        let env = make_test_env_with_stdin(vec!["sort", "-nur"], "1\n2\n2\n3\n1\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert_eq!("3\n2\n1\n", stdout);
    }

    #[test]
    fn key_with_all_modifiers() {
        let env = make_test_env_with_stdin(vec!["sort", "-k2,2bf"], "a  B\nb  a\nc  C\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        // Sort by field 2, ignoring case and leading blanks
        assert_eq!("b  a\na  B\nc  C\n", stdout);
    }

    #[test]
    fn check_with_key() {
        let env = make_test_env_with_stdin(vec!["sort", "-c", "-k2n"], "a 1\nb 2\nc 3\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
    }

    #[test]
    fn check_with_key_fail() {
        let env = make_test_env_with_stdin(vec!["sort", "-c", "-k2n"], "a 1\nb 3\nc 2\n");
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
    }

    // ========================================================================
    // Helper function additional tests
    // ========================================================================

    #[test]
    fn extract_key_first_field() {
        let key = KeySpec {
            start_field: 1,
            start_char: 1,
            end_field: 1,
            end_char: 0,
            modifiers: KeyModifiers::default(),
        };
        let result = extract_key("hello world", &key, None);
        println!("result: {:?}", result);
        assert_eq!("hello", result);
    }

    #[test]
    fn extract_key_middle_chars() {
        let key = KeySpec {
            start_field: 1,
            start_char: 2,
            end_field: 1,
            end_char: 4,
            modifiers: KeyModifiers::default(),
        };
        let result = extract_key("hello world", &key, None);
        println!("result: {:?}", result);
        assert_eq!("ell", result);
    }

    #[test]
    fn extract_key_with_separator() {
        let key = KeySpec {
            start_field: 2,
            start_char: 1,
            end_field: 2,
            end_char: 0,
            modifiers: KeyModifiers::default(),
        };
        let result = extract_key("a:b:c", &key, Some(':'));
        println!("result: {:?}", result);
        assert_eq!("b", result);
    }

    #[test]
    fn apply_modifiers_all() {
        let mods = KeyModifiers {
            ignore_leading_blanks: true,
            dictionary_order: true,
            ignore_case: true,
            ignore_nonprinting: true,
            ..Default::default()
        };
        let result = apply_modifiers("  Hello-World!\x01", &mods);
        println!("result: {:?}", result);
        // Trim leading blanks, keep only alphanumeric/space, uppercase, remove control
        assert_eq!("HELLOWORLD", result);
    }

    #[test]
    fn parse_numeric_with_leading_space() {
        assert_eq!(42.0, parse_numeric("  42"));
        assert_eq!(-10.0, parse_numeric(" -10 "));
        assert_eq!(3.25, parse_numeric("3.25"));
    }

    #[test]
    fn parse_human_numeric_all_suffixes() {
        assert_eq!(1024.0, parse_human_numeric("1K"));
        assert_eq!(1024.0 * 1024.0, parse_human_numeric("1M"));
        assert_eq!(1024.0 * 1024.0 * 1024.0, parse_human_numeric("1G"));
        assert_eq!(1024.0 * 1024.0 * 1024.0 * 1024.0, parse_human_numeric("1T"));
        assert_eq!(
            1024.0 * 1024.0 * 1024.0 * 1024.0 * 1024.0,
            parse_human_numeric("1P")
        );
        assert_eq!(
            1024.0 * 1024.0 * 1024.0 * 1024.0 * 1024.0 * 1024.0,
            parse_human_numeric("1E")
        );
    }

    #[test]
    fn compare_version_edge_cases() {
        assert_eq!(Ordering::Less, compare_version("", "1"));
        assert_eq!(Ordering::Greater, compare_version("1", ""));
        assert_eq!(Ordering::Equal, compare_version("", ""));
        assert_eq!(Ordering::Less, compare_version("a", "b"));
        assert_eq!(Ordering::Equal, compare_version("abc", "abc"));
    }

    #[test]
    fn split_fields_single_field() {
        let fields = split_fields("hello", None);
        println!("fields: {:?}", fields);
        assert_eq!(vec!["hello"], fields);
    }

    #[test]
    fn split_fields_leading_separator() {
        let fields = split_fields(":a:b", Some(':'));
        println!("fields: {:?}", fields);
        assert_eq!(vec!["", "a", "b"], fields);
    }

    #[test]
    fn split_fields_trailing_separator() {
        let fields = split_fields("a:b:", Some(':'));
        println!("fields: {:?}", fields);
        assert_eq!(vec!["a", "b", ""], fields);
    }
}

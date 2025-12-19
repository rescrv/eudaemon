//! The split builtin: split a file into pieces.

use getopts::Options;
use regex::Regex;

use crate::{Environment, Error, ExitCode, Filesystem, FsError, StdioIn, StdioOut};

/// Default number of lines per output file.
const DEFAULT_LINES: u64 = 1000;

/// Options for the split command.
#[derive(Clone, Debug)]
struct SplitOptions {
    /// Number of lines per file (if splitting by lines).
    lines: Option<u64>,
    /// Number of bytes per file (if splitting by bytes).
    bytes: Option<u64>,
    /// Number of chunks to split into.
    chunks: Option<u64>,
    /// Pattern to split on (regex).
    pattern: Option<Regex>,
    /// Suffix length.
    suffix_len: usize,
    /// Use numeric suffix instead of alphabetic.
    numeric_suffix: bool,
    /// Don't clobber existing files.
    no_clobber: bool,
    /// Auto-extend suffix length if needed.
    auto_suffix: bool,
}

impl Default for SplitOptions {
    fn default() -> Self {
        Self {
            lines: None,
            bytes: None,
            chunks: None,
            pattern: None,
            suffix_len: 2,
            numeric_suffix: false,
            no_clobber: false,
            auto_suffix: true,
        }
    }
}

fn build_options() -> Options {
    let mut opts = Options::new();
    opts.optopt(
        "l",
        "",
        "Create split files line_count lines in length.",
        "line_count",
    );
    opts.optopt(
        "b",
        "",
        "Create split files byte_count bytes in length.",
        "byte_count",
    );
    opts.optopt(
        "n",
        "",
        "Split file into chunk_count smaller files.",
        "chunk_count",
    );
    opts.optopt(
        "p",
        "",
        "Split whenever a line matches pattern (extended regex).",
        "pattern",
    );
    opts.optopt(
        "a",
        "",
        "Use suffix_length letters to form the suffix.",
        "suffix_length",
    );
    opts.optflag("d", "", "Use a numeric suffix instead of alphabetic.");
    opts.optflag(
        "c",
        "",
        "Continue creating files and do not overwrite existing output files.",
    );
    opts
}

/// Parse a byte count string that may include suffixes (k, m, g).
fn parse_byte_count(s: &str) -> Option<u64> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }

    let (num_str, multiplier) =
        if let Some(prefix) = s.strip_suffix(|c: char| c.is_ascii_alphabetic()) {
            let suffix = s.chars().last()?;
            let mult = match suffix.to_ascii_lowercase() {
                'k' => 1024,
                'm' => 1024 * 1024,
                'g' => 1024 * 1024 * 1024,
                _ => return None,
            };
            (prefix, mult)
        } else {
            (s, 1)
        };

    let num: u64 = num_str.parse().ok()?;
    num.checked_mul(multiplier)
}

/// Generate suffix for file number.
fn generate_suffix(num: u64, len: usize, numeric: bool) -> String {
    let base: u64 = if numeric { 10 } else { 26 };
    let mut result = Vec::with_capacity(len);
    let mut n = num;

    for _ in 0..len {
        let digit = (n % base) as u8;
        let c = if numeric { b'0' + digit } else { b'a' + digit };
        result.push(c as char);
        n /= base;
    }

    result.reverse();
    result.into_iter().collect()
}

/// Calculate max files for a given suffix length.
fn max_files(suffix_len: usize, numeric: bool) -> u64 {
    let base: u64 = if numeric { 10 } else { 26 };
    base.saturating_pow(suffix_len as u32)
}

/// State for generating output file names.
struct FileNameGenerator {
    prefix: String,
    suffix_len: usize,
    numeric: bool,
    auto_suffix: bool,
    file_num: u64,
}

impl FileNameGenerator {
    fn new(prefix: String, suffix_len: usize, numeric: bool, auto_suffix: bool) -> Self {
        Self {
            prefix,
            suffix_len,
            numeric,
            auto_suffix,
            file_num: 0,
        }
    }

    fn next_name(&mut self) -> Option<String> {
        let max = max_files(self.suffix_len, self.numeric);

        if self.file_num >= max {
            if self.auto_suffix && !self.numeric {
                self.suffix_len += 1;
                self.file_num = 0;
            } else {
                return None;
            }
        }

        let suffix = generate_suffix(self.file_num, self.suffix_len, self.numeric);
        self.file_num += 1;
        Some(format!("{}{}", self.prefix, suffix))
    }
}

/// The split builtin: split a file into pieces.
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
            env.stderr.write_line(&format!("split: {}", e))?;
            return Ok(ExitCode::from(1));
        }
    };

    let mut options = SplitOptions {
        numeric_suffix: matches.opt_present("d"),
        no_clobber: matches.opt_present("c"),
        ..Default::default()
    };

    // Parse -a flag
    if let Some(a) = matches.opt_str("a") {
        match a.parse::<usize>() {
            Ok(0) => {
                options.suffix_len = 2;
                options.auto_suffix = true;
            }
            Ok(n) => {
                options.suffix_len = n;
                options.auto_suffix = false;
            }
            Err(_) => {
                env.stderr
                    .write_line(&format!("split: {}: invalid suffix length", a))?;
                return Ok(ExitCode::from(1));
            }
        }
    }

    // Parse -l flag
    if let Some(l) = matches.opt_str("l") {
        match l.parse::<u64>() {
            Ok(n) if n > 0 => options.lines = Some(n),
            _ => {
                env.stderr
                    .write_line(&format!("split: {}: invalid line count", l))?;
                return Ok(ExitCode::from(1));
            }
        }
    }

    // Parse -b flag
    if let Some(b) = matches.opt_str("b") {
        match parse_byte_count(&b) {
            Some(n) if n > 0 => options.bytes = Some(n),
            _ => {
                env.stderr
                    .write_line(&format!("split: {}: invalid byte count", b))?;
                return Ok(ExitCode::from(1));
            }
        }
    }

    // Parse -n flag
    if let Some(n) = matches.opt_str("n") {
        match n.parse::<u64>() {
            Ok(count) if count > 0 => options.chunks = Some(count),
            _ => {
                env.stderr
                    .write_line(&format!("split: {}: invalid chunk count", n))?;
                return Ok(ExitCode::from(1));
            }
        }
    }

    // Parse -p flag
    if let Some(p) = matches.opt_str("p") {
        match Regex::new(&p) {
            Ok(re) => options.pattern = Some(re),
            Err(e) => {
                env.stderr
                    .write_line(&format!("split: {}: invalid regex: {}", p, e))?;
                return Ok(ExitCode::from(1));
            }
        }
    }

    // Validate mutually exclusive options
    let mode_count = [
        options.bytes.is_some(),
        options.chunks.is_some(),
        options.pattern.is_some(),
    ]
    .iter()
    .filter(|&&b| b)
    .count();

    if mode_count > 1 {
        env.stderr
            .write_line("split: -b, -n, and -p are mutually exclusive")?;
        return Ok(ExitCode::from(1));
    }

    if options.pattern.is_some() && options.lines.is_some() {
        env.stderr.write_line("split: -p is incompatible with -l")?;
        return Ok(ExitCode::from(1));
    }

    // Get input file and prefix from positional arguments
    let (input_file, prefix) = match matches.free.len() {
        0 => (None, "x".to_string()),
        1 => {
            let arg = &matches.free[0];
            if arg == "-" {
                (None, "x".to_string())
            } else {
                (Some(arg.as_str()), "x".to_string())
            }
        }
        2 => {
            let file_arg = &matches.free[0];
            let prefix = matches.free[1].clone();
            if file_arg == "-" {
                (None, prefix)
            } else {
                (Some(file_arg.as_str()), prefix)
            }
        }
        _ => {
            env.stderr.write_line("split: too many arguments")?;
            return Ok(ExitCode::from(1));
        }
    };

    // Read input
    let input = if let Some(path) = input_file {
        match env.fs.read_to_string(path) {
            Ok(contents) => contents,
            Err(FsError::Io(e)) => {
                env.stderr.write_line(&format!("split: {}: {}", path, e))?;
                return Ok(ExitCode::from(1));
            }
        }
    } else {
        let mut contents = String::new();
        while let Some(line) = env.stdin.read_line()? {
            contents.push_str(&line);
            contents.push('\n');
        }
        contents
    };

    // Dispatch to appropriate split function
    if let Some(byte_count) = options.bytes {
        split_by_bytes(env, &input, byte_count, &prefix, &options)
    } else if let Some(chunk_count) = options.chunks {
        split_into_chunks(env, &input, chunk_count, &prefix, &options)
    } else if let Some(ref pattern) = options.pattern {
        split_by_pattern(env, &input, pattern, &prefix, &options)
    } else {
        let line_count = options.lines.unwrap_or(DEFAULT_LINES);
        split_by_lines(env, &input, line_count, &prefix, &options)
    }
}

/// Split input by line count.
fn split_by_lines<SI, SO, SE, FS>(
    env: &Environment<SI, SO, SE, FS>,
    input: &str,
    line_count: u64,
    prefix: &str,
    options: &SplitOptions,
) -> Result<ExitCode, Error>
where
    SI: StdioIn,
    SO: StdioOut,
    SE: StdioOut,
    FS: Filesystem,
{
    let mut name_gen = FileNameGenerator::new(
        prefix.to_string(),
        options.suffix_len,
        options.numeric_suffix,
        options.auto_suffix,
    );

    let mut current_lines: Vec<&str> = Vec::new();
    let mut lines_in_current = 0u64;

    for line in input.lines() {
        current_lines.push(line);
        lines_in_current += 1;

        if lines_in_current >= line_count {
            let filename = loop {
                let name = match name_gen.next_name() {
                    Some(n) => n,
                    None => {
                        env.stderr.write_line("split: too many files")?;
                        return Ok(ExitCode::from(1));
                    }
                };
                if options.no_clobber && env.fs.exists(&name) {
                    continue;
                }
                break name;
            };

            let content: String = current_lines.iter().map(|l| format!("{}\n", l)).collect();
            env.fs.write_string(&filename, &content)?;

            current_lines.clear();
            lines_in_current = 0;
        }
    }

    // Write remaining lines
    if !current_lines.is_empty() {
        let filename = loop {
            let name = match name_gen.next_name() {
                Some(n) => n,
                None => {
                    env.stderr.write_line("split: too many files")?;
                    return Ok(ExitCode::from(1));
                }
            };
            if options.no_clobber && env.fs.exists(&name) {
                continue;
            }
            break name;
        };

        let content: String = current_lines.iter().map(|l| format!("{}\n", l)).collect();
        env.fs.write_string(&filename, &content)?;
    }

    Ok(ExitCode::from(0))
}

/// Split input by byte count.
fn split_by_bytes<SI, SO, SE, FS>(
    env: &Environment<SI, SO, SE, FS>,
    input: &str,
    byte_count: u64,
    prefix: &str,
    options: &SplitOptions,
) -> Result<ExitCode, Error>
where
    SI: StdioIn,
    SO: StdioOut,
    SE: StdioOut,
    FS: Filesystem,
{
    let mut name_gen = FileNameGenerator::new(
        prefix.to_string(),
        options.suffix_len,
        options.numeric_suffix,
        options.auto_suffix,
    );

    let bytes = input.as_bytes();
    let byte_count = byte_count as usize;
    let mut offset = 0;

    while offset < bytes.len() {
        let end = std::cmp::min(offset + byte_count, bytes.len());
        let chunk = &bytes[offset..end];

        let filename = loop {
            let name = match name_gen.next_name() {
                Some(n) => n,
                None => {
                    env.stderr.write_line("split: too many files")?;
                    return Ok(ExitCode::from(1));
                }
            };
            if options.no_clobber && env.fs.exists(&name) {
                continue;
            }
            break name;
        };

        let content = String::from_utf8_lossy(chunk);
        env.fs.write_string(&filename, &content)?;

        offset = end;
    }

    Ok(ExitCode::from(0))
}

/// Split input into N chunks.
fn split_into_chunks<SI, SO, SE, FS>(
    env: &Environment<SI, SO, SE, FS>,
    input: &str,
    chunk_count: u64,
    prefix: &str,
    options: &SplitOptions,
) -> Result<ExitCode, Error>
where
    SI: StdioIn,
    SO: StdioOut,
    SE: StdioOut,
    FS: Filesystem,
{
    let total_bytes = input.len() as u64;

    if chunk_count > total_bytes {
        env.stderr.write_line(&format!(
            "split: can't split into more than {} files",
            total_bytes
        ))?;
        return Ok(ExitCode::from(1));
    }

    let bytes_per_chunk = total_bytes / chunk_count;
    split_by_bytes(env, input, bytes_per_chunk, prefix, options)
}

/// Split input by pattern match.
fn split_by_pattern<SI, SO, SE, FS>(
    env: &Environment<SI, SO, SE, FS>,
    input: &str,
    pattern: &Regex,
    prefix: &str,
    options: &SplitOptions,
) -> Result<ExitCode, Error>
where
    SI: StdioIn,
    SO: StdioOut,
    SE: StdioOut,
    FS: Filesystem,
{
    let mut name_gen = FileNameGenerator::new(
        prefix.to_string(),
        options.suffix_len,
        options.numeric_suffix,
        options.auto_suffix,
    );

    let mut current_lines: Vec<&str> = Vec::new();
    let mut first_file = true;

    for line in input.lines() {
        if pattern.is_match(line) && !current_lines.is_empty() {
            // Write current chunk
            let filename = loop {
                let name = match name_gen.next_name() {
                    Some(n) => n,
                    None => {
                        env.stderr.write_line("split: too many files")?;
                        return Ok(ExitCode::from(1));
                    }
                };
                if options.no_clobber && env.fs.exists(&name) {
                    continue;
                }
                break name;
            };

            let content: String = current_lines.iter().map(|l| format!("{}\n", l)).collect();
            env.fs.write_string(&filename, &content)?;

            current_lines.clear();
            first_file = false;
        }

        // For the first file, create it if we haven't yet
        if first_file && current_lines.is_empty() && !pattern.is_match(line) {
            // This line doesn't match, so it goes into the first file
        }

        current_lines.push(line);
    }

    // Write remaining lines
    if !current_lines.is_empty() {
        let filename = loop {
            let name = match name_gen.next_name() {
                Some(n) => n,
                None => {
                    env.stderr.write_line("split: too many files")?;
                    return Ok(ExitCode::from(1));
                }
            };
            if options.no_clobber && env.fs.exists(&name) {
                continue;
            }
            break name;
        };

        let content: String = current_lines.iter().map(|l| format!("{}\n", l)).collect();
        env.fs.write_string(&filename, &content)?;
    }

    Ok(ExitCode::from(0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TestFilesystemExt;
    use crate::test_utils::make_test_env_with_stdin;

    // ========================================================================
    // Helper function tests
    // ========================================================================

    #[test]
    fn generate_suffix_alphabetic() {
        assert_eq!("aa", generate_suffix(0, 2, false));
        assert_eq!("ab", generate_suffix(1, 2, false));
        assert_eq!("az", generate_suffix(25, 2, false));
        assert_eq!("ba", generate_suffix(26, 2, false));
        assert_eq!("zz", generate_suffix(675, 2, false));
    }

    #[test]
    fn generate_suffix_numeric() {
        assert_eq!("00", generate_suffix(0, 2, true));
        assert_eq!("01", generate_suffix(1, 2, true));
        assert_eq!("09", generate_suffix(9, 2, true));
        assert_eq!("10", generate_suffix(10, 2, true));
        assert_eq!("99", generate_suffix(99, 2, true));
    }

    #[test]
    fn generate_suffix_three_chars() {
        assert_eq!("aaa", generate_suffix(0, 3, false));
        assert_eq!("aab", generate_suffix(1, 3, false));
        assert_eq!("aba", generate_suffix(26, 3, false));
    }

    #[test]
    fn max_files_calculation() {
        assert_eq!(676, max_files(2, false));
        assert_eq!(100, max_files(2, true));
        assert_eq!(17576, max_files(3, false));
        assert_eq!(1000, max_files(3, true));
    }

    #[test]
    fn parse_byte_count_plain() {
        assert_eq!(Some(100), parse_byte_count("100"));
        assert_eq!(Some(1), parse_byte_count("1"));
    }

    #[test]
    fn parse_byte_count_k_suffix() {
        assert_eq!(Some(1024), parse_byte_count("1k"));
        assert_eq!(Some(1024), parse_byte_count("1K"));
        assert_eq!(Some(2048), parse_byte_count("2k"));
    }

    #[test]
    fn parse_byte_count_m_suffix() {
        assert_eq!(Some(1024 * 1024), parse_byte_count("1m"));
        assert_eq!(Some(1024 * 1024), parse_byte_count("1M"));
    }

    #[test]
    fn parse_byte_count_g_suffix() {
        assert_eq!(Some(1024 * 1024 * 1024), parse_byte_count("1g"));
        assert_eq!(Some(1024 * 1024 * 1024), parse_byte_count("1G"));
    }

    #[test]
    fn parse_byte_count_invalid() {
        assert_eq!(None, parse_byte_count(""));
        assert_eq!(None, parse_byte_count("abc"));
        assert_eq!(None, parse_byte_count("1x"));
    }

    // ========================================================================
    // Basic functionality tests
    // ========================================================================

    #[test]
    fn default_split_1000_lines() {
        let mut input = String::new();
        for i in 1..=2500 {
            input.push_str(&format!("line{}\n", i));
        }
        let env = make_test_env_with_stdin(vec!["split"], &input);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());

        let xaa = env.fs.read_to_string("xaa").unwrap();
        let xab = env.fs.read_to_string("xab").unwrap();
        let xac = env.fs.read_to_string("xac").unwrap();

        assert_eq!(1000, xaa.lines().count());
        assert_eq!(1000, xab.lines().count());
        assert_eq!(500, xac.lines().count());
        assert!(xaa.starts_with("line1\n"));
        assert!(xab.starts_with("line1001\n"));
        assert!(xac.starts_with("line2001\n"));
    }

    #[test]
    fn split_stdin_two_lines_each() {
        let env =
            make_test_env_with_stdin(vec!["split", "-l", "2"], "one\ntwo\nthree\nfour\nfive\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());

        assert_eq!("one\ntwo\n", env.fs.read_to_string("xaa").unwrap());
        assert_eq!("three\nfour\n", env.fs.read_to_string("xab").unwrap());
        assert_eq!("five\n", env.fs.read_to_string("xac").unwrap());
    }

    #[test]
    fn split_from_file() {
        let env = make_test_env_with_stdin(vec!["split", "-l", "2", "input.txt"], "");
        env.fs.add_file("input.txt", "a\nb\nc\nd\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());

        assert_eq!("a\nb\n", env.fs.read_to_string("xaa").unwrap());
        assert_eq!("c\nd\n", env.fs.read_to_string("xab").unwrap());
    }

    #[test]
    fn split_with_custom_prefix() {
        let env = make_test_env_with_stdin(vec!["split", "-l", "1", "-", "out_"], "foo\nbar\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());

        assert_eq!("foo\n", env.fs.read_to_string("out_aa").unwrap());
        assert_eq!("bar\n", env.fs.read_to_string("out_ab").unwrap());
    }

    #[test]
    fn split_file_with_prefix() {
        let env = make_test_env_with_stdin(vec!["split", "-l", "1", "in.txt", "chunk_"], "");
        env.fs.add_file("in.txt", "x\ny\nz\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());

        assert_eq!("x\n", env.fs.read_to_string("chunk_aa").unwrap());
        assert_eq!("y\n", env.fs.read_to_string("chunk_ab").unwrap());
        assert_eq!("z\n", env.fs.read_to_string("chunk_ac").unwrap());
    }

    // ========================================================================
    // -b flag: split by bytes
    // ========================================================================

    #[test]
    fn split_by_bytes_simple() {
        let env = make_test_env_with_stdin(vec!["split", "-b", "5"], "0123456789");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());

        assert_eq!("01234", env.fs.read_to_string("xaa").unwrap());
        assert_eq!("56789", env.fs.read_to_string("xab").unwrap());
    }

    #[test]
    fn split_by_bytes_with_remainder() {
        let env = make_test_env_with_stdin(vec!["split", "-b", "3", "input.txt"], "");
        env.fs.add_file("input.txt", "12345678");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());

        assert_eq!("123", env.fs.read_to_string("xaa").unwrap());
        assert_eq!("456", env.fs.read_to_string("xab").unwrap());
        assert_eq!("78", env.fs.read_to_string("xac").unwrap());
    }

    #[test]
    fn split_by_bytes_k_suffix() {
        let input = "x".repeat(2048);
        let env = make_test_env_with_stdin(vec!["split", "-b", "1k"], &input);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());

        assert_eq!(1024, env.fs.read_to_string("xaa").unwrap().len());
        assert_eq!(1024, env.fs.read_to_string("xab").unwrap().len());
    }

    // ========================================================================
    // -n flag: split into chunks
    // ========================================================================

    #[test]
    fn split_into_chunks() {
        let env = make_test_env_with_stdin(vec!["split", "-n", "3"], "123456789");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());

        assert_eq!("123", env.fs.read_to_string("xaa").unwrap());
        assert_eq!("456", env.fs.read_to_string("xab").unwrap());
        assert_eq!("789", env.fs.read_to_string("xac").unwrap());
    }

    #[test]
    fn split_into_chunks_more_than_bytes() {
        let env = make_test_env_with_stdin(vec!["split", "-n", "100"], "abc");
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("can't split into more than"));
    }

    // ========================================================================
    // -p flag: split by pattern
    // ========================================================================

    #[test]
    fn split_by_pattern() {
        let input = "header\ndata1\ndata2\nheader\ndata3\n";
        let env = make_test_env_with_stdin(vec!["split", "-p", "^header"], input);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());

        assert_eq!(
            "header\ndata1\ndata2\n",
            env.fs.read_to_string("xaa").unwrap()
        );
        assert_eq!("header\ndata3\n", env.fs.read_to_string("xab").unwrap());
    }

    #[test]
    fn split_by_pattern_no_initial_match() {
        let input = "prelude\nchapter\ntext\nchapter\nmore\n";
        let env = make_test_env_with_stdin(vec!["split", "-p", "^chapter"], input);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());

        assert_eq!("prelude\n", env.fs.read_to_string("xaa").unwrap());
        assert_eq!("chapter\ntext\n", env.fs.read_to_string("xab").unwrap());
        assert_eq!("chapter\nmore\n", env.fs.read_to_string("xac").unwrap());
    }

    // ========================================================================
    // -d flag: numeric suffix
    // ========================================================================

    #[test]
    fn numeric_suffix() {
        let env = make_test_env_with_stdin(vec!["split", "-d", "-l", "1"], "a\nb\nc\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());

        assert_eq!("a\n", env.fs.read_to_string("x00").unwrap());
        assert_eq!("b\n", env.fs.read_to_string("x01").unwrap());
        assert_eq!("c\n", env.fs.read_to_string("x02").unwrap());
    }

    // ========================================================================
    // -a flag: suffix length
    // ========================================================================

    #[test]
    fn custom_suffix_length() {
        let env = make_test_env_with_stdin(vec!["split", "-a", "3", "-l", "1"], "a\nb\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());

        assert_eq!("a\n", env.fs.read_to_string("xaaa").unwrap());
        assert_eq!("b\n", env.fs.read_to_string("xaab").unwrap());
    }

    #[test]
    fn suffix_length_one() {
        let env = make_test_env_with_stdin(vec!["split", "-a", "1", "-l", "1"], "a\nb\nc\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());

        assert_eq!("a\n", env.fs.read_to_string("xa").unwrap());
        assert_eq!("b\n", env.fs.read_to_string("xb").unwrap());
        assert_eq!("c\n", env.fs.read_to_string("xc").unwrap());
    }

    // ========================================================================
    // -c flag: no clobber
    // ========================================================================

    #[test]
    fn no_clobber_skips_existing() {
        let env = make_test_env_with_stdin(vec!["split", "-c", "-l", "1"], "a\nb\nc\n");
        env.fs.add_file("xab", "existing");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());

        assert_eq!("a\n", env.fs.read_to_string("xaa").unwrap());
        assert_eq!("existing", env.fs.read_to_string("xab").unwrap());
        assert_eq!("b\n", env.fs.read_to_string("xac").unwrap());
        assert_eq!("c\n", env.fs.read_to_string("xad").unwrap());
    }

    // ========================================================================
    // Error cases
    // ========================================================================

    #[test]
    fn invalid_line_count() {
        let env = make_test_env_with_stdin(vec!["split", "-l", "0"], "test");
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("invalid line count"));
    }

    #[test]
    fn invalid_line_count_negative() {
        let env = make_test_env_with_stdin(vec!["split", "-l", "-5"], "test");
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("invalid line count"));
    }

    #[test]
    fn invalid_byte_count() {
        let env = make_test_env_with_stdin(vec!["split", "-b", "0"], "test");
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("invalid byte count"));
    }

    #[test]
    fn invalid_chunk_count() {
        let env = make_test_env_with_stdin(vec!["split", "-n", "0"], "test");
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("invalid chunk count"));
    }

    #[test]
    fn invalid_regex() {
        let env = make_test_env_with_stdin(vec!["split", "-p", "[invalid"], "test");
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("invalid regex"));
    }

    #[test]
    fn mutually_exclusive_b_n() {
        let env = make_test_env_with_stdin(vec!["split", "-b", "10", "-n", "5"], "test");
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("mutually exclusive"));
    }

    #[test]
    fn pattern_incompatible_with_lines() {
        let env = make_test_env_with_stdin(vec!["split", "-p", "test", "-l", "10"], "test");
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("incompatible"));
    }

    #[test]
    fn file_not_found() {
        let env = make_test_env_with_stdin(vec!["split", "nonexistent.txt"], "");
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("nonexistent.txt"));
    }

    #[test]
    fn too_many_arguments() {
        let env = make_test_env_with_stdin(vec!["split", "a", "b", "c"], "");
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("too many arguments"));
    }

    #[test]
    fn invalid_suffix_length() {
        let env = make_test_env_with_stdin(vec!["split", "-a", "abc"], "test");
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("invalid suffix length"));
    }

    // ========================================================================
    // Edge cases
    // ========================================================================

    #[test]
    fn empty_input() {
        let env = make_test_env_with_stdin(vec!["split"], "");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert!(!env.fs.exists("xaa"));
    }

    #[test]
    fn single_line_input() {
        let env = make_test_env_with_stdin(vec!["split", "-l", "10"], "single line\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("single line\n", env.fs.read_to_string("xaa").unwrap());
    }

    #[test]
    fn dash_means_stdin() {
        let env = make_test_env_with_stdin(vec!["split", "-l", "1", "-"], "from stdin\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("from stdin\n", env.fs.read_to_string("xaa").unwrap());
    }

    #[test]
    fn suffix_a_zero_enables_auto() {
        let env = make_test_env_with_stdin(vec!["split", "-a", "0", "-l", "1"], "a\nb\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("a\n", env.fs.read_to_string("xaa").unwrap());
        assert_eq!("b\n", env.fs.read_to_string("xab").unwrap());
    }

    // ========================================================================
    // FileNameGenerator tests
    // ========================================================================

    #[test]
    fn file_name_generator_basic() {
        let mut name_gen = FileNameGenerator::new("x".to_string(), 2, false, false);
        assert_eq!(Some("xaa".to_string()), name_gen.next_name());
        assert_eq!(Some("xab".to_string()), name_gen.next_name());
        assert_eq!(Some("xac".to_string()), name_gen.next_name());
    }

    #[test]
    fn file_name_generator_numeric() {
        let mut name_gen = FileNameGenerator::new("out".to_string(), 2, true, false);
        assert_eq!(Some("out00".to_string()), name_gen.next_name());
        assert_eq!(Some("out01".to_string()), name_gen.next_name());
        assert_eq!(Some("out02".to_string()), name_gen.next_name());
        for _ in 3..99 {
            name_gen.next_name();
        }
        assert_eq!(Some("out99".to_string()), name_gen.next_name());
        assert_eq!(None, name_gen.next_name());
    }

    #[test]
    fn file_name_generator_auto_extend() {
        let mut name_gen = FileNameGenerator::new("x".to_string(), 1, false, true);
        for i in 0..26 {
            let name = name_gen.next_name().unwrap();
            let expected = format!("x{}", (b'a' + i) as char);
            assert_eq!(expected, name);
        }
        assert_eq!(Some("xaa".to_string()), name_gen.next_name());
    }
}

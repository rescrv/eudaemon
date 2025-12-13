//! The cut utility: cut out selected portions of each line of a file.

use getopts::Options;

use crate::{Environment, Error, ExitCode, Filesystem, Stderr, Stdin, Stdout};

/// The mode of operation for cut.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CutMode {
    /// Cut by byte positions (-b).
    Bytes,
    /// Cut by character positions (-c).
    Characters,
    /// Cut by field positions (-f).
    Fields,
}

/// Options for the cut command.
#[derive(Clone, Debug)]
struct CutOptions {
    /// The mode of operation.
    mode: CutMode,
    /// The field delimiter character (default is tab).
    delimiter: char,
    /// Whether to suppress lines with no delimiter (-s flag).
    suppress_no_delim: bool,
    /// Whether to use whitespace as delimiter (-w flag).
    whitespace_delim: bool,
    /// The selected positions (1-indexed, true means selected).
    positions: Vec<bool>,
    /// If set, select all positions from this index to end of line.
    /// This handles ranges like "5-" meaning "from 5 to end".
    autostop: Option<usize>,
}

impl Default for CutOptions {
    fn default() -> Self {
        Self {
            mode: CutMode::Bytes,
            delimiter: '\t',
            suppress_no_delim: false,
            whitespace_delim: false,
            positions: Vec::new(),
            autostop: None,
        }
    }
}

impl CutOptions {
    /// Check if a position (1-indexed) is selected.
    fn is_selected(&self, pos: usize) -> bool {
        if pos == 0 {
            return false;
        }
        if let Some(stop) = self.autostop
            && pos >= stop
        {
            return true;
        }
        self.positions.get(pos).copied().unwrap_or(false)
    }
}

/// Parse a list specification into selected positions.
/// The list is a comma or whitespace separated set of numbers and ranges.
/// Examples: "1,3,5" "1-5" "-3" "5-" "1-3,7-9"
fn parse_list(list: &str, opts: &mut CutOptions) -> Result<(), String> {
    // Split on comma or whitespace
    for part in list.split(|c: char| c == ',' || c.is_whitespace()) {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }

        // Parse the range
        if let Some(dash_pos) = part.find('-') {
            let left = &part[..dash_pos];
            let right = &part[dash_pos + 1..];

            let start: usize = if left.is_empty() {
                1 // "-N" means 1 to N
            } else {
                left.parse()
                    .map_err(|_| format!("illegal list value: {}", part))?
            };

            if start == 0 {
                return Err("values may not include zero".to_string());
            }

            if right.is_empty() {
                // "N-" means N to end of line
                opts.autostop = Some(start);
                ensure_capacity(&mut opts.positions, start);
                opts.positions[start] = true;
            } else {
                let end: usize = right
                    .parse()
                    .map_err(|_| format!("illegal list value: {}", part))?;
                if end == 0 {
                    return Err("values may not include zero".to_string());
                }
                if end < start {
                    return Err(format!("illegal list value: {}", part));
                }
                ensure_capacity(&mut opts.positions, end);
                for i in start..=end {
                    opts.positions[i] = true;
                }
            }
        } else {
            // Single number
            let pos: usize = part
                .parse()
                .map_err(|_| format!("illegal list value: {}", part))?;
            if pos == 0 {
                return Err("values may not include zero".to_string());
            }
            ensure_capacity(&mut opts.positions, pos);
            opts.positions[pos] = true;
        }
    }

    if opts.positions.iter().skip(1).all(|&x| !x) && opts.autostop.is_none() {
        return Err("no fields or positions specified".to_string());
    }

    Ok(())
}

/// Ensure the positions vector has capacity for index `n`.
fn ensure_capacity(positions: &mut Vec<bool>, n: usize) {
    if positions.len() <= n {
        positions.resize(n + 1, false);
    }
}

fn build_options() -> Options {
    let mut opts = Options::new();
    opts.optopt("b", "", "Select only these bytes", "LIST");
    opts.optopt("c", "", "Select only these characters", "LIST");
    opts.optopt(
        "d",
        "",
        "Use DELIM instead of TAB for field delimiter",
        "DELIM",
    );
    opts.optopt("f", "", "Select only these fields", "LIST");
    opts.optflag("s", "", "Do not print lines not containing delimiters");
    opts.optflag("w", "", "Use whitespace (spaces and tabs) as the delimiter");
    opts
}

/// The cut builtin: cut out selected portions of each line.
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
            env.stderr.write_line(&format!("cut: {}", e))?;
            return Ok(ExitCode::from(1));
        }
    };

    let mut cut_opts = CutOptions::default();

    // Count how many of -b, -c, -f are specified
    let b_opt = matches.opt_str("b");
    let c_opt = matches.opt_str("c");
    let f_opt = matches.opt_str("f");

    let mode_count = [&b_opt, &c_opt, &f_opt]
        .iter()
        .filter(|x| x.is_some())
        .count();

    if mode_count == 0 {
        env.stderr
            .write_line("cut: you must specify a list of bytes, characters, or fields")?;
        return Ok(ExitCode::from(1));
    }

    if mode_count > 1 {
        env.stderr
            .write_line("cut: only one type of list may be specified")?;
        return Ok(ExitCode::from(1));
    }

    // Parse the list and set mode
    if let Some(list) = b_opt {
        cut_opts.mode = CutMode::Bytes;
        if let Err(e) = parse_list(&list, &mut cut_opts) {
            env.stderr.write_line(&format!("cut: [-b] list: {}", e))?;
            return Ok(ExitCode::from(1));
        }
    } else if let Some(list) = c_opt {
        cut_opts.mode = CutMode::Characters;
        if let Err(e) = parse_list(&list, &mut cut_opts) {
            env.stderr.write_line(&format!("cut: [-c] list: {}", e))?;
            return Ok(ExitCode::from(1));
        }
    } else if let Some(list) = f_opt {
        cut_opts.mode = CutMode::Fields;
        if let Err(e) = parse_list(&list, &mut cut_opts) {
            env.stderr.write_line(&format!("cut: [-f] list: {}", e))?;
            return Ok(ExitCode::from(1));
        }
    }

    // Parse delimiter
    if let Some(d) = matches.opt_str("d") {
        if cut_opts.mode != CutMode::Fields {
            env.stderr.write_line("cut: -d may only be used with -f")?;
            return Ok(ExitCode::from(1));
        }
        let mut chars = d.chars();
        match (chars.next(), chars.next()) {
            (Some(c), None) => cut_opts.delimiter = c,
            (None, _) => {
                env.stderr.write_line("cut: bad delimiter")?;
                return Ok(ExitCode::from(1));
            }
            (Some(_), Some(_)) => {
                env.stderr.write_line("cut: bad delimiter")?;
                return Ok(ExitCode::from(1));
            }
        }
    }

    // Parse -s flag
    if matches.opt_present("s") {
        if cut_opts.mode != CutMode::Fields {
            env.stderr.write_line("cut: -s may only be used with -f")?;
            return Ok(ExitCode::from(1));
        }
        cut_opts.suppress_no_delim = true;
    }

    // Parse -w flag
    if matches.opt_present("w") {
        if cut_opts.mode != CutMode::Fields {
            env.stderr.write_line("cut: -w may only be used with -f")?;
            return Ok(ExitCode::from(1));
        }
        if matches.opt_present("d") {
            env.stderr
                .write_line("cut: -w and -d may not be used together")?;
            return Ok(ExitCode::from(1));
        }
        cut_opts.whitespace_delim = true;
    }

    let mut exit_code: i8 = 0;

    if matches.free.is_empty() {
        // Read from stdin
        cut_stdin(env, &cut_opts)?;
    } else {
        for file in &matches.free {
            if file == "-" {
                cut_stdin(env, &cut_opts)?;
            } else if let Err(code) = cut_file(env, file, &cut_opts) {
                exit_code = code;
            }
        }
    }

    Ok(ExitCode::from(exit_code))
}

fn cut_stdin<SI, SO, SE, FS>(
    env: &Environment<SI, SO, SE, FS>,
    opts: &CutOptions,
) -> Result<(), Error>
where
    SI: Stdin,
    SO: Stdout,
    SE: Stderr,
    FS: Filesystem,
{
    while let Some(line) = env.stdin.read_line()? {
        process_line(env, &line, opts)?;
    }
    Ok(())
}

fn cut_file<SI, SO, SE, FS>(
    env: &Environment<SI, SO, SE, FS>,
    path: &str,
    opts: &CutOptions,
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
                if process_line(env, line, opts).is_err() {
                    return Err(1);
                }
            }
            Ok(())
        }
        Err(Error::Io(e)) => {
            let _ = env.stderr.write_line(&format!("cut: {}: {}", path, e));
            Err(1)
        }
        Err(_e) => Err(1),
    }
}

/// Process a single line with the given options.
fn process_line<SI, SO, SE, FS>(
    env: &Environment<SI, SO, SE, FS>,
    line: &str,
    opts: &CutOptions,
) -> Result<(), Error>
where
    SI: Stdin,
    SO: Stdout,
    SE: Stderr,
    FS: Filesystem,
{
    match opts.mode {
        CutMode::Bytes => cut_bytes(env, line, opts),
        CutMode::Characters => cut_characters(env, line, opts),
        CutMode::Fields => cut_fields(env, line, opts),
    }
}

/// Cut by byte positions.
fn cut_bytes<SI, SO, SE, FS>(
    env: &Environment<SI, SO, SE, FS>,
    line: &str,
    opts: &CutOptions,
) -> Result<(), Error>
where
    SI: Stdin,
    SO: Stdout,
    SE: Stderr,
    FS: Filesystem,
{
    let bytes = line.as_bytes();
    let mut output = String::new();

    for (i, &byte) in bytes.iter().enumerate() {
        let pos = i + 1; // 1-indexed
        if opts.is_selected(pos) {
            output.push(byte as char);
        }
    }

    env.stdout.write_line(&output)
}

/// Cut by character positions.
fn cut_characters<SI, SO, SE, FS>(
    env: &Environment<SI, SO, SE, FS>,
    line: &str,
    opts: &CutOptions,
) -> Result<(), Error>
where
    SI: Stdin,
    SO: Stdout,
    SE: Stderr,
    FS: Filesystem,
{
    let mut output = String::new();

    for (i, ch) in line.chars().enumerate() {
        let pos = i + 1; // 1-indexed
        if opts.is_selected(pos) {
            output.push(ch);
        }
    }

    env.stdout.write_line(&output)
}

/// Cut by field positions.
fn cut_fields<SI, SO, SE, FS>(
    env: &Environment<SI, SO, SE, FS>,
    line: &str,
    opts: &CutOptions,
) -> Result<(), Error>
where
    SI: Stdin,
    SO: Stdout,
    SE: Stderr,
    FS: Filesystem,
{
    // Check if line contains delimiter
    let has_delim = if opts.whitespace_delim {
        line.chars().any(|c| c == ' ' || c == '\t')
    } else {
        line.contains(opts.delimiter)
    };

    if !has_delim {
        // No delimiter found
        if opts.suppress_no_delim {
            // Don't output anything
            return Ok(());
        } else {
            // Output the whole line unchanged
            return env.stdout.write_line(line);
        }
    }

    // Split into fields
    let fields: Vec<&str> = if opts.whitespace_delim {
        // Whitespace mode: consecutive whitespace counts as single separator
        line.split_whitespace().collect()
    } else {
        line.split(opts.delimiter).collect()
    };

    // Collect selected fields
    let mut output_parts: Vec<&str> = Vec::new();
    for (i, field) in fields.iter().enumerate() {
        let pos = i + 1; // 1-indexed
        if opts.is_selected(pos) {
            output_parts.push(field);
        }
    }

    // Join with delimiter (use tab for whitespace mode output)
    let output_delim = if opts.whitespace_delim {
        '\t'
    } else {
        opts.delimiter
    };
    let output: String = output_parts.join(&output_delim.to_string());

    env.stdout.write_line(&output)
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
    // Basic byte mode tests (-b)
    // ========================================================================

    #[test]
    fn bytes_single_position() {
        let env = make_env(vec!["cut", "-b", "1"], "hello");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("h\n", env.stdout.into_string());
    }

    #[test]
    fn bytes_multiple_positions() {
        let env = make_env(vec!["cut", "-b", "1,3,5"], "hello");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("hlo\n", env.stdout.into_string());
    }

    #[test]
    fn bytes_range() {
        let env = make_env(vec!["cut", "-b", "2-4"], "hello");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("ell\n", env.stdout.into_string());
    }

    #[test]
    fn bytes_range_from_start() {
        let env = make_env(vec!["cut", "-b", "-3"], "hello");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("hel\n", env.stdout.into_string());
    }

    #[test]
    fn bytes_range_to_end() {
        let env = make_env(vec!["cut", "-b", "3-"], "hello");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("llo\n", env.stdout.into_string());
    }

    #[test]
    fn bytes_mixed_ranges() {
        let env = make_env(vec!["cut", "-b", "1,3-4"], "hello");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("hll\n", env.stdout.into_string());
    }

    #[test]
    fn bytes_beyond_line_length() {
        let env = make_env(vec!["cut", "-b", "1,10"], "hello");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("h\n", env.stdout.into_string());
    }

    #[test]
    fn bytes_multiple_lines() {
        let env = make_env(vec!["cut", "-b", "1-3"], "hello\nworld");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("hel\nwor\n", env.stdout.into_string());
    }

    // ========================================================================
    // Character mode tests (-c)
    // ========================================================================

    #[test]
    fn chars_single_position() {
        let env = make_env(vec!["cut", "-c", "1"], "hello");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("h\n", env.stdout.into_string());
    }

    #[test]
    fn chars_range() {
        let env = make_env(vec!["cut", "-c", "1-3"], "hello");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("hel\n", env.stdout.into_string());
    }

    #[test]
    fn chars_unicode() {
        // Test with multi-byte UTF-8 characters
        let env = make_env(vec!["cut", "-c", "1-3"], "héllo");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("hél\n", env.stdout.into_string());
    }

    #[test]
    fn chars_unicode_range_to_end() {
        let env = make_env(vec!["cut", "-c", "2-"], "日本語");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("本語\n", env.stdout.into_string());
    }

    // ========================================================================
    // Field mode tests (-f)
    // ========================================================================

    #[test]
    fn fields_single_field() {
        let env = make_env(vec!["cut", "-f", "1"], "a\tb\tc");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("a\n", env.stdout.into_string());
    }

    #[test]
    fn fields_multiple_fields() {
        let env = make_env(vec!["cut", "-f", "1,3"], "a\tb\tc");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("a\tc\n", env.stdout.into_string());
    }

    #[test]
    fn fields_range() {
        let env = make_env(vec!["cut", "-f", "2-3"], "a\tb\tc\td");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("b\tc\n", env.stdout.into_string());
    }

    #[test]
    fn fields_custom_delimiter() {
        let env = make_env(vec!["cut", "-f", "1,3", "-d", ":"], "a:b:c");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("a:c\n", env.stdout.into_string());
    }

    #[test]
    fn fields_no_delimiter_passthrough() {
        let env = make_env(vec!["cut", "-f", "1"], "no tabs here");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("no tabs here\n", env.stdout.into_string());
    }

    #[test]
    fn fields_no_delimiter_suppress() {
        let env = make_env(vec!["cut", "-f", "1", "-s"], "no tabs here");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("", env.stdout.into_string());
    }

    #[test]
    fn fields_range_to_end() {
        let env = make_env(vec!["cut", "-f", "2-"], "a\tb\tc\td");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("b\tc\td\n", env.stdout.into_string());
    }

    #[test]
    fn fields_passwd_example() {
        // Classic /etc/passwd example: extract login and shell
        let env = make_env(
            vec!["cut", "-d", ":", "-f", "1,7"],
            "root:x:0:0:root:/root:/bin/bash",
        );
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("root:/bin/bash\n", env.stdout.into_string());
    }

    // ========================================================================
    // Whitespace delimiter mode tests (-w)
    // ========================================================================

    #[test]
    fn whitespace_single_field() {
        let env = make_env(vec!["cut", "-f", "2", "-w"], "one   two   three");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("two\n", env.stdout.into_string());
    }

    #[test]
    fn whitespace_multiple_fields() {
        let env = make_env(vec!["cut", "-f", "1,3", "-w"], "one   two   three");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("one\tthree\n", env.stdout.into_string());
    }

    #[test]
    fn whitespace_tabs_and_spaces() {
        let env = make_env(vec!["cut", "-f", "2", "-w"], "one \t two");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("two\n", env.stdout.into_string());
    }

    // ========================================================================
    // File handling tests
    // ========================================================================

    #[test]
    fn file_single() {
        let env = make_env(vec!["cut", "-b", "1-3", "file.txt"], "");
        env.fs.add_file("file.txt", "hello\nworld\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("hel\nwor\n", env.stdout.into_string());
    }

    #[test]
    fn file_not_found() {
        let env = make_env(vec!["cut", "-b", "1", "missing.txt"], "");
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("missing.txt"));
    }

    #[test]
    fn file_multiple() {
        let env = make_env(vec!["cut", "-b", "1", "a.txt", "b.txt"], "");
        env.fs.add_file("a.txt", "aaa\n");
        env.fs.add_file("b.txt", "bbb\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("a\nb\n", env.stdout.into_string());
    }

    #[test]
    fn stdin_dash() {
        let env = make_env(vec!["cut", "-b", "1", "-"], "hello");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("h\n", env.stdout.into_string());
    }

    // ========================================================================
    // Error handling tests
    // ========================================================================

    #[test]
    fn error_no_list() {
        let env = make_env(vec!["cut"], "");
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("must specify"));
    }

    #[test]
    fn error_multiple_modes() {
        let env = make_env(vec!["cut", "-b", "1", "-c", "1"], "");
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("only one type"));
    }

    #[test]
    fn error_zero_in_list() {
        let env = make_env(vec!["cut", "-b", "0"], "");
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("zero"));
    }

    #[test]
    fn error_d_without_f() {
        let env = make_env(vec!["cut", "-b", "1", "-d", ":"], "");
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("-d may only be used with -f"));
    }

    #[test]
    fn error_s_without_f() {
        let env = make_env(vec!["cut", "-b", "1", "-s"], "");
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("-s may only be used with -f"));
    }

    #[test]
    fn error_w_without_f() {
        let env = make_env(vec!["cut", "-b", "1", "-w"], "");
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("-w may only be used with -f"));
    }

    #[test]
    fn error_w_and_d_together() {
        let env = make_env(vec!["cut", "-f", "1", "-w", "-d", ":"], "");
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("-w and -d may not be used together"));
    }

    #[test]
    fn error_bad_delimiter() {
        let env = make_env(vec!["cut", "-f", "1", "-d", "ab"], "");
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("bad delimiter"));
    }

    #[test]
    fn error_empty_delimiter() {
        let env = make_env(vec!["cut", "-f", "1", "-d", ""], "");
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("bad delimiter"));
    }

    // ========================================================================
    // List parsing edge cases
    // ========================================================================

    #[test]
    fn list_overlapping_ranges() {
        let env = make_env(vec!["cut", "-b", "1-3,2-4"], "hello");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        // Should output positions 1,2,3,4 without duplication
        assert_eq!("hell\n", env.stdout.into_string());
    }

    #[test]
    fn list_out_of_order() {
        let env = make_env(vec!["cut", "-b", "3,1"], "hello");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        // Output should be in natural order (position order), not list order
        assert_eq!("hl\n", env.stdout.into_string());
    }

    #[test]
    fn list_repeated_positions() {
        let env = make_env(vec!["cut", "-b", "1,1,1"], "hello");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("h\n", env.stdout.into_string());
    }

    // ========================================================================
    // Empty input tests
    // ========================================================================

    #[test]
    fn empty_stdin() {
        let env = make_env(vec!["cut", "-b", "1"], "");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("", env.stdout.into_string());
    }

    #[test]
    fn empty_line() {
        let env = make_env(vec!["cut", "-b", "1"], "\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("\n", env.stdout.into_string());
    }

    #[test]
    fn empty_file() {
        let env = make_env(vec!["cut", "-b", "1", "empty.txt"], "");
        env.fs.add_file("empty.txt", "");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("", env.stdout.into_string());
    }

    // ========================================================================
    // Real-world usage examples
    // ========================================================================

    #[test]
    fn who_output_example() {
        // Simulating: who | cut -c 1-16,26-38
        let env = make_env(
            vec!["cut", "-c", "1-16,26-38"],
            "alice           pts/0        2024-01-15 09:00",
        );
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let output = env.stdout.into_string();
        println!("output: {:?}", output);
        assert!(output.starts_with("alice"));
    }

    #[test]
    fn csv_extraction() {
        let env = make_env(
            vec!["cut", "-d", ",", "-f", "1,3"],
            "name,age,city\nalice,30,london\nbob,25,paris",
        );
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!(
            "name,city\nalice,london\nbob,paris\n",
            env.stdout.into_string()
        );
    }
}

//! The paste utility: merge corresponding or subsequent lines of files.

use getopts::Options;

use crate::{Environment, Error, ExitCode, Filesystem, Stderr, Stdin, Stdout};

/// Options for the paste command.
#[derive(Clone, Debug)]
struct PasteOptions {
    /// The delimiter characters to use between fields.
    /// Defaults to a single tab.
    delimiters: Vec<char>,
    /// Sequential mode (-s): concatenate all lines of each file.
    sequential: bool,
}

impl Default for PasteOptions {
    fn default() -> Self {
        Self {
            delimiters: vec!['\t'],
            sequential: false,
        }
    }
}

/// Parse escape sequences in delimiter list.
/// Supports: \n (newline), \t (tab), \0 (empty string marker), \\ (backslash).
/// Any other backslash-char is treated as just the char.
/// Returns the parsed delimiters. Note: \0 becomes a NUL character which we
/// treat specially during output as "no delimiter".
fn parse_delimiters(s: &str) -> Result<Vec<char>, String> {
    let mut result = Vec::new();
    let mut chars = s.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '\\' {
            match chars.next() {
                Some('n') => result.push('\n'),
                Some('t') => result.push('\t'),
                Some('0') => result.push('\0'), // Empty string marker
                Some('\\') => result.push('\\'),
                Some(c) => result.push(c),
                None => result.push('\\'),
            }
        } else {
            result.push(ch);
        }
    }

    if result.is_empty() {
        return Err("no delimiters specified".to_string());
    }

    Ok(result)
}

fn build_options() -> Options {
    let mut opts = Options::new();
    opts.optopt("d", "", "Use characters from list instead of tab", "LIST");
    opts.optflag(
        "s",
        "",
        "Concatenate all lines of each file in command line order",
    );
    opts
}

/// The paste builtin: merge corresponding or subsequent lines of files.
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
            env.stderr.write_line(&format!("paste: {}", e))?;
            return Ok(ExitCode::from(1));
        }
    };

    let mut paste_opts = PasteOptions::default();

    // Parse -d option
    if let Some(d) = matches.opt_str("d") {
        match parse_delimiters(&d) {
            Ok(delims) => paste_opts.delimiters = delims,
            Err(e) => {
                env.stderr.write_line(&format!("paste: {}", e))?;
                return Ok(ExitCode::from(1));
            }
        }
    }

    // Parse -s flag
    paste_opts.sequential = matches.opt_present("s");

    // Need at least one file
    if matches.free.is_empty() {
        env.stderr.write_line("paste: missing operand")?;
        return Ok(ExitCode::from(1));
    }

    if paste_opts.sequential {
        sequential(env, &matches.free, &paste_opts)
    } else {
        parallel(env, &matches.free, &paste_opts)
    }
}

/// A source of lines (either stdin or a file's contents).
struct LineSource {
    lines: Vec<String>,
    index: usize,
    exhausted: bool,
}

impl LineSource {
    fn from_lines(lines: Vec<String>) -> Self {
        Self {
            lines,
            index: 0,
            exhausted: false,
        }
    }

    fn next_line(&mut self) -> Option<&str> {
        if self.index < self.lines.len() {
            let line = &self.lines[self.index];
            self.index += 1;
            Some(line)
        } else {
            self.exhausted = true;
            None
        }
    }

    fn is_exhausted(&self) -> bool {
        self.exhausted
    }
}

/// Parallel mode: merge corresponding lines from all files.
/// When stdin ('-') is used multiple times, it is read circularly - one line per '-' per row.
fn parallel<SI, SO, SE, FS>(
    env: &Environment<SI, SO, SE, FS>,
    files: &[String],
    opts: &PasteOptions,
) -> Result<ExitCode, Error>
where
    SI: Stdin,
    SO: Stdout,
    SE: Stderr,
    FS: Filesystem,
{
    // Build line sources for each file
    // For stdin ('-'), we use None as a marker and read from shared stdin
    let mut sources: Vec<Option<LineSource>> = Vec::new();
    let mut has_stdin = false;

    for file in files.iter() {
        if file == "-" {
            has_stdin = true;
            sources.push(None); // Marker for stdin
        } else {
            match env.fs.read_to_string(file) {
                Ok(contents) => {
                    let lines: Vec<String> = contents.lines().map(|s| s.to_string()).collect();
                    sources.push(Some(LineSource::from_lines(lines)));
                }
                Err(Error::Io(e)) => {
                    env.stderr.write_line(&format!("paste: {}: {}", file, e))?;
                    return Ok(ExitCode::from(1));
                }
                Err(_) => {
                    return Ok(ExitCode::from(1));
                }
            }
        }
    }

    // Read all stdin lines upfront if we have stdin references
    let mut stdin_lines: Vec<String> = Vec::new();
    let mut stdin_index = 0;
    let mut stdin_exhausted = false;

    if has_stdin {
        while let Some(line) = env.stdin.read_line()? {
            stdin_lines.push(line);
        }
        stdin_exhausted = stdin_lines.is_empty();
    }

    // Process lines
    loop {
        let mut any_output = false;
        let mut output = String::new();
        let mut delim_index = 0;

        for (i, source) in sources.iter_mut().enumerate() {
            // Add delimiter before all columns except the first
            if i > 0 {
                let delim = opts.delimiters[delim_index % opts.delimiters.len()];
                if delim != '\0' {
                    output.push(delim);
                }
                delim_index += 1;
            }

            // Get the line from appropriate source
            let line: Option<&str> = if source.is_none() {
                // Stdin source - read next line circularly
                if stdin_index < stdin_lines.len() {
                    let line = &stdin_lines[stdin_index];
                    stdin_index += 1;
                    Some(line.as_str())
                } else {
                    stdin_exhausted = true;
                    None
                }
            } else {
                source.as_mut().and_then(|s| s.next_line())
            };

            if let Some(line) = line {
                any_output = true;
                output.push_str(line);
            }
        }

        // Check if all sources are exhausted
        let all_exhausted = sources.iter().all(|s| match s {
            None => stdin_exhausted,
            Some(src) => src.is_exhausted(),
        });

        // Output the row if we got any content, or if not all sources are exhausted
        // (meaning some files still have data even if others are empty)
        if any_output {
            env.stdout.write_line(&output)?;
        }

        if all_exhausted {
            break;
        }
    }

    Ok(ExitCode::from(0))
}

/// Sequential mode: concatenate all lines of each file.
fn sequential<SI, SO, SE, FS>(
    env: &Environment<SI, SO, SE, FS>,
    files: &[String],
    opts: &PasteOptions,
) -> Result<ExitCode, Error>
where
    SI: Stdin,
    SO: Stdout,
    SE: Stderr,
    FS: Filesystem,
{
    let mut exit_code: i8 = 0;

    for file in files {
        let lines: Vec<String> = if file == "-" {
            let mut lines = Vec::new();
            while let Some(line) = env.stdin.read_line()? {
                lines.push(line);
            }
            lines
        } else {
            match env.fs.read_to_string(file) {
                Ok(contents) => contents.lines().map(|s| s.to_string()).collect(),
                Err(Error::Io(e)) => {
                    env.stderr.write_line(&format!("paste: {}: {}", file, e))?;
                    exit_code = 1;
                    continue;
                }
                Err(_) => {
                    exit_code = 1;
                    continue;
                }
            }
        };

        if lines.is_empty() {
            continue;
        }

        let mut output = String::new();
        let mut delim_index = 0;

        for (i, line) in lines.iter().enumerate() {
            if i > 0 {
                let delim = opts.delimiters[delim_index % opts.delimiters.len()];
                if delim != '\0' {
                    output.push(delim);
                }
                delim_index += 1;
            }
            output.push_str(line);
        }

        env.stdout.write_line(&output)?;
    }

    Ok(ExitCode::from(exit_code))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::make_test_env_with_stdin;

    // ========================================================================
    // Delimiter parsing tests
    // ========================================================================

    #[test]
    fn parse_delim_simple() {
        let result = parse_delimiters(",").unwrap();
        assert_eq!(vec![','], result);
    }

    #[test]
    fn parse_delim_multiple() {
        let result = parse_delimiters(",:").unwrap();
        assert_eq!(vec![',', ':'], result);
    }

    #[test]
    fn parse_delim_tab_escape() {
        let result = parse_delimiters("\\t").unwrap();
        assert_eq!(vec!['\t'], result);
    }

    #[test]
    fn parse_delim_newline_escape() {
        let result = parse_delimiters("\\n").unwrap();
        assert_eq!(vec!['\n'], result);
    }

    #[test]
    fn parse_delim_null_escape() {
        let result = parse_delimiters("\\0").unwrap();
        assert_eq!(vec!['\0'], result);
    }

    #[test]
    fn parse_delim_backslash_escape() {
        let result = parse_delimiters("\\\\").unwrap();
        assert_eq!(vec!['\\'], result);
    }

    #[test]
    fn parse_delim_mixed() {
        let result = parse_delimiters("\\t\\n").unwrap();
        assert_eq!(vec!['\t', '\n'], result);
    }

    #[test]
    fn parse_delim_unknown_escape() {
        let result = parse_delimiters("\\x").unwrap();
        assert_eq!(vec!['x'], result);
    }

    #[test]
    fn parse_delim_trailing_backslash() {
        let result = parse_delimiters("a\\").unwrap();
        assert_eq!(vec!['a', '\\'], result);
    }

    #[test]
    fn parse_delim_empty_error() {
        let result = parse_delimiters("");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("no delimiters"));
    }

    // ========================================================================
    // Basic parallel mode tests
    // ========================================================================

    #[test]
    fn parallel_two_files() {
        let env = make_test_env_with_stdin(vec!["paste", "a.txt", "b.txt"], "");
        env.fs.add_file("a.txt", "1\n2\n3\n");
        env.fs.add_file("b.txt", "a\nb\nc\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("1\ta\n2\tb\n3\tc\n", env.stdout.into_string());
    }

    #[test]
    fn parallel_three_files() {
        let env = make_test_env_with_stdin(vec!["paste", "a.txt", "b.txt", "c.txt"], "");
        env.fs.add_file("a.txt", "1\n2\n");
        env.fs.add_file("b.txt", "a\nb\n");
        env.fs.add_file("c.txt", "x\ny\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("1\ta\tx\n2\tb\ty\n", env.stdout.into_string());
    }

    #[test]
    fn parallel_unequal_lengths() {
        let env = make_test_env_with_stdin(vec!["paste", "a.txt", "b.txt"], "");
        env.fs.add_file("a.txt", "1\n2\n3\n");
        env.fs.add_file("b.txt", "a\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("1\ta\n2\t\n3\t\n", env.stdout.into_string());
    }

    #[test]
    fn parallel_custom_delimiter() {
        let env = make_test_env_with_stdin(vec!["paste", "-d", ":", "a.txt", "b.txt"], "");
        env.fs.add_file("a.txt", "1\n2\n");
        env.fs.add_file("b.txt", "a\nb\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("1:a\n2:b\n", env.stdout.into_string());
    }

    #[test]
    fn parallel_cycling_delimiters() {
        let env =
            make_test_env_with_stdin(vec!["paste", "-d", ",:", "a.txt", "b.txt", "c.txt"], "");
        env.fs.add_file("a.txt", "1\n2\n");
        env.fs.add_file("b.txt", "a\nb\n");
        env.fs.add_file("c.txt", "x\ny\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("1,a:x\n2,b:y\n", env.stdout.into_string());
    }

    #[test]
    fn parallel_stdin_single() {
        let env = make_test_env_with_stdin(vec!["paste", "-"], "line1\nline2\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("line1\nline2\n", env.stdout.into_string());
    }

    #[test]
    fn parallel_stdin_dash_with_file() {
        let env = make_test_env_with_stdin(vec!["paste", "-", "b.txt"], "1\n2\n");
        env.fs.add_file("b.txt", "a\nb\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("1\ta\n2\tb\n", env.stdout.into_string());
    }

    // ========================================================================
    // Sequential mode tests (-s)
    // ========================================================================

    #[test]
    fn sequential_single_file() {
        let env = make_test_env_with_stdin(vec!["paste", "-s", "a.txt"], "");
        env.fs.add_file("a.txt", "1\n2\n3\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("1\t2\t3\n", env.stdout.into_string());
    }

    #[test]
    fn sequential_two_files() {
        let env = make_test_env_with_stdin(vec!["paste", "-s", "a.txt", "b.txt"], "");
        env.fs.add_file("a.txt", "1\n2\n3\n");
        env.fs.add_file("b.txt", "a\nb\nc\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("1\t2\t3\na\tb\tc\n", env.stdout.into_string());
    }

    #[test]
    fn sequential_custom_delimiter() {
        let env = make_test_env_with_stdin(vec!["paste", "-s", "-d", ",", "a.txt"], "");
        env.fs.add_file("a.txt", "1\n2\n3\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("1,2,3\n", env.stdout.into_string());
    }

    #[test]
    fn sequential_cycling_delimiters() {
        let env = make_test_env_with_stdin(vec!["paste", "-s", "-d", "\\t\\n", "a.txt"], "");
        env.fs.add_file("a.txt", "1\n2\n3\n4\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("1\t2\n3\t4\n", env.stdout.into_string());
    }

    #[test]
    fn sequential_stdin() {
        let env = make_test_env_with_stdin(vec!["paste", "-s", "-"], "a\nb\nc\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("a\tb\tc\n", env.stdout.into_string());
    }

    // ========================================================================
    // Empty string delimiter (\0) tests
    // ========================================================================

    #[test]
    fn null_delimiter_parallel() {
        let env = make_test_env_with_stdin(vec!["paste", "-d", "\\0", "a.txt", "b.txt"], "");
        env.fs.add_file("a.txt", "foo\n");
        env.fs.add_file("b.txt", "bar\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("foobar\n", env.stdout.into_string());
    }

    #[test]
    fn null_delimiter_sequential() {
        let env = make_test_env_with_stdin(vec!["paste", "-s", "-d", "\\0", "a.txt"], "");
        env.fs.add_file("a.txt", "foo\nbar\nbaz\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("foobarbaz\n", env.stdout.into_string());
    }

    // ========================================================================
    // Error handling tests
    // ========================================================================

    #[test]
    fn error_no_files() {
        let env = make_test_env_with_stdin(vec!["paste"], "");
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("missing operand"));
    }

    #[test]
    fn error_file_not_found() {
        let env = make_test_env_with_stdin(vec!["paste", "missing.txt"], "");
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("missing.txt"));
    }

    #[test]
    fn error_file_not_found_sequential_continues() {
        let env = make_test_env_with_stdin(vec!["paste", "-s", "missing.txt", "good.txt"], "");
        env.fs.add_file("good.txt", "ok\n");
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("missing.txt"));
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert_eq!("ok\n", stdout);
    }

    // ========================================================================
    // Empty file tests
    // ========================================================================

    #[test]
    fn empty_file_parallel() {
        let env = make_test_env_with_stdin(vec!["paste", "a.txt", "b.txt"], "");
        env.fs.add_file("a.txt", "");
        env.fs.add_file("b.txt", "x\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("\tx\n", env.stdout.into_string());
    }

    #[test]
    fn empty_file_sequential() {
        let env = make_test_env_with_stdin(vec!["paste", "-s", "a.txt"], "");
        env.fs.add_file("a.txt", "");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("", env.stdout.into_string());
    }

    #[test]
    fn single_line_no_trailing_newline() {
        let env = make_test_env_with_stdin(vec!["paste", "a.txt"], "");
        env.fs.add_file("a.txt", "hello");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("hello\n", env.stdout.into_string());
    }

    // ========================================================================
    // Man page example tests
    // ========================================================================

    #[test]
    fn manpage_example_ls_three_columns() {
        // ls | paste - - -
        let env = make_test_env_with_stdin(
            vec!["paste", "-", "-", "-"],
            "file1\nfile2\nfile3\nfile4\nfile5\n",
        );
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert_eq!("file1\tfile2\tfile3\nfile4\tfile5\t\n", stdout);
    }

    #[test]
    fn manpage_example_combine_pairs() {
        // paste -s -d '\t\n' myfile
        let env = make_test_env_with_stdin(vec!["paste", "-s", "-d", "\\t\\n", "myfile"], "");
        env.fs.add_file("myfile", "line1\nline2\nline3\nline4\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("line1\tline2\nline3\tline4\n", env.stdout.into_string());
    }

    #[test]
    fn manpage_example_colon_list() {
        // find / -name bin -type d | paste -s -d : -
        let env = make_test_env_with_stdin(
            vec!["paste", "-s", "-d", ":", "-"],
            "/bin\n/usr/bin\n/usr/local/bin\n",
        );
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("/bin:/usr/bin:/usr/local/bin\n", env.stdout.into_string());
    }
}

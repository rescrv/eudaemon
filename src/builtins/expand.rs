//! The expand utility converts tabs to spaces.

use getopts::Options;

use crate::{Environment, Error, ExitCode, Filesystem, Stderr, Stdin, Stdout};

/// Tab stops configuration.
#[derive(Clone, Debug)]
pub enum TabStops {
    /// Default: tab stops every 8 columns.
    Default,
    /// Single interval: tab stops at every N columns.
    Interval(usize),
    /// Explicit list of tab stop positions (1-indexed column positions).
    List(Vec<usize>),
}

impl TabStops {
    /// Parse a tab stop specification string.
    ///
    /// Accepts:
    /// - A single number (interval mode): tab stops every N columns
    /// - Comma or space-separated list of numbers (explicit mode): tab stops at those columns
    pub fn parse(spec: &str) -> Result<Self, String> {
        let mut stops = Vec::new();
        let mut current = String::new();

        for ch in spec.chars() {
            if ch.is_ascii_digit() {
                current.push(ch);
            } else if ch == ',' || ch == ' ' {
                if !current.is_empty() {
                    let n: usize = current
                        .parse()
                        .map_err(|_| "bad tab stop spec".to_string())?;
                    if n == 0 {
                        return Err("bad tab stop spec".to_string());
                    }
                    if let Some(&last) = stops.last()
                        && n <= last
                    {
                        return Err("bad tab stop spec".to_string());
                    }
                    stops.push(n);
                    current.clear();
                }
            } else {
                return Err("bad tab stop spec".to_string());
            }
        }

        if !current.is_empty() {
            let n: usize = current
                .parse()
                .map_err(|_| "bad tab stop spec".to_string())?;
            if n == 0 {
                return Err("bad tab stop spec".to_string());
            }
            if let Some(&last) = stops.last()
                && n <= last
            {
                return Err("bad tab stop spec".to_string());
            }
            stops.push(n);
        }

        if stops.is_empty() {
            return Err("bad tab stop spec".to_string());
        }

        if stops.len() == 1 {
            Ok(TabStops::Interval(stops[0]))
        } else {
            Ok(TabStops::List(stops))
        }
    }

    /// Calculate how many spaces to emit for a tab at the given column position.
    /// Column is 0-indexed.
    pub fn spaces_for_tab(&self, column: usize) -> usize {
        match self {
            TabStops::Default => {
                // Tab stops every 8 columns (0, 8, 16, ...)
                8 - (column % 8)
            }
            TabStops::Interval(n) => {
                // Tab stops every n columns (0, n, 2n, ...)
                n - (column % n)
            }
            TabStops::List(stops) => {
                // Find the next tab stop after the current column
                // Column is 0-indexed, but tab stops are 1-indexed positions
                for &stop in stops {
                    // stop is 1-indexed, so stop-1 is the 0-indexed position
                    if stop > column + 1 {
                        return stop - 1 - column;
                    }
                }
                // Past all tab stops: just emit one space
                1
            }
        }
    }

    /// Find the next tab stop column (0-indexed) from the given column.
    /// Returns None if past all tab stops (for List mode).
    pub fn next_tab_stop(&self, column: usize) -> Option<usize> {
        match self {
            TabStops::Default => {
                // Tab stops every 8 columns (0, 8, 16, ...)
                Some(((column / 8) + 1) * 8)
            }
            TabStops::Interval(n) => {
                // Tab stops every n columns (0, n, 2n, ...)
                Some(((column / n) + 1) * n)
            }
            TabStops::List(stops) => {
                // Find the next tab stop after the current column
                // Column is 0-indexed, but tab stops are 1-indexed positions
                for &stop in stops {
                    if stop > column + 1 {
                        return Some(stop - 1);
                    }
                }
                // Past all tab stops
                None
            }
        }
    }
}

fn build_options() -> Options {
    let mut opts = Options::new();
    opts.optopt(
        "t",
        "",
        "Set tab stops at columns tab1, tab2, ... or every N columns",
        "TABLIST",
    );
    opts
}

/// The expand builtin: expand tabs to spaces.
pub fn bin<SI, SO, SE, FS>(env: &Environment<SI, SO, SE, FS>) -> Result<ExitCode, Error>
where
    SI: Stdin,
    SO: Stdout,
    SE: Stderr,
    FS: Filesystem,
{
    let opts_def = build_options();

    // Handle obsolete -N syntax (e.g., -8 for tab stops every 8)
    let mut args: Vec<String> = Vec::new();
    let mut tab_stops = TabStops::Default;
    let mut found_obsolete = false;

    for (i, arg) in env.args[1..].iter().enumerate() {
        if !found_obsolete
            && arg.starts_with('-')
            && arg.len() > 1
            && arg.chars().nth(1).is_some_and(|c| c.is_ascii_digit())
        {
            // Obsolete syntax: -N
            match TabStops::parse(&arg[1..]) {
                Ok(ts) => {
                    tab_stops = ts;
                    found_obsolete = true;
                }
                Err(e) => {
                    env.stderr.write_line(&format!("expand: {}", e))?;
                    return Ok(ExitCode::from(1));
                }
            }
        } else {
            // Check if remaining args contain -t; if so, reset tab_stops later
            args.push(arg.clone());
            if i == 0 && arg == "-t" {
                found_obsolete = false; // -t will override
            }
        }
    }

    let matches = match opts_def.parse(&args) {
        Ok(m) => m,
        Err(e) => {
            env.stderr.write_line(&format!("expand: {}", e))?;
            return Ok(ExitCode::from(1));
        }
    };

    // -t option overrides obsolete syntax
    if let Some(t_arg) = matches.opt_str("t") {
        match TabStops::parse(&t_arg) {
            Ok(ts) => tab_stops = ts,
            Err(e) => {
                env.stderr.write_line(&format!("expand: {}", e))?;
                return Ok(ExitCode::from(1));
            }
        }
    }

    let mut exit_code: i8 = 0;

    if matches.free.is_empty() {
        expand_stdin(env, &tab_stops)?;
    } else {
        for file in &matches.free {
            if file == "-" {
                expand_stdin(env, &tab_stops)?;
            } else if let Err(code) = expand_file(env, file, &tab_stops) {
                exit_code = code;
            }
        }
    }

    Ok(ExitCode::from(exit_code))
}

fn expand_stdin<SI, SO, SE, FS>(
    env: &Environment<SI, SO, SE, FS>,
    tab_stops: &TabStops,
) -> Result<(), Error>
where
    SI: Stdin,
    SO: Stdout,
    SE: Stderr,
    FS: Filesystem,
{
    while let Some(line) = env.stdin.read_line()? {
        let expanded = expand_line(&line, tab_stops);
        env.stdout.write_line(&expanded)?;
    }
    Ok(())
}

fn expand_file<SI, SO, SE, FS>(
    env: &Environment<SI, SO, SE, FS>,
    path: &str,
    tab_stops: &TabStops,
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
                let expanded = expand_line(line, tab_stops);
                if env.stdout.write_line(&expanded).is_err() {
                    return Err(1);
                }
            }
            Ok(())
        }
        Err(Error::Io(e)) => {
            let _ = env.stderr.write_line(&format!("expand: {}: {}", path, e));
            Err(1)
        }
        Err(_e) => Err(1),
    }
}

/// Expand tabs in a single line.
#[cfg(test)]
pub fn expand_line_for_test(line: &str, tab_stops: &TabStops) -> String {
    expand_line(line, tab_stops)
}

/// Expand tabs in a single line.
fn expand_line(line: &str, tab_stops: &TabStops) -> String {
    let mut output = String::new();
    let mut column: usize = 0;

    for ch in line.chars() {
        match ch {
            '\t' => {
                let spaces = tab_stops.spaces_for_tab(column);
                for _ in 0..spaces {
                    output.push(' ');
                }
                column += spaces;
            }
            '\x08' => {
                // Backspace: preserve and decrement column
                output.push(ch);
                column = column.saturating_sub(1);
            }
            '\n' => {
                output.push(ch);
                column = 0;
            }
            _ => {
                output.push(ch);
                // Wide characters would add more than 1 to column, but for simplicity
                // we treat all printable chars as width 1
                column += 1;
            }
        }
    }

    output
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
    // TabStops::parse tests
    // ========================================================================

    #[test]
    fn parse_single_number() {
        let ts = TabStops::parse("4").unwrap();
        assert!(matches!(ts, TabStops::Interval(4)));
    }

    #[test]
    fn parse_comma_list() {
        let ts = TabStops::parse("4,8,12").unwrap();
        if let TabStops::List(stops) = ts {
            assert_eq!(vec![4, 8, 12], stops);
        } else {
            panic!("Expected TabStops::List");
        }
    }

    #[test]
    fn parse_space_list() {
        let ts = TabStops::parse("4 8 12").unwrap();
        if let TabStops::List(stops) = ts {
            assert_eq!(vec![4, 8, 12], stops);
        } else {
            panic!("Expected TabStops::List");
        }
    }

    #[test]
    fn parse_zero_fails() {
        let result = TabStops::parse("0");
        println!("result: {:?}", result);
        assert!(result.is_err());
    }

    #[test]
    fn parse_non_increasing_fails() {
        let result = TabStops::parse("8,4");
        println!("result: {:?}", result);
        assert!(result.is_err());
    }

    #[test]
    fn parse_duplicate_fails() {
        let result = TabStops::parse("4,4");
        println!("result: {:?}", result);
        assert!(result.is_err());
    }

    #[test]
    fn parse_invalid_char_fails() {
        let result = TabStops::parse("4x8");
        println!("result: {:?}", result);
        assert!(result.is_err());
    }

    #[test]
    fn parse_empty_fails() {
        let result = TabStops::parse("");
        println!("result: {:?}", result);
        assert!(result.is_err());
    }

    // ========================================================================
    // TabStops::spaces_for_tab tests
    // ========================================================================

    #[test]
    fn default_tab_stops_at_column_0() {
        let ts = TabStops::Default;
        assert_eq!(8, ts.spaces_for_tab(0));
    }

    #[test]
    fn default_tab_stops_at_column_1() {
        let ts = TabStops::Default;
        assert_eq!(7, ts.spaces_for_tab(1));
    }

    #[test]
    fn default_tab_stops_at_column_7() {
        let ts = TabStops::Default;
        assert_eq!(1, ts.spaces_for_tab(7));
    }

    #[test]
    fn default_tab_stops_at_column_8() {
        let ts = TabStops::Default;
        assert_eq!(8, ts.spaces_for_tab(8));
    }

    #[test]
    fn interval_4_at_column_0() {
        let ts = TabStops::Interval(4);
        assert_eq!(4, ts.spaces_for_tab(0));
    }

    #[test]
    fn interval_4_at_column_3() {
        let ts = TabStops::Interval(4);
        assert_eq!(1, ts.spaces_for_tab(3));
    }

    #[test]
    fn interval_4_at_column_4() {
        let ts = TabStops::Interval(4);
        assert_eq!(4, ts.spaces_for_tab(4));
    }

    #[test]
    fn list_stops_at_column_0() {
        let ts = TabStops::List(vec![4, 8, 12]);
        // At column 0, next stop is 4 (1-indexed), which is column 3 (0-indexed)
        // spaces = 4 - 1 - 0 = 3
        assert_eq!(3, ts.spaces_for_tab(0));
    }

    #[test]
    fn list_stops_at_column_3() {
        let ts = TabStops::List(vec![4, 8, 12]);
        // At column 3 (0-indexed), current position is 4 (1-indexed)
        // Next stop is 8, spaces = 8 - 1 - 3 = 4
        assert_eq!(4, ts.spaces_for_tab(3));
    }

    #[test]
    fn list_stops_past_all() {
        let ts = TabStops::List(vec![4, 8, 12]);
        // At column 15, past all stops, should return 1
        assert_eq!(1, ts.spaces_for_tab(15));
    }

    // ========================================================================
    // Basic expand tests
    // ========================================================================

    #[test]
    fn no_tabs() {
        let env = make_env(vec!["expand"], "hello world");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("hello world\n", env.stdout.into_string());
    }

    #[test]
    fn single_tab_at_start() {
        let env = make_env(vec!["expand"], "\thello");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("        hello\n", env.stdout.into_string());
    }

    #[test]
    fn single_tab_after_text() {
        let env = make_env(vec!["expand"], "hi\tthere");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        // "hi" is 2 chars, tab goes to column 8, so 6 spaces
        assert_eq!("hi      there\n", env.stdout.into_string());
    }

    #[test]
    fn multiple_tabs() {
        let env = make_env(vec!["expand"], "\t\thello");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        // First tab: 8 spaces, second tab: 8 more spaces
        assert_eq!("                hello\n", env.stdout.into_string());
    }

    #[test]
    fn tab_at_column_7() {
        let env = make_env(vec!["expand"], "1234567\tx");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        // 7 chars, tab adds 1 space to reach column 8
        assert_eq!("1234567 x\n", env.stdout.into_string());
    }

    #[test]
    fn tab_at_column_8() {
        let env = make_env(vec!["expand"], "12345678\tx");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        // 8 chars, tab adds 8 spaces to reach column 16
        assert_eq!("12345678        x\n", env.stdout.into_string());
    }

    // ========================================================================
    // -t option tests
    // ========================================================================

    #[test]
    fn tab_interval_4() {
        let env = make_env(vec!["expand", "-t", "4"], "\thello");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("    hello\n", env.stdout.into_string());
    }

    #[test]
    fn tab_interval_4_after_text() {
        let env = make_env(vec!["expand", "-t", "4"], "ab\tcd");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        // "ab" is 2 chars, tab goes to column 4, so 2 spaces
        assert_eq!("ab  cd\n", env.stdout.into_string());
    }

    #[test]
    fn tab_list() {
        let env = make_env(vec!["expand", "-t", "4,8,12"], "\t\t\tx");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        // Tab 1: column 0 -> 3 (3 spaces to reach position 4)
        // Tab 2: column 3 -> 7 (4 spaces to reach position 8)
        // Tab 3: column 7 -> 11 (4 spaces to reach position 12)
        assert_eq!("           x\n", env.stdout.into_string());
    }

    #[test]
    fn tab_list_past_all_stops() {
        let env = make_env(vec!["expand", "-t", "4,8"], "12345678901234\tx");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        // 14 chars, past all tab stops, just 1 space
        assert_eq!("12345678901234 x\n", env.stdout.into_string());
    }

    // ========================================================================
    // Obsolete -N syntax tests
    // ========================================================================

    #[test]
    fn obsolete_syntax_single() {
        let env = make_env(vec!["expand", "-4"], "\thello");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("    hello\n", env.stdout.into_string());
    }

    #[test]
    fn obsolete_syntax_overridden_by_t() {
        let env = make_env(vec!["expand", "-4", "-t", "2"], "\thello");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        // -t 2 should override -4
        assert_eq!("  hello\n", env.stdout.into_string());
    }

    // ========================================================================
    // Backspace handling tests
    // ========================================================================

    #[test]
    fn backspace_decrements_column() {
        let env = make_env(vec!["expand"], "abc\x08\tx");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        // "abc" = 3 chars, backspace decrements to 2, tab from column 2 = 6 spaces
        assert_eq!("abc\x08      x\n", env.stdout.into_string());
    }

    #[test]
    fn backspace_at_column_0() {
        let env = make_env(vec!["expand"], "\x08\tx");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        // Backspace at column 0 stays at 0, tab = 8 spaces
        assert_eq!("\x08        x\n", env.stdout.into_string());
    }

    // ========================================================================
    // Multiple lines tests
    // ========================================================================

    #[test]
    fn multiple_lines() {
        let env = make_env(vec!["expand"], "\ta\n\tb\n\tc");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!(
            "        a\n        b\n        c\n",
            env.stdout.into_string()
        );
    }

    #[test]
    fn column_resets_per_line() {
        let env = make_env(vec!["expand"], "1234567\tx\n\ty");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        // Line 1: 7 chars + 1 space + x
        // Line 2: column resets, tab = 8 spaces
        assert_eq!("1234567 x\n        y\n", env.stdout.into_string());
    }

    // ========================================================================
    // File handling tests
    // ========================================================================

    #[test]
    fn read_from_file() {
        let env = make_env(vec!["expand", "input.txt"], "");
        env.fs.add_file("input.txt", "\thello\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("        hello\n", env.stdout.into_string());
    }

    #[test]
    fn file_not_found() {
        let env = make_env(vec!["expand", "nonexistent.txt"], "");
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("nonexistent.txt"));
    }

    #[test]
    fn multiple_files() {
        let env = make_env(vec!["expand", "a.txt", "b.txt"], "");
        env.fs.add_file("a.txt", "\tA\n");
        env.fs.add_file("b.txt", "\tB\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("        A\n        B\n", env.stdout.into_string());
    }

    #[test]
    fn dash_means_stdin() {
        let env = make_env(vec!["expand", "-"], "\tfrom stdin");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("        from stdin\n", env.stdout.into_string());
    }

    #[test]
    fn mixed_files_and_stdin() {
        let env = make_env(vec!["expand", "a.txt", "-", "b.txt"], "\tMIDDLE");
        env.fs.add_file("a.txt", "\tFIRST\n");
        env.fs.add_file("b.txt", "\tLAST\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!(
            "        FIRST\n        MIDDLE\n        LAST\n",
            env.stdout.into_string()
        );
    }

    // ========================================================================
    // Error handling tests
    // ========================================================================

    #[test]
    fn bad_tab_stop_spec() {
        let env = make_env(vec!["expand", "-t", "abc"], "");
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("bad tab stop spec"));
    }

    #[test]
    fn bad_tab_stop_zero() {
        let env = make_env(vec!["expand", "-t", "0"], "");
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("bad tab stop spec"));
    }

    #[test]
    fn bad_tab_stop_non_increasing() {
        let env = make_env(vec!["expand", "-t", "8,4"], "");
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("bad tab stop spec"));
    }

    // ========================================================================
    // Edge cases
    // ========================================================================

    #[test]
    fn empty_stdin() {
        let env = make_env(vec!["expand"], "");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("", env.stdout.into_string());
    }

    #[test]
    fn only_tabs() {
        let env = make_env(vec!["expand"], "\t\t\t");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("                        \n", env.stdout.into_string());
    }

    #[test]
    fn tab_interval_1() {
        let env = make_env(vec!["expand", "-t", "1"], "a\tb\tc");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        // With interval 1, every position is a tab stop, so tabs become 1 space
        assert_eq!("a b c\n", env.stdout.into_string());
    }

    #[test]
    fn very_long_interval() {
        let env = make_env(vec!["expand", "-t", "100"], "\tx");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let output = env.stdout.into_string();
        // Should have 100 spaces before x
        assert_eq!(100, output.find('x').unwrap());
    }

    #[test]
    fn multiple_files_one_missing() {
        let env = make_env(vec!["expand", "a.txt", "missing.txt", "b.txt"], "");
        env.fs.add_file("a.txt", "\tA\n");
        env.fs.add_file("b.txt", "\tB\n");
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert!(stdout.contains("        A"));
        assert!(stdout.contains("        B"));
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("missing.txt"));
    }
}

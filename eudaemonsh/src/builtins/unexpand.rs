//! The unexpand utility converts spaces to tabs.

use getopts::Options;

use super::expand::TabStops;
use crate::{Environment, Error, ExitCode, Filesystem, FsError, StdioIn, StdioOut};

fn build_options() -> Options {
    let mut opts = Options::new();
    opts.optflag("a", "", "Convert all blanks, not just leading blanks");
    opts.optopt(
        "t",
        "",
        "Set tab stops at columns tab1, tab2, ... or every N columns (implies -a)",
        "TABLIST",
    );
    opts
}

/// The unexpand builtin: convert spaces to tabs.
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
            env.stderr.write_line(&format!("unexpand: {}", e))?;
            return Ok(ExitCode::from(1));
        }
    };

    let mut all = matches.opt_present("a");
    let tab_stops = if let Some(t_arg) = matches.opt_str("t") {
        all = true; // -t implies -a
        match TabStops::parse(&t_arg) {
            Ok(ts) => ts,
            Err(e) => {
                env.stderr.write_line(&format!("unexpand: {}", e))?;
                return Ok(ExitCode::from(1));
            }
        }
    } else {
        TabStops::Default
    };

    let mut exit_code: i8 = 0;

    if matches.free.is_empty() {
        unexpand_stdin(env, &tab_stops, all)?;
    } else {
        for file in &matches.free {
            if file == "-" {
                unexpand_stdin(env, &tab_stops, all)?;
            } else if let Err(code) = unexpand_file(env, file, &tab_stops, all) {
                exit_code = code;
            }
        }
    }

    Ok(ExitCode::from(exit_code))
}

fn unexpand_stdin<SI, SO, SE, FS>(
    env: &Environment<SI, SO, SE, FS>,
    tab_stops: &TabStops,
    all: bool,
) -> Result<(), Error>
where
    SI: StdioIn,
    SO: StdioOut,
    SE: StdioOut,
    FS: Filesystem,
{
    while let Some(line) = env.stdin.read_line()? {
        let unexpanded = unexpand_line(&line, tab_stops, all);
        env.stdout.write_line(&unexpanded)?;
    }
    Ok(())
}

fn unexpand_file<SI, SO, SE, FS>(
    env: &Environment<SI, SO, SE, FS>,
    path: &str,
    tab_stops: &TabStops,
    all: bool,
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
                let unexpanded = unexpand_line(line, tab_stops, all);
                if env.stdout.write_line(&unexpanded).is_err() {
                    return Err(1);
                }
            }
            Ok(())
        }
        Err(FsError::Io(e)) => {
            let _ = env.stderr.write_line(&format!("unexpand: {}: {}", path, e));
            Err(1)
        }
    }
}

/// Unexpand spaces to tabs in a single line.
///
/// The algorithm tracks two columns:
/// - `ocol`: the output column (what we've actually emitted)
/// - `dcol`: the destination column (where we want to be)
///
/// When we see spaces, we accumulate them by advancing dcol.
/// When we see a non-space (or reach end of leading blanks when !all),
/// we emit tabs + spaces to catch up from ocol to dcol.
fn unexpand_line(line: &str, tab_stops: &TabStops, all: bool) -> String {
    let mut output = String::new();
    let mut ocol: usize = 0; // output column
    let mut dcol: usize = 0; // destination column
    let mut in_leading_blanks = true;

    let chars: Vec<char> = line.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        let ch = chars[i];

        if ch == ' ' && (all || in_leading_blanks) {
            // Accumulate space by advancing destination column
            dcol += 1;
            i += 1;
            continue;
        } else if ch == '\t' && (all || in_leading_blanks) {
            // Tab advances to next tab stop
            if let Some(next_stop) = tab_stops.next_tab_stop(dcol) {
                dcol = next_stop;
            } else {
                // Past all tab stops, treat as single space
                dcol += 1;
            }
            i += 1;
            continue;
        }

        // Non-blank character (or we're past leading blanks and !all)
        // Emit tabs and spaces to catch up from ocol to dcol
        emit_tabs_and_spaces(&mut output, &mut ocol, dcol, tab_stops);

        if ch == '\x08' {
            // Backspace: preserve and decrement columns
            output.push('\x08');
            ocol = ocol.saturating_sub(1);
            dcol = dcol.saturating_sub(1);
        } else if ch == '\n' {
            output.push('\n');
            ocol = 0;
            dcol = 0;
            in_leading_blanks = true;
        } else {
            // Regular character
            output.push(ch);
            ocol += 1;
            dcol += 1;
            if ch != ' ' && ch != '\t' {
                in_leading_blanks = false;
            }
        }

        i += 1;

        // If not processing all blanks and we've hit a non-blank,
        // emit the rest of the line unchanged
        if !all && !in_leading_blanks {
            while i < chars.len() {
                output.push(chars[i]);
                i += 1;
            }
        }
    }

    // Handle any trailing accumulated blanks
    emit_tabs_and_spaces(&mut output, &mut ocol, dcol, tab_stops);

    output
}

/// Emit tabs and spaces to move from ocol to dcol.
fn emit_tabs_and_spaces(output: &mut String, ocol: &mut usize, dcol: usize, tab_stops: &TabStops) {
    // Emit maximum tabs first
    while let Some(next_stop) = tab_stops.next_tab_stop(*ocol) {
        if next_stop <= dcol && next_stop - *ocol >= 2 {
            // Only emit tab if it saves at least one character (tab replaces 2+ spaces)
            output.push('\t');
            *ocol = next_stop;
        } else {
            break;
        }
    }

    // Emit remaining spaces
    while *ocol < dcol {
        output.push(' ');
        *ocol += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TestFilesystemExt;
    use crate::test_utils::make_test_env_with_stdin;

    // ========================================================================
    // Basic unexpand tests (default: leading blanks only)
    // ========================================================================

    #[test]
    fn no_spaces() {
        let env = make_test_env_with_stdin(vec!["unexpand"], "hello world");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("hello world\n", env.stdout.into_string());
    }

    #[test]
    fn eight_leading_spaces_become_tab() {
        let env = make_test_env_with_stdin(vec!["unexpand"], "        hello");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("\thello\n", env.stdout.into_string());
    }

    #[test]
    fn sixteen_leading_spaces_become_two_tabs() {
        let env = make_test_env_with_stdin(vec!["unexpand"], "                hello");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("\t\thello\n", env.stdout.into_string());
    }

    #[test]
    fn less_than_eight_spaces_remain_spaces() {
        let env = make_test_env_with_stdin(vec!["unexpand"], "       hello");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        // 7 spaces is less than 8, but can still become a tab (7 spaces < 8 column tab stop)
        // Actually: 7 spaces from column 0, next tab stop is at 8
        // 8 - 0 = 8 spaces needed for tab, but we only have 7
        // So 7 spaces stay as 7 spaces? No wait, we should still emit a tab
        // because the tab would take us to column 8, and we need to be at column 7.
        // So we can't use a tab here.
        assert_eq!("       hello\n", env.stdout.into_string());
    }

    #[test]
    fn nine_leading_spaces_become_tab_plus_space() {
        let env = make_test_env_with_stdin(vec!["unexpand"], "         hello");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        // 9 spaces: tab to column 8, then 1 space
        assert_eq!("\t hello\n", env.stdout.into_string());
    }

    #[test]
    fn internal_spaces_not_converted_by_default() {
        let env = make_test_env_with_stdin(vec!["unexpand"], "hello        world");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        // Internal spaces remain unchanged when -a not specified
        assert_eq!("hello        world\n", env.stdout.into_string());
    }

    #[test]
    fn leading_spaces_converted_internal_preserved() {
        let env = make_test_env_with_stdin(vec!["unexpand"], "        hello        world");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        // Leading 8 spaces become tab, internal spaces preserved
        assert_eq!("\thello        world\n", env.stdout.into_string());
    }

    // ========================================================================
    // -a flag tests (all spaces)
    // ========================================================================

    #[test]
    fn all_flag_converts_internal_spaces() {
        let env = make_test_env_with_stdin(vec!["unexpand", "-a"], "hello           world");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        // "hello" is 5 chars, then spaces to column 16 (11 spaces)
        // From column 5, next tab stop is 8, then 16
        // 8-5=3 spaces, then tab to 16 covers 8 spaces = tab
        // So: hello + 3 spaces + tab + world? No wait, let me recalculate
        // "hello" = columns 0-4 (5 chars), column 5 is next
        // 11 spaces takes us to column 16
        // From col 5, next tab stop is 8 (saves 3 chars), then 16 (saves 8 chars)
        // Tab from 5->8 saves 3-1=2 chars, tab from 8->16 saves 8-1=7 chars
        // So: hello + tab (to 8) + tab (to 16) + world
        let output = env.stdout.into_string();
        println!("output: {:?}", output);
        assert_eq!("hello\t\tworld\n", output);
    }

    #[test]
    fn all_flag_with_leading_and_internal() {
        let env = make_test_env_with_stdin(vec!["unexpand", "-a"], "        hello        world");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        // Leading 8 spaces -> tab
        // "hello" ends at column 13
        // 8 more spaces to column 21, but next tab stops are 16, 24
        // From 13, tab to 16 saves 3-1=2, but we only have 8 spaces to go to col 21
        // 16-13=3 spaces, then 21-16=5 spaces
        // Tab to 16, then 5 spaces
        let output = env.stdout.into_string();
        println!("output: {:?}", output);
        assert_eq!("\thello\t     world\n", output);
    }

    // ========================================================================
    // -t flag tests (custom tab stops)
    // ========================================================================

    #[test]
    fn custom_tab_interval() {
        let env = make_test_env_with_stdin(vec!["unexpand", "-t", "4"], "    hello");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("\thello\n", env.stdout.into_string());
    }

    #[test]
    fn custom_tab_interval_implies_all() {
        let env = make_test_env_with_stdin(vec!["unexpand", "-t", "4"], "hello    world");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        // "hello" is 5 chars, next tab stop at 8
        // 4 spaces to column 9, tab stop at 8 is already passed
        // next tab stop is 12, but we only need to get to 9
        // So: hello + space + tab(to 8)? No...
        // Actually with -t 4, tab stops are at 4, 8, 12, 16...
        // "hello" = cols 0-4, then at col 5
        // 4 spaces to col 9
        // From col 5, next stop is 8 (saves 3-1=2), then we need 1 more space to reach 9
        let output = env.stdout.into_string();
        println!("output: {:?}", output);
        assert_eq!("hello\t world\n", output);
    }

    #[test]
    fn custom_tab_list() {
        let env = make_test_env_with_stdin(vec!["unexpand", "-t", "4,8,12"], "            x");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        // 12 spaces, tab stops at 4, 8, 12
        // Tab to 4, tab to 8, tab to 12
        // But wait, tab stops are 1-indexed in the list!
        // So stops at columns 3, 7, 11 (0-indexed)
        // 12 spaces takes us to column 12
        // Tab to col 3, tab to col 7, tab to col 11, then 1 space to col 12
        let output = env.stdout.into_string();
        println!("output: {:?}", output);
        assert_eq!("\t\t\t x\n", output);
    }

    // ========================================================================
    // Tab input handling
    // ========================================================================

    #[test]
    fn input_tab_preserved() {
        let env = make_test_env_with_stdin(vec!["unexpand"], "\thello");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        // Input tab should be preserved as tab
        assert_eq!("\thello\n", env.stdout.into_string());
    }

    #[test]
    fn mixed_tabs_and_spaces() {
        let env = make_test_env_with_stdin(vec!["unexpand"], "\t    hello");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        // Tab to column 8, then 4 spaces to column 12
        // From col 8, next tab stop is 16, but we only need col 12
        // Can't use tab (would overshoot), so 4 spaces remain
        let output = env.stdout.into_string();
        println!("output: {:?}", output);
        assert_eq!("\t    hello\n", output);
    }

    // ========================================================================
    // Multiple lines tests
    // ========================================================================

    #[test]
    fn multiple_lines() {
        let env = make_test_env_with_stdin(vec!["unexpand"], "        a\n        b\n        c");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("\ta\n\tb\n\tc\n", env.stdout.into_string());
    }

    #[test]
    fn column_resets_per_line() {
        let env = make_test_env_with_stdin(vec!["unexpand"], "hello        world\n        x");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        // Line 1: no leading spaces, internal spaces preserved
        // Line 2: 8 leading spaces become tab
        assert_eq!("hello        world\n\tx\n", env.stdout.into_string());
    }

    // ========================================================================
    // File handling tests
    // ========================================================================

    #[test]
    fn read_from_file() {
        let env = make_test_env_with_stdin(vec!["unexpand", "input.txt"], "");
        env.fs.add_file("input.txt", "        hello\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("\thello\n", env.stdout.into_string());
    }

    #[test]
    fn file_not_found() {
        let env = make_test_env_with_stdin(vec!["unexpand", "nonexistent.txt"], "");
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("nonexistent.txt"));
    }

    #[test]
    fn multiple_files() {
        let env = make_test_env_with_stdin(vec!["unexpand", "a.txt", "b.txt"], "");
        env.fs.add_file("a.txt", "        A\n");
        env.fs.add_file("b.txt", "        B\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("\tA\n\tB\n", env.stdout.into_string());
    }

    #[test]
    fn dash_means_stdin() {
        let env = make_test_env_with_stdin(vec!["unexpand", "-"], "        from stdin");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("\tfrom stdin\n", env.stdout.into_string());
    }

    // ========================================================================
    // Error handling tests
    // ========================================================================

    #[test]
    fn bad_tab_stop_spec() {
        let env = make_test_env_with_stdin(vec!["unexpand", "-t", "abc"], "");
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
        let env = make_test_env_with_stdin(vec!["unexpand"], "");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("", env.stdout.into_string());
    }

    #[test]
    fn only_spaces() {
        let env = make_test_env_with_stdin(vec!["unexpand"], "                        ");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        // 24 spaces = 3 tabs
        assert_eq!("\t\t\t\n", env.stdout.into_string());
    }

    #[test]
    fn single_space_not_converted() {
        let env = make_test_env_with_stdin(vec!["unexpand"], " hello");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        // Single space can't be converted to tab (would need 8 spaces)
        assert_eq!(" hello\n", env.stdout.into_string());
    }

    #[test]
    fn two_spaces_not_converted_to_tab() {
        // Per BSD behavior, tab only replaces 2+ characters
        // But 2 spaces at column 0 don't reach column 8
        let env = make_test_env_with_stdin(vec!["unexpand"], "  hello");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("  hello\n", env.stdout.into_string());
    }

    #[test]
    fn backspace_handling() {
        let env = make_test_env_with_stdin(vec!["unexpand"], "abc\x08        x");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        // "abc" = cols 0-2, backspace decrements to col 2
        // Then we're not in leading blanks, so spaces are preserved
        let output = env.stdout.into_string();
        println!("output: {:?}", output);
        assert_eq!("abc\x08        x\n", output);
    }

    #[test]
    fn roundtrip_expand_unexpand() {
        // expand then unexpand should give back tabs
        let input = "\t\thello\t\tworld";
        let tab_stops = TabStops::Default;

        // First expand: tabs to spaces
        let expanded = super::super::expand::expand_line_for_test(input, &tab_stops);
        println!("expanded: {:?}", expanded);

        // Then unexpand with -a: spaces back to tabs
        let unexpanded = unexpand_line(&expanded, &tab_stops, true);
        println!("unexpanded: {:?}", unexpanded);

        assert_eq!(input, unexpanded);
    }
}

use getopts::Options;

use crate::{Environment, Error, ExitCode, Filesystem, FsError, Stderr, Stdin, Stdout};

/// Options for the cat command.
#[derive(Clone, Copy, Debug, Default)]
struct CatOptions {
    /// -b: Number non-blank output lines.
    number_nonblank: bool,
    /// -e: Display non-printing characters and $ at end of each line.
    show_ends: bool,
    /// -l: Set an exclusive advisory lock (no-op in mock).
    #[allow(dead_code)]
    lock: bool,
    /// -n: Number all output lines.
    number: bool,
    /// -s: Squeeze multiple adjacent empty lines.
    squeeze_blank: bool,
    /// -t: Display non-printing characters and tabs as ^I.
    show_tabs: bool,
    /// -u: Disable output buffering (no-op).
    #[allow(dead_code)]
    unbuffered: bool,
    /// -v: Display non-printing characters visibly.
    show_nonprinting: bool,
}

/// State for processing cat output.
struct CatState {
    line_number: usize,
    prev_blank: bool,
}

impl CatState {
    fn new() -> Self {
        Self {
            line_number: 0,
            prev_blank: false,
        }
    }
}

fn build_options() -> Options {
    let mut opts = Options::new();
    opts.optflag("b", "", "Number the non-blank output lines, starting at 1.");
    opts.optflag(
        "e",
        "",
        "Display non-printing characters and $ at end of each line.",
    );
    opts.optflag(
        "l",
        "",
        "Set an exclusive advisory lock on the standard output.",
    );
    opts.optflag("n", "", "Number the output lines, starting at 1.");
    opts.optflag(
        "s",
        "",
        "Squeeze multiple adjacent empty lines, causing single spacing.",
    );
    opts.optflag("t", "", "Display non-printing characters and tabs as ^I.");
    opts.optflag("u", "", "Disable output buffering.");
    opts.optflag(
        "v",
        "",
        "Display non-printing characters so they are visible.",
    );
    opts
}

/// The cat builtin: concatenate and print files or stdin.
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
            env.stderr.write_line(&format!("cat: {}", e))?;
            return Ok(ExitCode::from(1));
        }
    };

    let show_ends = matches.opt_present("e");
    let show_tabs = matches.opt_present("t");
    // -e and -t imply -v
    let show_nonprinting = matches.opt_present("v") || show_ends || show_tabs;

    let cat_opts = CatOptions {
        number_nonblank: matches.opt_present("b"),
        show_ends,
        lock: matches.opt_present("l"),
        number: matches.opt_present("n"),
        squeeze_blank: matches.opt_present("s"),
        show_tabs,
        unbuffered: matches.opt_present("u"),
        show_nonprinting,
    };

    let mut state = CatState::new();
    let mut exit_code: i8 = 0;

    if matches.free.is_empty() {
        // Read from stdin
        cat_stdin(env, &cat_opts, &mut state)?;
    } else {
        for file in &matches.free {
            if file == "-" {
                cat_stdin(env, &cat_opts, &mut state)?;
            } else if let Err(code) = cat_file(env, file, &cat_opts, &mut state) {
                exit_code = code;
            }
        }
    }

    Ok(ExitCode::from(exit_code))
}

fn cat_stdin<SI, SO, SE, FS>(
    env: &Environment<SI, SO, SE, FS>,
    opts: &CatOptions,
    state: &mut CatState,
) -> Result<(), Error>
where
    SI: Stdin,
    SO: Stdout,
    SE: Stderr,
    FS: Filesystem,
{
    while let Some(line) = env.stdin.read_line()? {
        process_line(env, &line, opts, state)?;
    }
    Ok(())
}

fn cat_file<SI, SO, SE, FS>(
    env: &Environment<SI, SO, SE, FS>,
    path: &str,
    opts: &CatOptions,
    state: &mut CatState,
) -> Result<(), i8>
where
    SI: Stdin,
    SO: Stdout,
    SE: Stderr,
    FS: Filesystem,
{
    match env.fs.read_to_string(path) {
        Ok(contents) => {
            // Process line by line
            let lines: Vec<&str> = contents.lines().collect();

            for line in lines.iter() {
                if process_line(env, line, opts, state).is_err() {
                    return Err(1);
                }
            }
            Ok(())
        }
        Err(FsError::Io(e)) => {
            let _ = env.stderr.write_line(&format!("cat: {}: {}", path, e));
            Err(1)
        }
    }
}

/// Process a single line with the given options.
fn process_line<SI, SO, SE, FS>(
    env: &Environment<SI, SO, SE, FS>,
    line: &str,
    opts: &CatOptions,
    state: &mut CatState,
) -> Result<(), Error>
where
    SI: Stdin,
    SO: Stdout,
    SE: Stderr,
    FS: Filesystem,
{
    let is_blank = line.is_empty();

    // Handle -s: squeeze blank lines
    if opts.squeeze_blank && is_blank && state.prev_blank {
        return Ok(());
    }
    state.prev_blank = is_blank;

    // Build the output line
    let mut output = String::new();

    // Handle line numbering
    if opts.number_nonblank {
        // -b: only number non-blank lines
        if !is_blank {
            state.line_number += 1;
            output.push_str(&format!("{:6}\t", state.line_number));
        }
    } else if opts.number {
        // -n: number all lines
        state.line_number += 1;
        output.push_str(&format!("{:6}\t", state.line_number));
    }

    // Process the line content
    if opts.show_nonprinting || opts.show_tabs {
        for ch in line.chars() {
            if opts.show_tabs && ch == '\t' {
                output.push_str("^I");
            } else if opts.show_nonprinting {
                output.push_str(&make_visible(ch));
            } else {
                output.push(ch);
            }
        }
    } else {
        output.push_str(line);
    }

    // Handle -e: show $ at end of line
    if opts.show_ends {
        output.push('$');
    }

    env.stdout.write_line(&output)?;
    Ok(())
}

/// Convert a character to its visible representation for -v option.
fn make_visible(ch: char) -> String {
    let byte = ch as u32;

    if byte > 127 {
        // Non-ASCII: print as M- followed by the character for the low 7 bits
        let low = (byte & 0x7F) as u8;
        if low < 32 {
            // M-^X for meta + control
            format!("M-^{}", (low + 64) as char)
        } else if low == 127 {
            // M-^? for meta + delete
            "M-^?".to_string()
        } else {
            // M-X for meta + printable
            format!("M-{}", low as char)
        }
    } else if byte < 32 {
        // Control characters: ^X
        // Tab (9) is handled separately if -t is set
        if byte == 9 {
            // Tab without -t flag: pass through
            "\t".to_string()
        } else {
            format!("^{}", (byte as u8 + 64) as char)
        }
    } else if byte == 127 {
        // Delete character: ^?
        "^?".to_string()
    } else {
        // Normal printable character
        ch.to_string()
    }
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
    fn empty_stdin() {
        let env = make_test_env_with_stdin(vec!["cat"], "");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("", env.stdout.into_string());
    }

    #[test]
    fn single_line() {
        let env = make_test_env_with_stdin(vec!["cat"], "hello");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("hello\n", env.stdout.into_string());
    }

    #[test]
    fn multiple_lines() {
        let env = make_test_env_with_stdin(vec!["cat"], "line1\nline2\nline3");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("line1\nline2\nline3\n", env.stdout.into_string());
    }

    #[test]
    fn file_not_found() {
        let env = make_test_env_with_stdin(vec!["cat", "nonexistent.txt"], "");
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("nonexistent.txt"));
    }

    #[test]
    fn single_file() {
        let env = make_test_env_with_stdin(vec!["cat", "file.txt"], "");
        env.fs.add_file("file.txt", "hello world\n");
        let result = bin(&env).unwrap();
        println!("stdout: {:?}", env.stdout.into_string());
        println!("stderr: {:?}", env.stderr.into_string());
        assert_eq!(0, result.code());
        assert_eq!("hello world\n", env.stdout.into_string());
    }

    #[test]
    fn multiple_files() {
        let env = make_test_env_with_stdin(vec!["cat", "a.txt", "b.txt"], "");
        env.fs.add_file("a.txt", "aaa\n");
        env.fs.add_file("b.txt", "bbb\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("aaa\nbbb\n", env.stdout.into_string());
    }

    #[test]
    fn dash_means_stdin() {
        let env = make_test_env_with_stdin(vec!["cat", "-"], "from stdin");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("from stdin\n", env.stdout.into_string());
    }

    #[test]
    fn mixed_files_and_stdin() {
        let env = make_test_env_with_stdin(vec!["cat", "a.txt", "-", "b.txt"], "middle");
        env.fs.add_file("a.txt", "first\n");
        env.fs.add_file("b.txt", "last\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("first\nmiddle\nlast\n", env.stdout.into_string());
    }

    // ========================================================================
    // -n flag: number all lines
    // ========================================================================

    #[test]
    fn number_lines_single() {
        let env = make_test_env_with_stdin(vec!["cat", "-n"], "hello");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("     1\thello\n", env.stdout.into_string());
    }

    #[test]
    fn number_lines_multiple() {
        let env = make_test_env_with_stdin(vec!["cat", "-n"], "one\ntwo\nthree");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let expected = "     1\tone\n     2\ttwo\n     3\tthree\n";
        assert_eq!(expected, env.stdout.into_string());
    }

    #[test]
    fn number_lines_with_blanks() {
        let env = make_test_env_with_stdin(vec!["cat", "-n"], "one\n\nthree");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let expected = "     1\tone\n     2\t\n     3\tthree\n";
        assert_eq!(expected, env.stdout.into_string());
    }

    #[test]
    fn number_lines_across_files() {
        let env = make_test_env_with_stdin(vec!["cat", "-n", "a.txt", "b.txt"], "");
        env.fs.add_file("a.txt", "one\ntwo\n");
        env.fs.add_file("b.txt", "three\nfour\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let expected = "     1\tone\n     2\ttwo\n     3\tthree\n     4\tfour\n";
        println!("stdout: {:?}", env.stdout.into_string());
        assert_eq!(expected, env.stdout.into_string());
    }

    // ========================================================================
    // -b flag: number non-blank lines only
    // ========================================================================

    #[test]
    fn number_nonblank_with_blanks() {
        let env = make_test_env_with_stdin(vec!["cat", "-b"], "one\n\nthree");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let expected = "     1\tone\n\n     2\tthree\n";
        println!("stdout: {:?}", env.stdout.into_string());
        assert_eq!(expected, env.stdout.into_string());
    }

    #[test]
    fn number_nonblank_overrides_n() {
        // When both -b and -n are specified, -b takes precedence
        let env = make_test_env_with_stdin(vec!["cat", "-bn"], "one\n\nthree");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let expected = "     1\tone\n\n     2\tthree\n";
        assert_eq!(expected, env.stdout.into_string());
    }

    #[test]
    fn number_nonblank_no_blanks() {
        let env = make_test_env_with_stdin(vec!["cat", "-b"], "one\ntwo\nthree");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let expected = "     1\tone\n     2\ttwo\n     3\tthree\n";
        assert_eq!(expected, env.stdout.into_string());
    }

    // ========================================================================
    // -s flag: squeeze blank lines
    // ========================================================================

    #[test]
    fn squeeze_blank_double() {
        let env = make_test_env_with_stdin(vec!["cat", "-s"], "one\n\n\nfour");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let expected = "one\n\nfour\n";
        println!("stdout: {:?}", env.stdout.into_string());
        assert_eq!(expected, env.stdout.into_string());
    }

    #[test]
    fn squeeze_blank_many() {
        let env = make_test_env_with_stdin(vec!["cat", "-s"], "one\n\n\n\n\nsix");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let expected = "one\n\nsix\n";
        assert_eq!(expected, env.stdout.into_string());
    }

    #[test]
    fn squeeze_blank_single_preserved() {
        let env = make_test_env_with_stdin(vec!["cat", "-s"], "one\n\nthree");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let expected = "one\n\nthree\n";
        assert_eq!(expected, env.stdout.into_string());
    }

    #[test]
    fn squeeze_blank_at_start() {
        let env = make_test_env_with_stdin(vec!["cat", "-s"], "\n\n\none");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let expected = "\none\n";
        println!("stdout: {:?}", env.stdout.into_string());
        assert_eq!(expected, env.stdout.into_string());
    }

    // ========================================================================
    // -e flag: show ends ($) and non-printing
    // ========================================================================

    #[test]
    fn show_ends_simple() {
        let env = make_test_env_with_stdin(vec!["cat", "-e"], "hello\nworld");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let expected = "hello$\nworld$\n";
        assert_eq!(expected, env.stdout.into_string());
    }

    #[test]
    fn show_ends_empty_line() {
        let env = make_test_env_with_stdin(vec!["cat", "-e"], "one\n\nthree");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let expected = "one$\n$\nthree$\n";
        assert_eq!(expected, env.stdout.into_string());
    }

    // ========================================================================
    // -t flag: show tabs as ^I
    // ========================================================================

    #[test]
    fn show_tabs_simple() {
        let env = make_test_env_with_stdin(vec!["cat", "-t"], "hello\tworld");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let expected = "hello^Iworld\n";
        assert_eq!(expected, env.stdout.into_string());
    }

    #[test]
    fn show_tabs_multiple() {
        let env = make_test_env_with_stdin(vec!["cat", "-t"], "\thello\t\tworld\t");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let expected = "^Ihello^I^Iworld^I\n";
        assert_eq!(expected, env.stdout.into_string());
    }

    // ========================================================================
    // -v flag: show non-printing characters
    // ========================================================================

    #[test]
    fn show_nonprinting_control_chars() {
        // Test control characters (0x01 = ^A, 0x02 = ^B, etc.)
        let input = "\x01\x02\x03";
        let env = make_test_env_with_stdin(vec!["cat", "-v"], input);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let expected = "^A^B^C\n";
        assert_eq!(expected, env.stdout.into_string());
    }

    #[test]
    fn show_nonprinting_delete() {
        // Test delete character (0x7F = ^?)
        let input = "a\x7Fb";
        let env = make_test_env_with_stdin(vec!["cat", "-v"], input);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let expected = "a^?b\n";
        assert_eq!(expected, env.stdout.into_string());
    }

    #[test]
    fn show_nonprinting_high_bit_printable() {
        // Test high bit set with printable low 7 bits (e.g., 0x80 + 'A' = 0xC1)
        // 0xC1 = 193 = M-A
        let input = "a\u{00C1}b"; // Á in Latin-1, but we treat it as M-A
        let env = make_test_env_with_stdin(vec!["cat", "-v"], input);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        // 0xC1 = 193 = 128 + 65, low 7 bits = 65 = 'A'
        let expected = "aM-Ab\n";
        println!("stdout: {:?}", env.stdout.into_string());
        assert_eq!(expected, env.stdout.into_string());
    }

    #[test]
    fn show_nonprinting_tab_preserved() {
        // With just -v, tabs should pass through (only -t shows tabs as ^I)
        let input = "a\tb";
        let env = make_test_env_with_stdin(vec!["cat", "-v"], input);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let expected = "a\tb\n";
        assert_eq!(expected, env.stdout.into_string());
    }

    #[test]
    fn show_nonprinting_high_bit_control() {
        // Test high bit set with control character in low 7 bits
        // 0x81 = 129 = M-^A
        let input = "a\u{0081}b";
        let env = make_test_env_with_stdin(vec!["cat", "-v"], input);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let expected = "aM-^Ab\n";
        println!("stdout: {:?}", env.stdout.into_string());
        assert_eq!(expected, env.stdout.into_string());
    }

    #[test]
    fn show_nonprinting_high_bit_delete() {
        // Test 0xFF = M-^?
        let input = "a\u{00FF}b";
        let env = make_test_env_with_stdin(vec!["cat", "-v"], input);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        // 0xFF = 255 = 128 + 127, low 7 bits = 127 = DEL = ^?
        let expected = "aM-^?b\n";
        println!("stdout: {:?}", env.stdout.into_string());
        assert_eq!(expected, env.stdout.into_string());
    }

    // ========================================================================
    // Combined flags
    // ========================================================================

    #[test]
    fn combined_sn() {
        // -s and -n together
        let env = make_test_env_with_stdin(vec!["cat", "-sn"], "one\n\n\nfour");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let expected = "     1\tone\n     2\t\n     3\tfour\n";
        println!("stdout: {:?}", env.stdout.into_string());
        assert_eq!(expected, env.stdout.into_string());
    }

    #[test]
    fn combined_sb() {
        // -s and -b together
        let env = make_test_env_with_stdin(vec!["cat", "-sb"], "one\n\n\nfour");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let expected = "     1\tone\n\n     2\tfour\n";
        println!("stdout: {:?}", env.stdout.into_string());
        assert_eq!(expected, env.stdout.into_string());
    }

    #[test]
    fn combined_et() {
        // -e and -t together (both imply -v)
        let env = make_test_env_with_stdin(vec!["cat", "-et"], "hello\tworld");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let expected = "hello^Iworld$\n";
        assert_eq!(expected, env.stdout.into_string());
    }

    #[test]
    fn combined_net() {
        // -n, -e, and -t together
        let env = make_test_env_with_stdin(vec!["cat", "-net"], "a\tb\nc");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let expected = "     1\ta^Ib$\n     2\tc$\n";
        assert_eq!(expected, env.stdout.into_string());
    }

    #[test]
    fn all_flags() {
        // All flags together (except -l and -u which are no-ops)
        let env = make_test_env_with_stdin(vec!["cat", "-benstuv"], "one\n\n\nfour\tfive");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        // -b: number non-blank only
        // -s: squeeze blanks
        // -e: show $ at end
        // -t: show tabs as ^I
        // -v: show non-printing (already implied by -e and -t)
        let expected = "     1\tone$\n$\n     2\tfour^Ifive$\n";
        println!("stdout: {:?}", env.stdout.into_string());
        assert_eq!(expected, env.stdout.into_string());
    }

    // ========================================================================
    // Option parsing tests
    // ========================================================================

    #[test]
    fn illegal_option() {
        let env = make_test_env_with_stdin(vec!["cat", "-x"], "");
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("Unrecognized option"));
    }

    #[test]
    fn double_dash_ends_options() {
        // -- should end option processing
        let env = make_test_env_with_stdin(vec!["cat", "-n", "--", "-b"], "");
        env.fs.add_file("-b", "contents of -b file\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let expected = "     1\tcontents of -b file\n";
        println!("stdout: {:?}", env.stdout.into_string());
        assert_eq!(expected, env.stdout.into_string());
    }

    #[test]
    fn options_combined() {
        let env = make_test_env_with_stdin(vec!["cat", "-bn", "-s"], "one\n\n\nfour");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        // -b takes precedence over -n
        let expected = "     1\tone\n\n     2\tfour\n";
        assert_eq!(expected, env.stdout.into_string());
    }

    // ========================================================================
    // Edge cases
    // ========================================================================

    #[test]
    fn empty_file() {
        let env = make_test_env_with_stdin(vec!["cat", "empty.txt"], "");
        env.fs.add_file("empty.txt", "");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("", env.stdout.into_string());
    }

    #[test]
    fn file_no_trailing_newline() {
        let env = make_test_env_with_stdin(vec!["cat", "file.txt"], "");
        env.fs.add_file("file.txt", "no newline");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        // We add a newline at the end
        assert_eq!("no newline\n", env.stdout.into_string());
    }

    #[test]
    fn file_with_trailing_newline() {
        let env = make_test_env_with_stdin(vec!["cat", "file.txt"], "");
        env.fs.add_file("file.txt", "with newline\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("with newline\n", env.stdout.into_string());
    }

    #[test]
    fn multiple_files_one_missing() {
        let env = make_test_env_with_stdin(vec!["cat", "a.txt", "missing.txt", "b.txt"], "");
        env.fs.add_file("a.txt", "aaa\n");
        env.fs.add_file("b.txt", "bbb\n");
        let result = bin(&env).unwrap();
        // Exit code should be non-zero due to missing file
        assert_eq!(1, result.code());
        // But we should still see output from the valid files
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert!(stdout.contains("aaa"));
        assert!(stdout.contains("bbb"));
        // And an error message
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("missing.txt"));
    }

    #[test]
    fn file_as_first_arg_after_double_dash() {
        let env = make_test_env_with_stdin(vec!["cat", "--", "file.txt"], "");
        env.fs.add_file("file.txt", "contents\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("contents\n", env.stdout.into_string());
    }

    // ========================================================================
    // -l and -u flags (no-op tests)
    // ========================================================================

    #[test]
    fn lock_flag_noop() {
        let env = make_test_env_with_stdin(vec!["cat", "-l"], "hello");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("hello\n", env.stdout.into_string());
    }

    #[test]
    fn unbuffered_flag_noop() {
        let env = make_test_env_with_stdin(vec!["cat", "-u"], "hello");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("hello\n", env.stdout.into_string());
    }

    // ========================================================================
    // make_visible function unit tests
    // ========================================================================

    #[test]
    fn make_visible_printable() {
        assert_eq!("a", make_visible('a'));
        assert_eq!("Z", make_visible('Z'));
        assert_eq!(" ", make_visible(' '));
        assert_eq!("~", make_visible('~'));
    }

    #[test]
    fn make_visible_control() {
        assert_eq!("^@", make_visible('\x00')); // NUL
        assert_eq!("^A", make_visible('\x01'));
        assert_eq!("^G", make_visible('\x07')); // BEL
        assert_eq!("\t", make_visible('\x09')); // TAB (preserved with just -v)
        assert_eq!("^M", make_visible('\x0D')); // CR
        assert_eq!("^[", make_visible('\x1B')); // ESC
        assert_eq!("^_", make_visible('\x1F'));
    }

    #[test]
    fn make_visible_delete() {
        assert_eq!("^?", make_visible('\x7F'));
    }

    #[test]
    fn make_visible_meta_printable() {
        // 0x80 + 0x41 (A) = 0xC1
        assert_eq!("M-A", make_visible('\u{00C1}'));
        // 0x80 + 0x20 (space) = 0xA0
        assert_eq!("M- ", make_visible('\u{00A0}'));
    }

    #[test]
    fn make_visible_meta_control() {
        // 0x80 + 0x00 = 0x80 -> M-^@
        assert_eq!("M-^@", make_visible('\u{0080}'));
        // 0x80 + 0x01 = 0x81 -> M-^A
        assert_eq!("M-^A", make_visible('\u{0081}'));
    }

    #[test]
    fn make_visible_meta_delete() {
        // 0x80 + 0x7F = 0xFF -> M-^?
        assert_eq!("M-^?", make_visible('\u{00FF}'));
    }

    // ========================================================================
    // Line numbering format tests
    // ========================================================================

    #[test]
    fn line_number_format_single_digit() {
        let env = make_test_env_with_stdin(vec!["cat", "-n"], "a");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        // Should be right-justified in 6 characters
        assert_eq!("     1\ta\n", env.stdout.into_string());
    }

    #[test]
    fn line_number_format_multi_digit() {
        // Create input with 12 lines
        let input = "1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n11\n12";
        let env = make_test_env_with_stdin(vec!["cat", "-n"], input);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let output = env.stdout.into_string();
        println!("output: {:?}", output);
        // Check that line 12 is formatted correctly
        assert!(output.contains("    12\t12"));
    }

    // ========================================================================
    // Stdin reading with dash in various positions
    // ========================================================================

    #[test]
    fn stdin_first() {
        let env = make_test_env_with_stdin(vec!["cat", "-", "a.txt"], "stdin content");
        env.fs.add_file("a.txt", "file content\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("stdin content\nfile content\n", env.stdout.into_string());
    }

    #[test]
    fn stdin_last() {
        let env = make_test_env_with_stdin(vec!["cat", "a.txt", "-"], "stdin content");
        env.fs.add_file("a.txt", "file content\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("file content\nstdin content\n", env.stdout.into_string());
    }

    #[test]
    fn stdin_middle() {
        let env = make_test_env_with_stdin(vec!["cat", "a.txt", "-", "b.txt"], "stdin");
        env.fs.add_file("a.txt", "AAA\n");
        env.fs.add_file("b.txt", "BBB\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("AAA\nstdin\nBBB\n", env.stdout.into_string());
    }
}

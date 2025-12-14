//! The yes utility: be repetitively affirmative.

use crate::{Environment, Error, ExitCode, Filesystem, Stderr, Stdin, Stdout};

/// Default number of lines to output when no limit is specified.
const DEFAULT_COUNT: u64 = 1000;

/// The yes builtin: output a string repeatedly.
///
/// Usage:
///   yes [expletive]
///
/// Outputs the expletive (default "y") repeatedly, one per line.
/// For safety in a virtual shell environment, defaults to 1000 lines.
pub fn bin<SI, SO, SE, FS>(env: &Environment<SI, SO, SE, FS>) -> Result<ExitCode, Error>
where
    SI: Stdin,
    SO: Stdout,
    SE: Stderr,
    FS: Filesystem,
{
    let expletive = if env.args.len() > 1 {
        env.args[1..].join(" ")
    } else {
        "y".to_string()
    };

    for _ in 0..DEFAULT_COUNT {
        if env.is_exit_signaled() {
            break;
        }
        env.stdout.write_line(&expletive)?;
    }

    Ok(ExitCode::from(0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::make_test_env;

    #[test]
    fn default_outputs_y() {
        let env = make_test_env(vec!["yes"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let output = env.stdout.into_string();
        let lines: Vec<&str> = output.lines().collect();
        assert_eq!(DEFAULT_COUNT as usize, lines.len());
        for line in lines {
            assert_eq!("y", line);
        }
    }

    #[test]
    fn custom_expletive() {
        let env = make_test_env(vec!["yes", "hello"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let output = env.stdout.into_string();
        let lines: Vec<&str> = output.lines().collect();
        assert_eq!(DEFAULT_COUNT as usize, lines.len());
        for line in lines {
            assert_eq!("hello", line);
        }
    }

    #[test]
    fn multiple_words_joined() {
        let env = make_test_env(vec!["yes", "hello", "world"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let output = env.stdout.into_string();
        let lines: Vec<&str> = output.lines().collect();
        assert_eq!(DEFAULT_COUNT as usize, lines.len());
        for line in lines {
            assert_eq!("hello world", line);
        }
    }

    #[test]
    fn empty_string_expletive() {
        let env = make_test_env(vec!["yes", ""]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let output = env.stdout.into_string();
        let lines: Vec<&str> = output.lines().collect();
        assert_eq!(DEFAULT_COUNT as usize, lines.len());
        for line in lines {
            assert_eq!("", line);
        }
    }

    #[test]
    fn respects_exit_signal() {
        let env = make_test_env(vec!["yes"]);
        env.signal_exit();
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let output = env.stdout.into_string();
        println!("output length: {}", output.len());
        assert_eq!("", output);
    }

    #[test]
    fn special_characters() {
        let env = make_test_env(vec!["yes", "!@#$%"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let output = env.stdout.into_string();
        let lines: Vec<&str> = output.lines().collect();
        assert_eq!(DEFAULT_COUNT as usize, lines.len());
        for line in lines {
            assert_eq!("!@#$%", line);
        }
    }

    #[test]
    fn produces_no_stderr() {
        let env = make_test_env(vec!["yes"]);
        let _ = bin(&env).unwrap();
        assert_eq!("", env.stderr.into_string());
    }

    #[test]
    fn unicode_expletive() {
        let env = make_test_env(vec!["yes", "日本語"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let output = env.stdout.into_string();
        let lines: Vec<&str> = output.lines().collect();
        assert_eq!(DEFAULT_COUNT as usize, lines.len());
        for line in lines {
            assert_eq!("日本語", line);
        }
    }

    #[test]
    fn newline_in_expletive() {
        let env = make_test_env(vec!["yes", "line1\\nline2"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let output = env.stdout.into_string();
        let lines: Vec<&str> = output.lines().collect();
        // Each iteration outputs "line1\nline2" literally (no escape processing)
        assert_eq!(DEFAULT_COUNT as usize, lines.len());
        for line in lines {
            assert_eq!("line1\\nline2", line);
        }
    }
}

use crate::{Environment, Error, ExitCode, Filesystem, Stderr, Stdin, Stdout};

mod basename;
mod cat;
mod comm;
mod cut;
mod echo;
mod fold;
mod head;
mod nl;
mod paste;
mod seq;
pub mod sh;
mod tail;
mod tee;
mod tr;
mod truncate;
mod uniq;
mod wc;

/// Look up a builtin binary by name.
#[allow(clippy::type_complexity)]
pub fn lookup_bin<SI, SO, SE, FS>(
    bin: &str,
) -> Result<fn(&Environment<SI, SO, SE, FS>) -> Result<ExitCode, Error>, Error>
where
    SI: Stdin,
    SO: Stdout,
    SE: Stderr,
    FS: Filesystem,
{
    match bin {
        "basename" | "/usr/bin/basename" => Ok(basename::bin),
        "cat" | "/bin/cat" => Ok(cat::bin),
        "comm" | "/usr/bin/comm" => Ok(comm::bin),
        "cut" | "/usr/bin/cut" => Ok(cut::bin),
        "echo" | "/bin/echo" => Ok(echo::bin),
        "exit" => Ok(exit_bin),
        "fold" | "/usr/bin/fold" => Ok(fold::bin),
        "head" | "/usr/bin/head" => Ok(head::bin),
        "nl" | "/usr/bin/nl" => Ok(nl::bin),
        "paste" | "/usr/bin/paste" => Ok(paste::bin),
        "seq" | "/usr/bin/seq" => Ok(seq::bin),
        "tail" | "/usr/bin/tail" => Ok(tail::bin),
        "tee" | "/usr/bin/tee" => Ok(tee::bin),
        "tr" | "/usr/bin/tr" => Ok(tr::bin),
        "false" | "/bin/false" => Ok(|env| sh::run_string(include_str!("../../shell/false"), env)),
        "sh" | "/bin/sh" => Ok(sh::bin),
        "true" | "/bin/true" => Ok(|env| sh::run_string(include_str!("../../shell/true"), env)),
        "truncate" | "/usr/bin/truncate" => Ok(truncate::bin),
        "uniq" | "/usr/bin/uniq" => Ok(uniq::bin),
        "wc" | "/usr/bin/wc" => Ok(wc::bin),
        _ => Err(Error::UnknownBinary(bin.to_string())),
    }
}

/// The exit builtin: exit the shell with an optional exit code.
///
/// Usage:
///   exit [n]
///
/// If n is omitted, the exit code is 0.
fn exit_bin<SI, SO, SE, FS>(env: &Environment<SI, SO, SE, FS>) -> Result<ExitCode, Error>
where
    SI: Stdin,
    SO: Stdout,
    SE: Stderr,
    FS: Filesystem,
{
    let code = if env.args.len() > 1 {
        match env.args[1].parse::<i8>() {
            Ok(n) => n,
            Err(_) => {
                env.stderr
                    .write_line(&format!("exit: {}: numeric argument required", env.args[1]))?;
                return Ok(ExitCode::from(2));
            }
        }
    } else {
        0
    };
    env.signal_exit();
    Ok(ExitCode::from(code))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MockFilesystem, StringStderr, StringStdin, StringStdout};

    fn make_env(
        args: Vec<&str>,
    ) -> Environment<StringStdin, StringStdout, StringStderr, MockFilesystem> {
        Environment {
            stdin: StringStdin::new(""),
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
    // exit builtin tests
    // ========================================================================

    #[test]
    fn exit_no_args_returns_zero() {
        let env = make_env(vec!["exit"]);
        let result = exit_bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert!(env.is_exit_signaled());
    }

    #[test]
    fn exit_zero_returns_zero() {
        let env = make_env(vec!["exit", "0"]);
        let result = exit_bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert!(env.is_exit_signaled());
    }

    #[test]
    fn exit_one_returns_one() {
        let env = make_env(vec!["exit", "1"]);
        let result = exit_bin(&env).unwrap();
        assert_eq!(1, result.code());
        assert!(env.is_exit_signaled());
    }

    #[test]
    fn exit_42_returns_42() {
        let env = make_env(vec!["exit", "42"]);
        let result = exit_bin(&env).unwrap();
        assert_eq!(42, result.code());
        assert!(env.is_exit_signaled());
    }

    #[test]
    fn exit_negative_returns_negative() {
        let env = make_env(vec!["exit", "-1"]);
        let result = exit_bin(&env).unwrap();
        assert_eq!(-1, result.code());
        assert!(env.is_exit_signaled());
    }

    #[test]
    fn exit_127_returns_127() {
        let env = make_env(vec!["exit", "127"]);
        let result = exit_bin(&env).unwrap();
        assert_eq!(127, result.code());
        assert!(env.is_exit_signaled());
    }

    #[test]
    fn exit_non_numeric_returns_error() {
        let env = make_env(vec!["exit", "abc"]);
        let result = exit_bin(&env).unwrap();
        assert_eq!(2, result.code());
        assert!(!env.is_exit_signaled());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("numeric argument required"));
        assert!(stderr.contains("abc"));
    }

    #[test]
    fn exit_empty_string_returns_error() {
        let env = make_env(vec!["exit", ""]);
        let result = exit_bin(&env).unwrap();
        assert_eq!(2, result.code());
        assert!(!env.is_exit_signaled());
    }

    #[test]
    fn exit_overflow_returns_error() {
        let env = make_env(vec!["exit", "999"]);
        let result = exit_bin(&env).unwrap();
        assert_eq!(2, result.code());
        assert!(!env.is_exit_signaled());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("numeric argument required"));
    }

    #[test]
    fn exit_extra_args_ignored() {
        let env = make_env(vec!["exit", "5", "extra", "args"]);
        let result = exit_bin(&env).unwrap();
        assert_eq!(5, result.code());
        assert!(env.is_exit_signaled());
    }

    // ========================================================================
    // true builtin tests
    // ========================================================================

    #[test]
    fn true_returns_zero() {
        let env = make_env(vec!["true"]);
        let bin =
            lookup_bin::<StringStdin, StringStdout, StringStderr, MockFilesystem>("true").unwrap();
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
    }

    #[test]
    fn true_with_args_returns_zero() {
        let env = make_env(vec!["true", "ignored", "arguments"]);
        let bin =
            lookup_bin::<StringStdin, StringStdout, StringStderr, MockFilesystem>("true").unwrap();
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
    }

    #[test]
    fn true_via_path_returns_zero() {
        let env = make_env(vec!["/bin/true"]);
        let bin =
            lookup_bin::<StringStdin, StringStdout, StringStderr, MockFilesystem>("/bin/true")
                .unwrap();
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
    }

    #[test]
    fn true_produces_no_output() {
        let env = make_env(vec!["true"]);
        let bin =
            lookup_bin::<StringStdin, StringStdout, StringStderr, MockFilesystem>("true").unwrap();
        let _ = bin(&env).unwrap();
        assert_eq!("", env.stdout.into_string());
        assert_eq!("", env.stderr.into_string());
    }

    // ========================================================================
    // false builtin tests
    // ========================================================================

    #[test]
    fn false_returns_one() {
        let env = make_env(vec!["false"]);
        let bin =
            lookup_bin::<StringStdin, StringStdout, StringStderr, MockFilesystem>("false").unwrap();
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
    }

    #[test]
    fn false_with_args_returns_one() {
        let env = make_env(vec!["false", "ignored", "arguments"]);
        let bin =
            lookup_bin::<StringStdin, StringStdout, StringStderr, MockFilesystem>("false").unwrap();
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
    }

    #[test]
    fn false_via_path_returns_one() {
        let env = make_env(vec!["/bin/false"]);
        let bin =
            lookup_bin::<StringStdin, StringStdout, StringStderr, MockFilesystem>("/bin/false")
                .unwrap();
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
    }

    #[test]
    fn false_produces_no_output() {
        let env = make_env(vec!["false"]);
        let bin =
            lookup_bin::<StringStdin, StringStdout, StringStderr, MockFilesystem>("false").unwrap();
        let _ = bin(&env).unwrap();
        assert_eq!("", env.stdout.into_string());
        assert_eq!("", env.stderr.into_string());
    }

    // ========================================================================
    // lookup_bin tests
    // ========================================================================

    #[test]
    fn lookup_exit() {
        let result = lookup_bin::<StringStdin, StringStdout, StringStderr, MockFilesystem>("exit");
        assert!(result.is_ok());
    }

    #[test]
    fn lookup_true() {
        let result = lookup_bin::<StringStdin, StringStdout, StringStderr, MockFilesystem>("true");
        assert!(result.is_ok());
    }

    #[test]
    fn lookup_false() {
        let result = lookup_bin::<StringStdin, StringStdout, StringStderr, MockFilesystem>("false");
        assert!(result.is_ok());
    }

    #[test]
    fn lookup_unknown_returns_error() {
        let result =
            lookup_bin::<StringStdin, StringStdout, StringStderr, MockFilesystem>("nonexistent");
        assert!(matches!(result, Err(Error::UnknownBinary(_))));
    }
}

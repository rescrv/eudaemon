use crate::{Environment, Error, ExitCode, Filesystem, StdioIn, StdioOut};

/// The echo builtin: write arguments to standard output.
pub fn bin<SI, SO, SE, FS>(env: &Environment<SI, SO, SE, FS>) -> Result<ExitCode, Error>
where
    SI: StdioIn,
    SO: StdioOut,
    SE: StdioOut,
    FS: Filesystem,
{
    let mut args = env.args[1..].iter().map(|s| s.as_str()).peekable();
    let mut nflag = false;

    // Check for -n flag (first argument only, no getopt)
    if args.peek() == Some(&"-n") {
        nflag = true;
        args.next();
    }

    let args: Vec<&str> = args.collect();

    for (i, arg) in args.iter().enumerate() {
        let is_last = i == args.len() - 1;

        if is_last {
            // Check for trailing \c in the last argument
            if arg.len() >= 2 && arg.ends_with("\\c") {
                // Write without the \c and suppress newline
                env.stdout.write_str(&arg[..arg.len() - 2])?;
                nflag = true;
            } else {
                env.stdout.write_str(arg)?;
            }
        } else {
            env.stdout.write_str(arg)?;
            env.stdout.write_str(" ")?;
        }
    }

    if !nflag {
        env.stdout.write_str("\n")?;
    }

    Ok(ExitCode::from(0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::make_test_env;

    #[test]
    fn no_arguments() {
        let env = make_test_env(vec!["echo"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("\n", env.stdout.into_string());
    }

    #[test]
    fn single_argument() {
        let env = make_test_env(vec!["echo", "hello"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("hello\n", env.stdout.into_string());
    }

    #[test]
    fn multiple_arguments() {
        let env = make_test_env(vec!["echo", "hello", "world"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("hello world\n", env.stdout.into_string());
    }

    #[test]
    fn nflag_suppresses_newline() {
        let env = make_test_env(vec!["echo", "-n", "hello"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("hello", env.stdout.into_string());
    }

    #[test]
    fn nflag_no_arguments() {
        let env = make_test_env(vec!["echo", "-n"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("", env.stdout.into_string());
    }

    #[test]
    fn nflag_multiple_arguments() {
        let env = make_test_env(vec!["echo", "-n", "hello", "world"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("hello world", env.stdout.into_string());
    }

    #[test]
    fn backslash_c_suppresses_newline() {
        let env = make_test_env(vec!["echo", "hello\\c"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("hello", env.stdout.into_string());
    }

    #[test]
    fn backslash_c_in_last_arg_only() {
        let env = make_test_env(vec!["echo", "hello", "world\\c"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("hello world", env.stdout.into_string());
    }

    #[test]
    fn backslash_c_not_at_end_is_literal() {
        let env = make_test_env(vec!["echo", "hello\\c", "world"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        // \c in non-last arg is written literally
        assert_eq!("hello\\c world\n", env.stdout.into_string());
    }

    #[test]
    fn backslash_c_alone() {
        let env = make_test_env(vec!["echo", "\\c"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("", env.stdout.into_string());
    }

    #[test]
    fn double_dash_is_literal() {
        // Per FreeBSD man page: -- is NOT recognized, written literally
        let env = make_test_env(vec!["echo", "--", "hello"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("-- hello\n", env.stdout.into_string());
    }

    #[test]
    fn dash_n_after_first_arg_is_literal() {
        // -n is only recognized as first argument
        let env = make_test_env(vec!["echo", "hello", "-n"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("hello -n\n", env.stdout.into_string());
    }

    #[test]
    fn backslashes_are_literal() {
        // No escape processing except \c at end
        let env = make_test_env(vec!["echo", "hello\\tworld"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("hello\\tworld\n", env.stdout.into_string());
    }

    #[test]
    fn backslash_n_is_literal() {
        let env = make_test_env(vec!["echo", "hello\\nworld"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("hello\\nworld\n", env.stdout.into_string());
    }

    #[test]
    fn empty_string_argument() {
        let env = make_test_env(vec!["echo", ""]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("\n", env.stdout.into_string());
    }

    #[test]
    fn empty_strings_with_spaces() {
        let env = make_test_env(vec!["echo", "", "hello", ""]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!(" hello \n", env.stdout.into_string());
    }

    #[test]
    fn hyphen_hello_is_literal() {
        // From man page example: /bin/echo "-hello\tworld" outputs -hello\tworld
        let env = make_test_env(vec!["echo", "-hello\\tworld"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("-hello\\tworld\n", env.stdout.into_string());
    }
}

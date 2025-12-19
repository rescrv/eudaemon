//! The pwd builtin: print working directory name.

use crate::{Environment, Error, ExitCode, Filesystem, StdioIn, StdioOut};

/// The pwd builtin: write the absolute pathname of the current working directory to stdout.
///
/// Usage:
///   pwd [-L | -P]
///
/// Options:
///   -L  Display the logical current working directory.
///   -P  Display the physical current working directory (all symbolic links resolved).
///
/// If no options are specified, -P is assumed. In this virtual filesystem implementation,
/// there are no symbolic links, so -L and -P behave identically.
pub fn bin<SI, SO, SE, FS>(env: &Environment<SI, SO, SE, FS>) -> Result<ExitCode, Error>
where
    SI: StdioIn,
    SO: StdioOut,
    SE: StdioOut,
    FS: Filesystem,
{
    let mut args = env.args[1..].iter().peekable();
    let mut logical = false;

    // Parse options
    while let Some(arg) = args.peek() {
        if !arg.starts_with('-') || *arg == "-" {
            break;
        }
        let arg = args.next().unwrap();
        if arg == "--" {
            break;
        }
        for ch in arg[1..].chars() {
            match ch {
                'L' => logical = true,
                'P' => logical = false,
                _ => {
                    env.stderr
                        .write_line(&format!("pwd: -{}: invalid option", ch))?;
                    env.stderr.write_line("usage: pwd [-L | -P]")?;
                    return Ok(ExitCode::from(1));
                }
            }
        }
    }

    // Check for extra arguments
    if args.next().is_some() {
        env.stderr.write_line("usage: pwd [-L | -P]")?;
        return Ok(ExitCode::from(1));
    }

    // For -L, check PWD environment variable first
    if logical
        && let Some(pwd) = env.env.get("PWD")
        && pwd.starts_with('/')
    {
        env.stdout.write_line(pwd)?;
        return Ok(ExitCode::from(0));
    }

    // Fall back to physical cwd (or use it directly for -P)
    env.stdout.write_line(env.cwd.as_str())?;
    Ok(ExitCode::from(0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::{TestEnvBuilder, make_test_env};

    #[test]
    fn no_arguments_prints_cwd() {
        let env = make_test_env(vec!["pwd"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("/\n", env.stdout.into_string());
    }

    #[test]
    fn prints_non_root_cwd() {
        let env = TestEnvBuilder::new()
            .args(vec!["pwd"])
            .cwd("/usr/src/sys/kern")
            .build();
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("/usr/src/sys/kern\n", env.stdout.into_string());
    }

    #[test]
    fn physical_flag_prints_cwd() {
        let env = TestEnvBuilder::new()
            .args(vec!["pwd", "-P"])
            .cwd("/home/user")
            .build();
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("/home/user\n", env.stdout.into_string());
    }

    #[test]
    fn logical_flag_uses_pwd_env() {
        let env = TestEnvBuilder::new()
            .args(vec!["pwd", "-L"])
            .cwd("/usr/src/sys/kern")
            .env_var("PWD", "/sys/kern")
            .build();
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("/sys/kern\n", env.stdout.into_string());
    }

    #[test]
    fn logical_flag_falls_back_to_cwd_if_no_pwd() {
        let env = TestEnvBuilder::new()
            .args(vec!["pwd", "-L"])
            .cwd("/usr/src/sys/kern")
            .build();
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("/usr/src/sys/kern\n", env.stdout.into_string());
    }

    #[test]
    fn logical_flag_falls_back_if_pwd_not_absolute() {
        let env = TestEnvBuilder::new()
            .args(vec!["pwd", "-L"])
            .cwd("/usr/src")
            .env_var("PWD", "relative/path")
            .build();
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("/usr/src\n", env.stdout.into_string());
    }

    #[test]
    fn physical_overrides_logical() {
        let env = TestEnvBuilder::new()
            .args(vec!["pwd", "-L", "-P"])
            .cwd("/usr/src/sys/kern")
            .env_var("PWD", "/sys/kern")
            .build();
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        // -P comes last, so physical wins
        assert_eq!("/usr/src/sys/kern\n", env.stdout.into_string());
    }

    #[test]
    fn logical_overrides_physical() {
        let env = TestEnvBuilder::new()
            .args(vec!["pwd", "-P", "-L"])
            .cwd("/usr/src/sys/kern")
            .env_var("PWD", "/sys/kern")
            .build();
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        // -L comes last, so logical wins
        assert_eq!("/sys/kern\n", env.stdout.into_string());
    }

    #[test]
    fn combined_flags_lp() {
        let env = TestEnvBuilder::new()
            .args(vec!["pwd", "-LP"])
            .cwd("/usr/src/sys/kern")
            .env_var("PWD", "/sys/kern")
            .build();
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        // P comes after L in -LP, so physical wins
        assert_eq!("/usr/src/sys/kern\n", env.stdout.into_string());
    }

    #[test]
    fn combined_flags_pl() {
        let env = TestEnvBuilder::new()
            .args(vec!["pwd", "-PL"])
            .cwd("/usr/src/sys/kern")
            .env_var("PWD", "/sys/kern")
            .build();
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        // L comes after P in -PL, so logical wins
        assert_eq!("/sys/kern\n", env.stdout.into_string());
    }

    #[test]
    fn invalid_option_returns_error() {
        let env = make_test_env(vec!["pwd", "-x"]);
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("invalid option"));
        assert!(stderr.contains("-x"));
    }

    #[test]
    fn extra_arguments_returns_error() {
        let env = make_test_env(vec!["pwd", "extra"]);
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("usage:"));
    }

    #[test]
    fn double_dash_stops_option_parsing() {
        let env = make_test_env(vec!["pwd", "--"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("/\n", env.stdout.into_string());
    }

    #[test]
    fn double_dash_with_extra_args_is_error() {
        let env = make_test_env(vec!["pwd", "--", "extra"]);
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
    }

    #[test]
    fn produces_no_stderr_on_success() {
        let env = make_test_env(vec!["pwd"]);
        let _ = bin(&env).unwrap();
        assert_eq!("", env.stderr.into_string());
    }
}

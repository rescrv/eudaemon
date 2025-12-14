//! The uname builtin: print synshell system identification.

use crate::{Environment, Error, ExitCode, Filesystem, Stderr, Stdin, Stdout};

/// Synshell version, pulled from Cargo.toml at compile time.
const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The uname builtin: print synshell system identification.
///
/// Usage:
///   uname [-a] [-s] [-r] [-v] [-m] [-o]
///
/// Options:
///   -a  Print all information (equivalent to -srvmo).
///   -s  Print the system name (synshell).
///   -r  Print the release version.
///   -v  Print the build version info.
///   -m  Print the machine hardware name (host architecture).
///   -o  Print the operating system name (host OS).
///
/// With no options, -s is assumed.
///
/// This command is designed for an LLM to identify the synshell environment it's operating in.
pub fn bin<SI, SO, SE, FS>(env: &Environment<SI, SO, SE, FS>) -> Result<ExitCode, Error>
where
    SI: Stdin,
    SO: Stdout,
    SE: Stderr,
    FS: Filesystem,
{
    let mut args = env.args[1..].iter().peekable();

    let mut print_sysname = false;
    let mut print_release = false;
    let mut print_version = false;
    let mut print_machine = false;
    let mut print_os = false;

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
                'a' => {
                    print_sysname = true;
                    print_release = true;
                    print_version = true;
                    print_machine = true;
                    print_os = true;
                }
                's' => print_sysname = true,
                'r' => print_release = true,
                'v' => print_version = true,
                'm' => print_machine = true,
                'o' => print_os = true,
                _ => {
                    env.stderr
                        .write_line(&format!("uname: -{}: invalid option", ch))?;
                    env.stderr
                        .write_line("usage: uname [-a] [-s] [-r] [-v] [-m] [-o]")?;
                    return Ok(ExitCode::from(1));
                }
            }
        }
    }

    // Check for extra arguments
    if args.next().is_some() {
        env.stderr
            .write_line("usage: uname [-a] [-s] [-r] [-v] [-m] [-o]")?;
        return Ok(ExitCode::from(1));
    }

    // Default to -s if no options specified
    if !print_sysname && !print_release && !print_version && !print_machine && !print_os {
        print_sysname = true;
    }

    // Collect output parts
    let mut parts: Vec<&str> = Vec::new();

    if print_sysname {
        parts.push("synshell");
    }

    if print_release {
        parts.push(VERSION);
    }

    if print_version {
        // Build version info - could include git hash, build date, etc.
        // For now, just use the version
        parts.push(VERSION);
    }

    if print_machine {
        parts.push(std::env::consts::ARCH);
    }

    if print_os {
        parts.push(std::env::consts::OS);
    }

    env.stdout.write_line(&parts.join(" "))?;
    Ok(ExitCode::from(0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::make_test_env;

    #[test]
    fn no_arguments_prints_sysname() {
        let env = make_test_env(vec!["uname"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("synshell\n", env.stdout.into_string());
    }

    #[test]
    fn sysname_flag() {
        let env = make_test_env(vec!["uname", "-s"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("synshell\n", env.stdout.into_string());
    }

    #[test]
    fn release_flag() {
        let env = make_test_env(vec!["uname", "-r"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let output = env.stdout.into_string();
        println!("output: {:?}", output);
        assert!(output.contains(VERSION));
    }

    #[test]
    fn version_flag() {
        let env = make_test_env(vec!["uname", "-v"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let output = env.stdout.into_string();
        println!("output: {:?}", output);
        assert!(output.contains(VERSION));
    }

    #[test]
    fn machine_flag() {
        let env = make_test_env(vec!["uname", "-m"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let output = env.stdout.into_string();
        println!("output: {:?}", output);
        assert!(output.contains(std::env::consts::ARCH));
    }

    #[test]
    fn os_flag() {
        let env = make_test_env(vec!["uname", "-o"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let output = env.stdout.into_string();
        println!("output: {:?}", output);
        assert!(output.contains(std::env::consts::OS));
    }

    #[test]
    fn all_flag() {
        let env = make_test_env(vec!["uname", "-a"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let output = env.stdout.into_string();
        println!("output: {:?}", output);
        assert!(output.contains("synshell"));
        assert!(output.contains(VERSION));
        assert!(output.contains(std::env::consts::ARCH));
        assert!(output.contains(std::env::consts::OS));
    }

    #[test]
    fn combined_flags_sr() {
        let env = make_test_env(vec!["uname", "-sr"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let output = env.stdout.into_string();
        println!("output: {:?}", output);
        assert!(output.starts_with("synshell "));
        assert!(output.contains(VERSION));
    }

    #[test]
    fn multiple_separate_flags() {
        let env = make_test_env(vec!["uname", "-s", "-m"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let output = env.stdout.into_string();
        println!("output: {:?}", output);
        assert!(output.contains("synshell"));
        assert!(output.contains(std::env::consts::ARCH));
    }

    #[test]
    fn invalid_option_returns_error() {
        let env = make_test_env(vec!["uname", "-x"]);
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("invalid option"));
        assert!(stderr.contains("-x"));
    }

    #[test]
    fn extra_arguments_returns_error() {
        let env = make_test_env(vec!["uname", "extra"]);
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("usage:"));
    }

    #[test]
    fn double_dash_stops_option_parsing() {
        let env = make_test_env(vec!["uname", "--"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("synshell\n", env.stdout.into_string());
    }

    #[test]
    fn double_dash_with_extra_args_is_error() {
        let env = make_test_env(vec!["uname", "--", "extra"]);
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
    }

    #[test]
    fn produces_no_stderr_on_success() {
        let env = make_test_env(vec!["uname"]);
        let _ = bin(&env).unwrap();
        assert_eq!("", env.stderr.into_string());
    }

    #[test]
    fn output_is_space_separated() {
        let env = make_test_env(vec!["uname", "-smo"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let output = env.stdout.into_string();
        println!("output: {:?}", output);
        let parts: Vec<&str> = output.trim().split(' ').collect();
        assert_eq!(3, parts.len());
        assert_eq!("synshell", parts[0]);
    }
}

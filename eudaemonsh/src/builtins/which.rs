//! The which builtin: locate a command.

use crate::{Environment, Error, ExitCode, Filesystem, Stderr, Stdin, Stdout};

/// Known command paths for builtins.
const KNOWN_COMMANDS: &[(&str, &str)] = &[
    ("base64", "/usr/bin/base64"),
    ("basename", "/usr/bin/basename"),
    ("cat", "/bin/cat"),
    ("comm", "/usr/bin/comm"),
    ("cp", "/bin/cp"),
    ("cut", "/usr/bin/cut"),
    ("date", "/bin/date"),
    ("du", "/usr/bin/du"),
    ("echo", "/bin/echo"),
    ("env", "/usr/bin/env"),
    ("exit", "exit"),
    ("expand", "/usr/bin/expand"),
    ("false", "/bin/false"),
    ("fold", "/usr/bin/fold"),
    ("grep", "/usr/bin/grep"),
    ("egrep", "/usr/bin/egrep"),
    ("fgrep", "/usr/bin/fgrep"),
    ("head", "/usr/bin/head"),
    ("ln", "/bin/ln"),
    ("ls", "/bin/ls"),
    ("mkdir", "/bin/mkdir"),
    ("mktemp", "/usr/bin/mktemp"),
    ("mv", "/bin/mv"),
    ("nl", "/usr/bin/nl"),
    ("paste", "/usr/bin/paste"),
    ("printf", "/usr/bin/printf"),
    ("pwd", "/bin/pwd"),
    ("readlink", "/usr/bin/readlink"),
    ("realpath", "/bin/realpath"),
    ("rm", "/bin/rm"),
    ("rmdir", "/bin/rmdir"),
    ("seq", "/usr/bin/seq"),
    ("sh", "/bin/sh"),
    ("shuf", "/usr/bin/shuf"),
    ("sort", "/usr/bin/sort"),
    ("split", "/usr/bin/split"),
    ("stat", "/usr/bin/stat"),
    ("tail", "/usr/bin/tail"),
    ("tee", "/usr/bin/tee"),
    ("test", "/usr/bin/test"),
    ("[", "/usr/bin/["),
    ("touch", "/usr/bin/touch"),
    ("tr", "/usr/bin/tr"),
    ("true", "/bin/true"),
    ("truncate", "/usr/bin/truncate"),
    ("uname", "/usr/bin/uname"),
    ("unexpand", "/usr/bin/unexpand"),
    ("uniq", "/usr/bin/uniq"),
    ("unlink", "/bin/unlink"),
    ("wc", "/usr/bin/wc"),
    ("which", "/usr/bin/which"),
    ("yes", "/usr/bin/yes"),
];

/// The which builtin: locate a command.
///
/// Usage:
///   which [-a] command ...
///
/// For each command argument, prints the pathname of the file that would
/// be executed when the command is invoked.
///
/// Options:
///   -a    Print all matching pathnames of each matching command.
///
/// Returns 0 if all commands are found, 1 if one or more are not found.
pub fn bin<SI, SO, SE, FS>(env: &Environment<SI, SO, SE, FS>) -> Result<ExitCode, Error>
where
    SI: Stdin,
    SO: Stdout,
    SE: Stderr,
    FS: Filesystem,
{
    let mut args = env.args[1..].iter().peekable();
    let mut all = false;
    let mut commands = Vec::new();

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
                'a' => all = true,
                _ => {
                    env.stderr
                        .write_line(&format!("which: invalid option -- '{}'", ch))?;
                    return Ok(ExitCode::from(1));
                }
            }
        }
    }

    // Collect remaining args as commands to look up
    for arg in args {
        commands.push(arg.clone());
    }

    if commands.is_empty() {
        env.stderr.write_line("usage: which [-a] command ...")?;
        return Ok(ExitCode::from(1));
    }

    let mut all_found = true;

    for cmd in &commands {
        let mut found = false;

        // Check if it's a known builtin
        for (name, path) in KNOWN_COMMANDS {
            if cmd == *name {
                env.stdout.write_line(path)?;
                found = true;
                if !all {
                    break;
                }
            }
        }

        // If it's already an absolute path, check if it's a known path
        if !found && cmd.starts_with('/') {
            for (_, path) in KNOWN_COMMANDS {
                if cmd == *path {
                    env.stdout.write_line(path)?;
                    found = true;
                    break;
                }
            }
        }

        if !found {
            env.stderr.write_line(&format!("{}: not found", cmd))?;
            all_found = false;
        }
    }

    Ok(ExitCode::from(if all_found { 0 } else { 1 }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::make_test_env;

    #[test]
    fn find_cat() {
        let env = make_test_env(vec!["which", "cat"]);
        let result = bin(&env).unwrap();
        let stdout = env.stdout.into_string();
        let stderr = env.stderr.into_string();
        println!("stdout: {:?}", stdout);
        println!("stderr: {:?}", stderr);
        assert_eq!(0, result.code());
        assert_eq!("/bin/cat\n", stdout);
    }

    #[test]
    fn find_ls() {
        let env = make_test_env(vec!["which", "ls"]);
        let result = bin(&env).unwrap();
        let stdout = env.stdout.into_string();
        let stderr = env.stderr.into_string();
        println!("stdout: {:?}", stdout);
        println!("stderr: {:?}", stderr);
        assert_eq!(0, result.code());
        assert_eq!("/bin/ls\n", stdout);
    }

    #[test]
    fn find_multiple() {
        let env = make_test_env(vec!["which", "cat", "ls", "echo"]);
        let result = bin(&env).unwrap();
        let stdout = env.stdout.into_string();
        let stderr = env.stderr.into_string();
        println!("stdout: {:?}", stdout);
        println!("stderr: {:?}", stderr);
        assert_eq!(0, result.code());
        assert_eq!("/bin/cat\n/bin/ls\n/bin/echo\n", stdout);
    }

    #[test]
    fn not_found() {
        let env = make_test_env(vec!["which", "nonexistent"]);
        let result = bin(&env).unwrap();
        let stdout = env.stdout.into_string();
        let stderr = env.stderr.into_string();
        println!("stdout: {:?}", stdout);
        println!("stderr: {:?}", stderr);
        assert_eq!(1, result.code());
        assert_eq!("", stdout);
        assert!(stderr.contains("not found"));
    }

    #[test]
    fn partial_not_found() {
        let env = make_test_env(vec!["which", "cat", "nonexistent"]);
        let result = bin(&env).unwrap();
        let stdout = env.stdout.into_string();
        let stderr = env.stderr.into_string();
        println!("stdout: {:?}", stdout);
        println!("stderr: {:?}", stderr);
        assert_eq!(1, result.code());
        assert_eq!("/bin/cat\n", stdout);
        assert!(stderr.contains("not found"));
    }

    #[test]
    fn find_builtin_exit() {
        let env = make_test_env(vec!["which", "exit"]);
        let result = bin(&env).unwrap();
        let stdout = env.stdout.into_string();
        let stderr = env.stderr.into_string();
        println!("stdout: {:?}", stdout);
        println!("stderr: {:?}", stderr);
        assert_eq!(0, result.code());
        assert_eq!("exit\n", stdout);
    }

    #[test]
    fn no_args_shows_usage() {
        let env = make_test_env(vec!["which"]);
        let result = bin(&env).unwrap();
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert_eq!(1, result.code());
        assert!(stderr.contains("usage"));
    }

    #[test]
    fn find_which() {
        let env = make_test_env(vec!["which", "which"]);
        let result = bin(&env).unwrap();
        let stdout = env.stdout.into_string();
        let stderr = env.stderr.into_string();
        println!("stdout: {:?}", stdout);
        println!("stderr: {:?}", stderr);
        assert_eq!(0, result.code());
        assert_eq!("/usr/bin/which\n", stdout);
    }

    #[test]
    fn invalid_option() {
        let env = make_test_env(vec!["which", "-x", "cat"]);
        let result = bin(&env).unwrap();
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert_eq!(1, result.code());
        assert!(stderr.contains("invalid option"));
    }
}

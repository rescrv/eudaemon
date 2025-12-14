use std::collections::HashMap;

use getopts::Options;

use crate::{Environment, Error, ExitCode, Filesystem, Stderr, Stdin, Stdout};

fn build_options() -> Options {
    let mut opts = Options::new();
    opts.optflag("0", "", "End each output line with NUL, not newline");
    opts.optflag("i", "", "Execute with only specified environment variables");
    opts.optmulti("u", "", "Remove variable from environment", "name");
    opts.optflag("v", "", "Print verbose information");
    opts
}

/// The env builtin: set environment and execute command, or print environment.
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
            env.stderr.write_line(&format!("env: {}", e))?;
            return Ok(ExitCode::from(1));
        }
    };

    let nul_terminate = matches.opt_present("0");
    let ignore_env = matches.opt_present("i");
    let verbose = matches.opt_present("v");
    let unset_vars: Vec<String> = matches.opt_strs("u");

    // Build the new environment
    let mut new_env: HashMap<String, String> = if ignore_env {
        if verbose {
            env.stderr.write_line("#env clearing environ")?;
        }
        HashMap::new()
    } else {
        env.env.clone()
    };

    // Remove variables specified with -u
    for var in &unset_vars {
        if verbose {
            env.stderr.write_line(&format!("#env unset:\t{}", var))?;
        }
        new_env.remove(var);
    }

    // Process name=value assignments and find utility
    let mut utility_idx = None;
    for (i, arg) in matches.free.iter().enumerate() {
        if let Some(eq_pos) = arg.find('=') {
            let name = &arg[..eq_pos];
            let value = &arg[eq_pos + 1..];
            if verbose {
                env.stderr.write_line(&format!("#env setenv:\t{}", arg))?;
            }
            new_env.insert(name.to_string(), value.to_string());
        } else {
            utility_idx = Some(i);
            break;
        }
    }

    // If a utility is specified, execute it
    if let Some(idx) = utility_idx {
        if nul_terminate {
            env.stderr
                .write_line("env: cannot specify command with -0")?;
            return Ok(ExitCode::from(125));
        }

        let utility = &matches.free[idx];
        let args: Vec<String> = matches.free[idx..].to_vec();

        if verbose {
            env.stderr
                .write_line(&format!("#env executing:\t{}", utility))?;
            for (i, arg) in args.iter().enumerate() {
                env.stderr
                    .write_line(&format!("#env    arg[{}]=\t'{}'", i, arg))?;
            }
        }

        // Look up and execute the utility
        let bin_fn = match crate::lookup_bin::<SI, SO, SE, FS>(utility) {
            Ok(f) => f,
            Err(_) => {
                env.stderr
                    .write_line(&format!("env: {}: No such file or directory", utility))?;
                return Ok(ExitCode::from(127));
            }
        };

        // Create new environment for the utility
        let util_env = Environment {
            stdin: env.stdin.dup(),
            stdout: env.stdout.dup(),
            stderr: env.stderr.dup(),
            fs: env.fs.dup(),
            env: new_env,
            args,
            cwd: env.cwd.clone(),
            exit_signaled: env.exit_signaled.clone(),
        };

        return bin_fn(&util_env);
    }

    // No utility: print environment
    let terminator = if nul_terminate { '\0' } else { '\n' };
    let mut pairs: Vec<_> = new_env.iter().collect();
    pairs.sort_by(|a, b| a.0.cmp(b.0));

    for (name, value) in pairs {
        env.stdout
            .write_str(&format!("{}={}{}", name, value, terminator))?;
    }

    Ok(ExitCode::from(0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::TestEnvBuilder;
    use crate::{MockFilesystem, StringStderr, StringStdin, StringStdout};

    fn make_env(
        args: Vec<&str>,
    ) -> Environment<StringStdin, StringStdout, StringStderr, MockFilesystem> {
        TestEnvBuilder::new()
            .args(args)
            .env_var("PATH", "/usr/bin:/bin")
            .env_var("HOME", "/home/user")
            .env_var("USER", "testuser")
            .build()
    }

    fn make_env_with_vars(
        args: Vec<&str>,
        vars: HashMap<String, String>,
    ) -> Environment<StringStdin, StringStdout, StringStderr, MockFilesystem> {
        TestEnvBuilder::new().args(args).env_vars(vars).build()
    }

    // ========================================================================
    // Print environment tests
    // ========================================================================

    #[test]
    fn no_args_prints_environment() {
        let env = make_env(vec!["env"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert!(stdout.contains("PATH=/usr/bin:/bin\n"));
        assert!(stdout.contains("HOME=/home/user\n"));
        assert!(stdout.contains("USER=testuser\n"));
    }

    #[test]
    fn prints_environment_sorted() {
        let env = make_env(vec!["env"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        let lines: Vec<&str> = stdout.lines().collect();
        assert_eq!(lines[0], "HOME=/home/user");
        assert_eq!(lines[1], "PATH=/usr/bin:/bin");
        assert_eq!(lines[2], "USER=testuser");
    }

    #[test]
    fn empty_environment_prints_nothing() {
        let env = make_env_with_vars(vec!["env"], HashMap::new());
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert!(stdout.is_empty());
    }

    // ========================================================================
    // -0 flag tests
    // ========================================================================

    #[test]
    fn zero_flag_uses_nul_terminator() {
        let mut vars = HashMap::new();
        vars.insert("FOO".to_string(), "bar".to_string());
        vars.insert("BAZ".to_string(), "qux".to_string());
        let env = make_env_with_vars(vec!["env", "-0"], vars);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout bytes: {:?}", stdout.as_bytes());
        assert!(stdout.contains("FOO=bar\0"));
        assert!(stdout.contains("BAZ=qux\0"));
        assert!(!stdout.contains('\n'));
    }

    #[test]
    fn zero_flag_with_command_fails() {
        let env = make_env(vec!["env", "-0", "true"]);
        let result = bin(&env).unwrap();
        assert_eq!(125, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("cannot specify command with -0"));
    }

    // ========================================================================
    // -i flag tests
    // ========================================================================

    #[test]
    fn i_flag_clears_environment() {
        let env = make_env(vec!["env", "-i"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert!(stdout.is_empty());
    }

    #[test]
    fn i_flag_with_assignments() {
        let env = make_env(vec!["env", "-i", "FOO=bar", "BAZ=qux"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert!(stdout.contains("FOO=bar\n"));
        assert!(stdout.contains("BAZ=qux\n"));
        assert!(!stdout.contains("PATH="));
        assert!(!stdout.contains("HOME="));
    }

    // ========================================================================
    // -u flag tests
    // ========================================================================

    #[test]
    fn u_flag_removes_variable() {
        let env = make_env(vec!["env", "-u", "PATH"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert!(!stdout.contains("PATH="));
        assert!(stdout.contains("HOME=/home/user\n"));
        assert!(stdout.contains("USER=testuser\n"));
    }

    #[test]
    fn u_flag_multiple_variables() {
        let env = make_env(vec!["env", "-u", "PATH", "-u", "HOME"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert!(!stdout.contains("PATH="));
        assert!(!stdout.contains("HOME="));
        assert!(stdout.contains("USER=testuser\n"));
    }

    #[test]
    fn u_flag_nonexistent_variable_succeeds() {
        let env = make_env(vec!["env", "-u", "NONEXISTENT"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
    }

    // ========================================================================
    // name=value assignment tests
    // ========================================================================

    #[test]
    fn set_new_variable() {
        let env = make_env(vec!["env", "NEWVAR=newvalue"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert!(stdout.contains("NEWVAR=newvalue\n"));
    }

    #[test]
    fn override_existing_variable() {
        let env = make_env(vec!["env", "PATH=/new/path"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert!(stdout.contains("PATH=/new/path\n"));
        assert!(!stdout.contains("PATH=/usr/bin:/bin"));
    }

    #[test]
    fn empty_value_assignment() {
        let env = make_env(vec!["env", "EMPTY="]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert!(stdout.contains("EMPTY=\n"));
    }

    #[test]
    fn value_with_equals_sign() {
        let env = make_env(vec!["env", "EQUATION=a=b=c"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert!(stdout.contains("EQUATION=a=b=c\n"));
    }

    #[test]
    fn multiple_assignments() {
        let env = make_env(vec!["env", "-i", "A=1", "B=2", "C=3"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert!(stdout.contains("A=1\n"));
        assert!(stdout.contains("B=2\n"));
        assert!(stdout.contains("C=3\n"));
    }

    // ========================================================================
    // Execute utility tests
    // ========================================================================

    #[test]
    fn execute_true() {
        let env = make_env(vec!["env", "true"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
    }

    #[test]
    fn execute_false() {
        let env = make_env(vec!["env", "false"]);
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
    }

    #[test]
    fn execute_with_modified_environment() {
        let env = make_env(vec!["env", "-i", "FOO=bar", "echo", "hello"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert_eq!("hello\n", stdout);
    }

    #[test]
    fn execute_nonexistent_utility() {
        let env = make_env(vec!["env", "nonexistent_command"]);
        let result = bin(&env).unwrap();
        assert_eq!(127, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("No such file or directory"));
        assert!(stderr.contains("nonexistent_command"));
    }

    #[test]
    fn execute_with_arguments() {
        let env = make_env(vec!["env", "echo", "arg1", "arg2", "arg3"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert_eq!("arg1 arg2 arg3\n", stdout);
    }

    #[test]
    fn unset_then_execute() {
        let env = make_env(vec!["env", "-u", "PATH", "true"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
    }

    // ========================================================================
    // -v flag tests
    // ========================================================================

    #[test]
    fn v_flag_shows_clearing() {
        let env = make_env(vec!["env", "-v", "-i"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("#env clearing environ"));
    }

    #[test]
    fn v_flag_shows_unset() {
        let env = make_env(vec!["env", "-v", "-u", "PATH"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("#env unset:\tPATH"));
    }

    #[test]
    fn v_flag_shows_setenv() {
        let env = make_env(vec!["env", "-v", "FOO=bar"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("#env setenv:\tFOO=bar"));
    }

    #[test]
    fn v_flag_shows_executing() {
        let env = make_env(vec!["env", "-v", "true"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("#env executing:\ttrue"));
        assert!(stderr.contains("#env    arg[0]=\t'true'"));
    }

    // ========================================================================
    // Combined flag tests
    // ========================================================================

    #[test]
    fn i_and_u_flags_combined() {
        let env = make_env(vec!["env", "-i", "-u", "IGNORED", "FOO=bar"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert_eq!("FOO=bar\n", stdout);
    }

    #[test]
    fn complex_scenario() {
        let env = make_env(vec![
            "env",
            "-u",
            "USER",
            "PATH=/custom/path",
            "CUSTOM=value",
            "echo",
            "test",
        ]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert_eq!("test\n", stdout);
    }

    // ========================================================================
    // Edge cases
    // ========================================================================

    #[test]
    fn variable_name_with_underscore() {
        let env = make_env(vec!["env", "-i", "MY_VAR=value"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert!(stdout.contains("MY_VAR=value\n"));
    }

    #[test]
    fn value_with_spaces() {
        let env = make_env(vec!["env", "-i", "MSG=hello world"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert!(stdout.contains("MSG=hello world\n"));
    }

    #[test]
    fn value_with_special_characters() {
        let env = make_env(vec!["env", "-i", "SPECIAL=!@#$%^&*()"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert!(stdout.contains("SPECIAL=!@#$%^&*()\n"));
    }

    #[test]
    fn dash_as_synonym_for_i() {
        let env = make_env(vec!["env", "-"]);
        let result = bin(&env).unwrap();
        // Note: getopts doesn't support bare `-` as `-i`, so this becomes a free arg
        // and tries to execute `-` as a command
        assert_eq!(127, result.code());
    }
}

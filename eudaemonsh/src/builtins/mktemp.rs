use getopts::Options;

use crate::{Environment, Error, ExitCode, Filesystem, Stderr, Stdin, Stdout};

fn build_options() -> Options {
    let mut opts = Options::new();
    opts.optflag("d", "directory", "Make a directory instead of a file");
    opts.optopt(
        "p",
        "tmpdir",
        "Use tmpdir for -t if TMPDIR is not set",
        "tmpdir",
    );
    opts.optflag("q", "quiet", "Fail silently if an error occurs");
    opts.optopt("t", "", "Generate template from prefix", "prefix");
    opts.optflag("u", "dry-run", "Unlink file before mktemp exits");
    opts
}

/// The mktemp builtin: create temporary files or directories.
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
            env.stderr.write_line(&format!("mktemp: {}", e))?;
            return Ok(ExitCode::from(1));
        }
    };

    let dflag = matches.opt_present("d");
    let qflag = matches.opt_present("q");
    let uflag = matches.opt_present("u");
    let tflag = matches.opt_present("t");
    let prefix = matches.opt_str("t");

    let mut tmpdir = matches.opt_str("p");
    let mut prefer_tmpdir = true;

    if tmpdir.as_ref().is_some_and(|s| s.is_empty()) {
        tmpdir = env.env.get("TMPDIR").cloned();
        prefer_tmpdir = false;
    } else if tmpdir.is_some() {
        prefer_tmpdir = false;
    }

    let mut templates: Vec<String> = Vec::new();

    if tflag || matches.free.is_empty() {
        let actual_prefix = prefix.as_deref().unwrap_or("tmp");

        let envtmp = if prefer_tmpdir || tmpdir.is_none() {
            env.env.get("TMPDIR").cloned()
        } else {
            None
        };

        let dir = envtmp
            .or(tmpdir.clone())
            .unwrap_or_else(|| "/tmp".to_string());
        let dir = dir.trim_end_matches('/');

        templates.push(format!("{}/{}.XXXXXXXXXX", dir, actual_prefix));
    }

    for arg in &matches.free {
        if let Some(ref dir) = tmpdir
            && !tflag
        {
            let dir = dir.trim_end_matches('/');
            templates.push(format!("{}/{}", dir, arg));
            continue;
        }
        templates.push(arg.clone());
    }

    let mut exit_code: i8 = 0;

    for template in templates {
        let result = if dflag {
            create_directory(env, &template, uflag)
        } else {
            create_file(env, &template, uflag)
        };

        match result {
            Ok(path) => {
                env.stdout.write_line(&path)?;
            }
            Err(e) => {
                exit_code = 1;
                if !qflag {
                    env.stderr.write_line(&format!("mktemp: {}", e))?;
                }
            }
        }
    }

    Ok(ExitCode::from(exit_code))
}

/// Create a temporary file using mkstemp.
fn create_file<SI, SO, SE, FS>(
    env: &Environment<SI, SO, SE, FS>,
    template: &str,
    unlink: bool,
) -> Result<String, String>
where
    SI: Stdin,
    SO: Stdout,
    SE: Stderr,
    FS: Filesystem,
{
    let path = env
        .fs
        .mkstemp(template)
        .map_err(|e| format!("mkstemp failed on {}: {:?}", template, e))?;

    if unlink {
        let _ = env.fs.unlink(&path);
    }

    Ok(path)
}

/// Create a temporary directory using mkdtemp.
fn create_directory<SI, SO, SE, FS>(
    env: &Environment<SI, SO, SE, FS>,
    template: &str,
    unlink: bool,
) -> Result<String, String>
where
    SI: Stdin,
    SO: Stdout,
    SE: Stderr,
    FS: Filesystem,
{
    let path = env
        .fs
        .mkdtemp(template)
        .map_err(|e| format!("mkdtemp failed on {}: {:?}", template, e))?;

    if unlink {
        let _ = env.fs.rmdir(&path);
    }

    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TestFilesystemExt;
    use crate::test_utils::{TestEnvBuilder, TestFilesystem};
    use crate::{StringStderr, StringStdin, StringStdout};

    fn make_env(
        args: Vec<&str>,
    ) -> Environment<StringStdin, StringStdout, StringStderr, TestFilesystem> {
        let env = TestEnvBuilder::new()
            .args(args)
            .env_var("TMPDIR", "/tmp")
            .build();
        env.fs.add_directory("/tmp");
        env
    }

    fn make_env_with_tmpdir(
        args: Vec<&str>,
        tmpdir: Option<&str>,
    ) -> Environment<StringStdin, StringStdout, StringStderr, TestFilesystem> {
        let mut builder = TestEnvBuilder::new().args(args);
        if let Some(dir) = tmpdir {
            builder = builder.env_var("TMPDIR", dir);
        }
        let env = builder.build();
        env.fs.add_directory("/tmp");
        env.fs.add_directory("/custom");
        env
    }

    #[test]
    fn no_args_creates_file_in_tmpdir() {
        let env = make_env(vec!["mktemp"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert!(stdout.starts_with("/tmp/tmp."));
        let path = stdout.trim();
        assert!(env.fs.exists(path));
    }

    #[test]
    fn d_flag_creates_directory() {
        let env = make_env(vec!["mktemp", "-d"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        let path = stdout.trim();
        assert!(env.fs.is_dir(path));
    }

    #[test]
    fn t_flag_uses_prefix() {
        let env = make_env(vec!["mktemp", "-t", "foo"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert!(stdout.starts_with("/tmp/foo."));
    }

    #[test]
    fn template_argument() {
        let env = make_env(vec!["mktemp", "/tmp/myapp.XXXXXX"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert!(stdout.starts_with("/tmp/myapp."));
        let path = stdout.trim();
        assert!(env.fs.exists(path));
    }

    #[test]
    fn u_flag_unlinks_file() {
        let env = make_env(vec!["mktemp", "-u"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        let path = stdout.trim();
        assert!(!env.fs.exists(path));
    }

    #[test]
    fn u_flag_unlinks_directory() {
        let env = make_env(vec!["mktemp", "-d", "-u"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        let path = stdout.trim();
        assert!(!env.fs.exists(path));
    }

    #[test]
    fn p_flag_overrides_default_tmpdir() {
        let env = make_env_with_tmpdir(vec!["mktemp", "-p", "/custom"], None);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert!(stdout.starts_with("/custom/tmp."));
    }

    #[test]
    fn p_flag_overrides_tmpdir_env_with_t() {
        let env = make_env_with_tmpdir(vec!["mktemp", "-p", "/custom", "-t", "foo"], Some("/tmp"));
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert!(stdout.starts_with("/custom/foo."));
    }

    #[test]
    fn p_flag_without_tmpdir_env() {
        let env = make_env_with_tmpdir(vec!["mktemp", "-p", "/custom", "-t", "bar"], None);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert!(stdout.starts_with("/custom/bar."));
    }

    #[test]
    fn multiple_templates() {
        let env = make_env(vec![
            "mktemp",
            "/tmp/a.XXXXXX",
            "/tmp/b.XXXXXX",
            "/tmp/c.XXXXXX",
        ]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        let lines: Vec<&str> = stdout.lines().collect();
        assert_eq!(3, lines.len());
        assert!(lines[0].starts_with("/tmp/a."));
        assert!(lines[1].starts_with("/tmp/b."));
        assert!(lines[2].starts_with("/tmp/c."));
    }

    #[test]
    fn q_flag_suppresses_errors() {
        let env = make_env_with_tmpdir(vec!["mktemp", "-q", "/nonexistent/foo.XXXXXX"], None);
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.is_empty());
    }

    #[test]
    fn combined_d_t_flags() {
        let env = make_env(vec!["mktemp", "-d", "-t", "mydir"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert!(stdout.starts_with("/tmp/mydir."));
        let path = stdout.trim();
        assert!(env.fs.is_dir(path));
    }

    #[test]
    fn p_flag_with_template_argument() {
        let env = make_env_with_tmpdir(vec!["mktemp", "-p", "/custom", "test.XXXXXX"], None);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert!(stdout.starts_with("/custom/test."));
    }

    #[test]
    fn trailing_slash_in_tmpdir() {
        let env = make_env_with_tmpdir(vec!["mktemp", "-t", "foo"], Some("/tmp/"));
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert!(stdout.starts_with("/tmp/foo."));
        assert!(!stdout.contains("//"));
    }
}

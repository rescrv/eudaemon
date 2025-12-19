use getopts::Options;

use crate::{Environment, Error, ExitCode, Filesystem, FsError, StdioIn, StdioOut};

fn build_options() -> Options {
    let mut opts = Options::new();
    opts.optflag("p", "", "Remove directory and ancestors");
    opts.optflag("v", "", "Be verbose when removing directories");
    opts
}

/// The rmdir builtin: remove empty directories.
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
            env.stderr.write_line(&format!("rmdir: {}", e))?;
            return Ok(ExitCode::from(2));
        }
    };

    let remove_parents = matches.opt_present("p");
    let verbose = matches.opt_present("v");

    if matches.free.is_empty() {
        env.stderr.write_line("usage: rmdir [-pv] directory ...")?;
        return Ok(ExitCode::from(2));
    }

    let mut errors = false;

    for dir in &matches.free {
        // Strip trailing slashes (but keep "/" as-is)
        let dir = if dir.len() > 1 {
            dir.trim_end_matches('/')
        } else {
            dir.as_str()
        };
        if let Err(e) = env.fs.rmdir(dir) {
            env.stderr
                .write_line(&format!("rmdir: {}: {}", dir, fs_error_message(&e)))?;
            errors = true;
        } else {
            if verbose {
                env.stdout.write_line(dir)?;
            }
            if remove_parents && rm_path(env, dir, verbose).is_err() {
                errors = true;
            }
        }
    }

    if errors {
        Ok(ExitCode::from(1))
    } else {
        Ok(ExitCode::from(0))
    }
}

/// Remove parent directories of a path, stopping at the first non-empty or root directory.
fn rm_path<SI, SO, SE, FS>(
    env: &Environment<SI, SO, SE, FS>,
    path: &str,
    verbose: bool,
) -> Result<(), ()>
where
    SI: StdioIn,
    SO: StdioOut,
    SE: StdioOut,
    FS: Filesystem,
{
    let mut path = path.trim_end_matches('/').to_string();

    while let Some(slash_pos) = path.rfind('/') {
        // Trim trailing slashes from what remains
        let mut end = slash_pos;
        while end > 0 && path.as_bytes()[end - 1] == b'/' {
            end -= 1;
        }
        if end == 0 {
            break;
        }
        path.truncate(end);

        if let Err(e) = env.fs.rmdir(&path) {
            env.stderr
                .write_line(&format!("rmdir: {}: {}", path, fs_error_message(&e)))
                .ok();
            return Err(());
        }
        if verbose {
            env.stdout.write_line(&path).ok();
        }
    }

    Ok(())
}

/// Extract a user-friendly message from a filesystem error.
fn fs_error_message(e: &FsError) -> String {
    match e {
        FsError::Io(io_err) => match io_err.kind() {
            std::io::ErrorKind::NotFound => "No such file or directory".to_string(),
            std::io::ErrorKind::DirectoryNotEmpty => "Directory not empty".to_string(),
            std::io::ErrorKind::NotADirectory => "Not a directory".to_string(),
            _ => io_err.to_string(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::{TestEnvBuilder, TestFilesystem};
    use crate::{StringStdioIn, StringStdioOut};

    fn make_env(
        args: Vec<&str>,
    ) -> Environment<StringStdioIn, StringStdioOut, StringStdioOut, TestFilesystem> {
        TestEnvBuilder::new().args(args).build()
    }

    // ========================================================================
    // Basic usage tests
    // ========================================================================

    #[test]
    fn no_args_shows_usage() {
        let env = make_env(vec!["rmdir"]);
        let result = bin(&env).unwrap();
        assert_eq!(2, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("usage:"));
    }

    #[test]
    fn remove_empty_directory() {
        let env = make_env(vec!["rmdir", "/foo"]);
        env.fs.mkdir("/foo").unwrap();
        assert!(env.fs.is_dir("/foo"));
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert!(!env.fs.is_dir("/foo"));
        assert!(!env.fs.exists("/foo"));
    }

    #[test]
    fn remove_multiple_directories() {
        let env = make_env(vec!["rmdir", "/foo", "/bar", "/baz"]);
        env.fs.mkdir("/foo").unwrap();
        env.fs.mkdir("/bar").unwrap();
        env.fs.mkdir("/baz").unwrap();
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert!(!env.fs.exists("/foo"));
        assert!(!env.fs.exists("/bar"));
        assert!(!env.fs.exists("/baz"));
    }

    #[test]
    fn remove_nonexistent_fails() {
        let env = make_env(vec!["rmdir", "/nonexistent"]);
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("No such file or directory"));
    }

    #[test]
    fn remove_file_fails() {
        let env = make_env(vec!["rmdir", "/foo"]);
        env.fs.write_string("/foo", "content").unwrap();
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("Not a directory"));
    }

    #[test]
    fn remove_nonempty_directory_fails() {
        let env = make_env(vec!["rmdir", "/foo"]);
        env.fs.mkdir("/foo").unwrap();
        env.fs.mkdir("/foo/bar").unwrap();
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("Directory not empty"));
        assert!(env.fs.is_dir("/foo"));
    }

    // ========================================================================
    // -p flag tests
    // ========================================================================

    #[test]
    fn p_removes_parent_directories() {
        let env = make_env(vec!["rmdir", "-p", "/foo/bar/baz"]);
        env.fs.mkdir("/foo").unwrap();
        env.fs.mkdir("/foo/bar").unwrap();
        env.fs.mkdir("/foo/bar/baz").unwrap();
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert!(!env.fs.exists("/foo/bar/baz"));
        assert!(!env.fs.exists("/foo/bar"));
        assert!(!env.fs.exists("/foo"));
    }

    #[test]
    fn p_stops_at_nonempty_parent() {
        let env = make_env(vec!["rmdir", "-p", "/foo/bar/baz"]);
        env.fs.mkdir("/foo").unwrap();
        env.fs.mkdir("/foo/bar").unwrap();
        env.fs.mkdir("/foo/bar/baz").unwrap();
        env.fs.write_string("/foo/sibling", "content").unwrap();
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        assert!(!env.fs.exists("/foo/bar/baz"));
        assert!(!env.fs.exists("/foo/bar"));
        assert!(env.fs.is_dir("/foo"));
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("Directory not empty"));
    }

    #[test]
    fn p_initial_failure_no_parent_removal() {
        let env = make_env(vec!["rmdir", "-p", "/foo/bar/baz"]);
        env.fs.mkdir("/foo").unwrap();
        env.fs.mkdir("/foo/bar").unwrap();
        env.fs.mkdir("/foo/bar/baz").unwrap();
        env.fs.write_string("/foo/bar/baz/file", "content").unwrap();
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        assert!(env.fs.is_dir("/foo/bar/baz"));
        assert!(env.fs.is_dir("/foo/bar"));
        assert!(env.fs.is_dir("/foo"));
    }

    // ========================================================================
    // -v flag tests
    // ========================================================================

    #[test]
    fn v_prints_removed_directory() {
        let env = make_env(vec!["rmdir", "-v", "/foo"]);
        env.fs.mkdir("/foo").unwrap();
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert!(stdout.contains("/foo"));
    }

    #[test]
    fn pv_prints_all_removed_directories() {
        let env = make_env(vec!["rmdir", "-pv", "/foo/bar/baz"]);
        env.fs.mkdir("/foo").unwrap();
        env.fs.mkdir("/foo/bar").unwrap();
        env.fs.mkdir("/foo/bar/baz").unwrap();
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert!(stdout.contains("/foo/bar/baz\n"));
        assert!(stdout.contains("/foo/bar\n"));
        assert!(stdout.contains("/foo\n"));
    }

    #[test]
    fn v_no_output_on_failure() {
        let env = make_env(vec!["rmdir", "-v", "/nonexistent"]);
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert!(stdout.is_empty());
    }

    // ========================================================================
    // Edge cases
    // ========================================================================

    #[test]
    fn multiple_dirs_partial_failure() {
        let env = make_env(vec!["rmdir", "/good1", "/nonempty", "/good2"]);
        env.fs.mkdir("/good1").unwrap();
        env.fs.mkdir("/nonempty").unwrap();
        env.fs.mkdir("/nonempty/child").unwrap();
        env.fs.mkdir("/good2").unwrap();
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        assert!(!env.fs.exists("/good1"));
        assert!(env.fs.is_dir("/nonempty"));
        assert!(!env.fs.exists("/good2"));
    }

    #[test]
    fn relative_path() {
        let env = make_env(vec!["rmdir", "foo"]);
        env.fs.mkdir("foo").unwrap();
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert!(!env.fs.exists("foo"));
    }

    #[test]
    fn trailing_slashes_handled() {
        let env = make_env(vec!["rmdir", "-p", "/foo/bar/"]);
        env.fs.mkdir("/foo").unwrap();
        env.fs.mkdir("/foo/bar").unwrap();
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert!(!env.fs.exists("/foo/bar"));
        assert!(!env.fs.exists("/foo"));
    }

    #[test]
    fn exit_code_2_on_invalid_option() {
        let env = make_env(vec!["rmdir", "-x", "/foo"]);
        let result = bin(&env).unwrap();
        assert_eq!(2, result.code());
    }
}

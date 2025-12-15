use getopts::Options;

use crate::{Environment, Error, ExitCode, Filesystem, Stderr, Stdin, Stdout};

fn build_options() -> Options {
    let mut opts = Options::new();
    opts.optopt("m", "", "Set file permission bits (ignored)", "mode");
    opts.optflag("p", "", "Create intermediate directories as required");
    opts.optflag("v", "", "Be verbose when creating directories");
    opts
}

/// The mkdir builtin: create directories.
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
            env.stderr.write_line(&format!("mkdir: {}", e))?;
            return Ok(ExitCode::from(1));
        }
    };

    let create_parents = matches.opt_present("p");
    let verbose = matches.opt_present("v");

    if matches.free.is_empty() {
        env.stderr.write_line("usage: mkdir [-pv] directory ...")?;
        return Ok(ExitCode::from(1));
    }

    let mut exit_code: i8 = 0;

    for dir in &matches.free {
        let result = if create_parents {
            create_with_parents(env, dir, verbose)
        } else {
            create_single(env, dir, verbose)
        };

        if let Err(e) = result {
            env.stderr.write_line(&format!("mkdir: {}: {}", dir, e))?;
            exit_code = 1;
        }
    }

    Ok(ExitCode::from(exit_code))
}

/// Create a single directory without creating parents.
fn create_single<SI, SO, SE, FS>(
    env: &Environment<SI, SO, SE, FS>,
    path: &str,
    verbose: bool,
) -> Result<(), String>
where
    SI: Stdin,
    SO: Stdout,
    SE: Stderr,
    FS: Filesystem,
{
    // Check if path already exists
    if env.fs.exists(path) || env.fs.is_dir(path) {
        return Err("File exists".to_string());
    }

    // Check parent exists
    let path_obj = std::path::Path::new(path);
    if let Some(parent) = path_obj.parent() {
        let parent_str = parent.to_str().unwrap_or("");
        if !parent_str.is_empty() && parent_str != "/" && !env.fs.is_dir(parent_str) {
            if env.fs.exists(parent_str) {
                return Err("Not a directory".to_string());
            } else {
                return Err("No such file or directory".to_string());
            }
        }
    }

    env.fs.mkdir(path).map_err(|e| match e {
        Error::Io(io_err) => io_err.to_string(),
        _ => "Unknown error".to_string(),
    })?;

    if verbose {
        let _ = env.stdout.write_line(path);
    }

    Ok(())
}

/// Create a directory and all parent directories as needed.
fn create_with_parents<SI, SO, SE, FS>(
    env: &Environment<SI, SO, SE, FS>,
    path: &str,
    verbose: bool,
) -> Result<(), String>
where
    SI: Stdin,
    SO: Stdout,
    SE: Stderr,
    FS: Filesystem,
{
    // If directory already exists, success (no error with -p)
    if env.fs.is_dir(path) {
        return Ok(());
    }

    // If it exists as a file, that's an error
    if env.fs.exists(path) {
        return Err("File exists".to_string());
    }

    // Collect ancestors to create
    let path_obj = std::path::Path::new(path);
    let mut to_create = Vec::new();

    for ancestor in path_obj.ancestors() {
        let ancestor_str = ancestor.to_str().unwrap_or("");
        if ancestor_str.is_empty() || ancestor_str == "/" {
            continue;
        }
        if env.fs.is_dir(ancestor_str) {
            break;
        }
        if env.fs.exists(ancestor_str) {
            return Err(format!("{}: Not a directory", ancestor_str));
        }
        to_create.push(ancestor_str.to_string());
    }

    // Create from top to bottom
    to_create.reverse();
    for dir in to_create {
        env.fs.mkdir(&dir).map_err(|e| match e {
            Error::Io(io_err) => io_err.to_string(),
            _ => "Unknown error".to_string(),
        })?;
        if verbose {
            let _ = env.stdout.write_line(&dir);
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::{TestEnvBuilder, TestFilesystem};
    use crate::{StringStderr, StringStdin, StringStdout};

    fn make_env(
        args: Vec<&str>,
    ) -> Environment<StringStdin, StringStdout, StringStderr, TestFilesystem> {
        TestEnvBuilder::new().args(args).build()
    }

    // ========================================================================
    // Basic usage tests
    // ========================================================================

    #[test]
    fn no_args_shows_usage() {
        let env = make_env(vec!["mkdir"]);
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("usage:"));
    }

    #[test]
    fn create_single_directory() {
        let env = make_env(vec!["mkdir", "/foo"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert!(env.fs.is_dir("/foo"));
    }

    #[test]
    fn create_multiple_directories() {
        let env = make_env(vec!["mkdir", "/foo", "/bar", "/baz"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert!(env.fs.is_dir("/foo"));
        assert!(env.fs.is_dir("/bar"));
        assert!(env.fs.is_dir("/baz"));
    }

    #[test]
    fn create_nested_without_p_fails() {
        let env = make_env(vec!["mkdir", "/foo/bar/baz"]);
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("No such file or directory"));
    }

    #[test]
    fn create_existing_directory_fails() {
        let env = make_env(vec!["mkdir", "/foo"]);
        env.fs.mkdir("/foo").unwrap();
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("File exists"));
    }

    #[test]
    fn create_over_file_fails() {
        let env = make_env(vec!["mkdir", "/foo"]);
        env.fs.write_string("/foo", "content").unwrap();
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("File exists"));
    }

    // ========================================================================
    // -p flag tests
    // ========================================================================

    #[test]
    fn p_creates_intermediate_directories() {
        let env = make_env(vec!["mkdir", "-p", "/foo/bar/baz"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert!(env.fs.is_dir("/foo"));
        assert!(env.fs.is_dir("/foo/bar"));
        assert!(env.fs.is_dir("/foo/bar/baz"));
    }

    #[test]
    fn p_existing_directory_succeeds() {
        let env = make_env(vec!["mkdir", "-p", "/foo"]);
        env.fs.mkdir("/foo").unwrap();
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
    }

    #[test]
    fn p_existing_file_in_path_fails() {
        let env = make_env(vec!["mkdir", "-p", "/foo/bar/baz"]);
        env.fs.mkdir("/foo").unwrap();
        env.fs.write_string("/foo/bar", "content").unwrap();
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("Not a directory"));
    }

    #[test]
    fn p_partial_path_exists() {
        let env = make_env(vec!["mkdir", "-p", "/foo/bar/baz"]);
        env.fs.mkdir("/foo").unwrap();
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert!(env.fs.is_dir("/foo/bar"));
        assert!(env.fs.is_dir("/foo/bar/baz"));
    }

    // ========================================================================
    // -v flag tests
    // ========================================================================

    #[test]
    fn v_prints_created_directory() {
        let env = make_env(vec!["mkdir", "-v", "/foo"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert!(stdout.contains("/foo"));
    }

    #[test]
    fn pv_prints_all_created_directories() {
        let env = make_env(vec!["mkdir", "-pv", "/foo/bar/baz"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert!(stdout.contains("/foo\n"));
        assert!(stdout.contains("/foo/bar\n"));
        assert!(stdout.contains("/foo/bar/baz\n"));
    }

    #[test]
    fn pv_existing_directory_no_output() {
        let env = make_env(vec!["mkdir", "-pv", "/foo"]);
        env.fs.mkdir("/foo").unwrap();
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert!(stdout.is_empty());
    }

    // ========================================================================
    // Edge cases
    // ========================================================================

    #[test]
    fn create_subdirectory_of_existing() {
        let env = make_env(vec!["mkdir", "/foo/bar"]);
        env.fs.mkdir("/foo").unwrap();
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert!(env.fs.is_dir("/foo/bar"));
    }

    #[test]
    fn parent_is_file_fails() {
        let env = make_env(vec!["mkdir", "/foo/bar"]);
        env.fs.write_string("/foo", "content").unwrap();
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("Not a directory"));
    }

    #[test]
    fn multiple_dirs_partial_failure() {
        let env = make_env(vec!["mkdir", "/good", "/bad/nested", "/also_good"]);
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        assert!(env.fs.is_dir("/good"));
        assert!(!env.fs.is_dir("/bad/nested"));
        assert!(env.fs.is_dir("/also_good"));
    }

    #[test]
    fn relative_path_single_component() {
        let env = make_env(vec!["mkdir", "foo"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert!(env.fs.is_dir("foo"));
    }

    #[test]
    fn relative_path_nested_with_p() {
        let env = make_env(vec!["mkdir", "-p", "foo/bar/baz"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert!(env.fs.is_dir("foo"));
        assert!(env.fs.is_dir("foo/bar"));
        assert!(env.fs.is_dir("foo/bar/baz"));
    }

    #[test]
    fn m_flag_accepted_but_ignored() {
        let env = make_env(vec!["mkdir", "-m", "755", "/foo"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert!(env.fs.is_dir("/foo"));
    }

    #[test]
    fn m_flag_with_p() {
        let env = make_env(vec!["mkdir", "-p", "-m", "700", "/foo/bar"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert!(env.fs.is_dir("/foo"));
        assert!(env.fs.is_dir("/foo/bar"));
    }
}

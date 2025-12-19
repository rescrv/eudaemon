use std::collections::VecDeque;

use getopts::Options;

use crate::{
    Environment, Error, ExitCode, FileType, Filesystem, FsError, StdioIn, StdioOut,
    fs_error_message,
};

fn build_options() -> Options {
    let mut opts = Options::new();
    opts.optflag("d", "", "Attempt to remove directories as well as files");
    opts.optflag(
        "f",
        "",
        "Force removal without prompting, ignore nonexistent files",
    );
    opts.optflag("r", "", "Remove file hierarchies recursively");
    opts.optflag("R", "", "Equivalent to -r");
    opts.optflag("v", "", "Be verbose, showing files as they are removed");
    opts
}

/// The rm builtin: remove files and directories.
pub fn bin<SI, SO, SE, FS>(env: &Environment<SI, SO, SE, FS>) -> Result<ExitCode, Error>
where
    SI: StdioIn,
    SO: StdioOut,
    SE: StdioOut,
    FS: Filesystem,
{
    let prog_name = env
        .args
        .first()
        .map(|s| s.rsplit('/').next().unwrap_or(s.as_str()).to_string())
        .unwrap_or_else(|| "rm".to_string());

    // Handle unlink mode
    if prog_name == "unlink" {
        return unlink_mode(env);
    }

    let opts_def = build_options();

    let matches = match opts_def.parse(&env.args[1..]) {
        Ok(m) => m,
        Err(e) => {
            env.stderr.write_line(&format!("rm: {}", e))?;
            return Ok(ExitCode::from(64)); // EX_USAGE
        }
    };

    let force = matches.opt_present("f");
    let recursive = matches.opt_present("r") || matches.opt_present("R");
    let remove_dirs = matches.opt_present("d") || recursive;
    let verbose = matches.opt_present("v");

    if matches.free.is_empty() {
        if force {
            return Ok(ExitCode::from(0));
        }
        env.stderr
            .write_line("usage: rm [-f | -i] [-dRrv] file ...")?;
        return Ok(ExitCode::from(64));
    }

    // Filter out invalid paths (/, ., ..)
    let mut args: Vec<String> = Vec::new();
    let mut eval = 0i8;

    for path in &matches.free {
        if is_slash(path) {
            env.stderr.write_line("rm: \"/\" may not be removed")?;
            eval = 1;
        } else if is_dot_or_dotdot(path) {
            env.stderr
                .write_line("rm: \".\" and \"..\" may not be removed")?;
            eval = 1;
        } else {
            args.push(path.clone());
        }
    }

    let opts = RmOptions {
        force,
        recursive,
        remove_dirs,
        verbose,
    };

    for path in &args {
        if let Err(e) = remove_path(env, path, &opts) {
            env.stderr.write_line(&format!("rm: {}: {}", path, e))?;
            eval = 1;
        }
    }

    Ok(ExitCode::from(eval))
}

/// Options for the rm operation.
struct RmOptions {
    force: bool,
    recursive: bool,
    remove_dirs: bool,
    verbose: bool,
}

/// Remove a single path (file or directory tree).
fn remove_path<SI, SO, SE, FS>(
    env: &Environment<SI, SO, SE, FS>,
    path: &str,
    opts: &RmOptions,
) -> Result<(), String>
where
    SI: StdioIn,
    SO: StdioOut,
    SE: StdioOut,
    FS: Filesystem,
{
    // Strip trailing slashes for consistency (but preserve "/" detection already done)
    let path = if path.len() > 1 {
        path.trim_end_matches('/')
    } else {
        path
    };

    // Check if path exists
    let entry = match env.fs.lstat(path) {
        Ok(e) => e,
        Err(e) => {
            if opts.force {
                return Ok(());
            }
            return Err(fs_error_message(&e));
        }
    };

    match entry.file_type {
        FileType::Directory => {
            if !opts.remove_dirs {
                return Err("is a directory".to_string());
            }
            if opts.recursive {
                remove_tree(env, path, opts)
            } else {
                remove_single_dir(env, path, opts)
            }
        }
        _ => remove_file(env, path, opts),
    }
}

/// Remove a single file or symlink.
fn remove_file<SI, SO, SE, FS>(
    env: &Environment<SI, SO, SE, FS>,
    path: &str,
    opts: &RmOptions,
) -> Result<(), String>
where
    SI: StdioIn,
    SO: StdioOut,
    SE: StdioOut,
    FS: Filesystem,
{
    env.fs.unlink(path).map_err(|e| {
        if opts.force && is_not_found(&e) {
            return String::new();
        }
        fs_error_message(&e)
    })?;

    if opts.verbose {
        env.stdout.write_line(path).ok();
    }

    Ok(())
}

/// Remove a single empty directory (non-recursive).
fn remove_single_dir<SI, SO, SE, FS>(
    env: &Environment<SI, SO, SE, FS>,
    path: &str,
    opts: &RmOptions,
) -> Result<(), String>
where
    SI: StdioIn,
    SO: StdioOut,
    SE: StdioOut,
    FS: Filesystem,
{
    env.fs.rmdir(path).map_err(|e| fs_error_message(&e))?;

    if opts.verbose {
        env.stdout.write_line(path).ok();
    }

    Ok(())
}

/// Remove a directory tree recursively.
fn remove_tree<SI, SO, SE, FS>(
    env: &Environment<SI, SO, SE, FS>,
    root: &str,
    opts: &RmOptions,
) -> Result<(), String>
where
    SI: StdioIn,
    SO: StdioOut,
    SE: StdioOut,
    FS: Filesystem,
{
    // Post-order traversal: collect all paths, then remove in reverse order
    // (deepest first, so directories are empty when we try to remove them)
    let mut to_remove: Vec<(String, FileType)> = Vec::new();
    let mut queue: VecDeque<String> = VecDeque::new();
    queue.push_back(root.to_string());

    while let Some(path) = queue.pop_front() {
        let entry = match env.fs.lstat(&path) {
            Ok(e) => e,
            Err(e) => {
                if opts.force && is_not_found(&e) {
                    continue;
                }
                return Err(format!("{}: {}", path, fs_error_message(&e)));
            }
        };

        to_remove.push((path.clone(), entry.file_type));

        if entry.file_type == FileType::Directory {
            let children = env.fs.read_dir(&path).map_err(|e| {
                if opts.force && is_not_found(&e) {
                    return String::new();
                }
                format!("{}: {}", path, fs_error_message(&e))
            })?;

            for (name, _child_entry) in children {
                if name == "." || name == ".." {
                    continue;
                }
                let child_path = if path == "/" {
                    format!("/{}", name)
                } else {
                    format!("{}/{}", path, name)
                };
                queue.push_back(child_path);
            }
        }
    }

    // Remove in reverse order (post-order)
    let mut last_error: Option<String> = None;
    for (path, file_type) in to_remove.into_iter().rev() {
        let result = if file_type == FileType::Directory {
            env.fs.rmdir(&path)
        } else {
            env.fs.unlink(&path)
        };

        match result {
            Ok(()) => {
                if opts.verbose {
                    env.stdout.write_line(&path).ok();
                }
            }
            Err(e) => {
                if opts.force && is_not_found(&e) {
                    continue;
                }
                let msg = format!("{}: {}", path, fs_error_message(&e));
                env.stderr.write_line(&format!("rm: {}", msg)).ok();
                last_error = Some(msg);
            }
        }
    }

    if let Some(e) = last_error {
        Err(e)
    } else {
        Ok(())
    }
}

/// Handle the unlink command mode.
fn unlink_mode<SI, SO, SE, FS>(env: &Environment<SI, SO, SE, FS>) -> Result<ExitCode, Error>
where
    SI: StdioIn,
    SO: StdioOut,
    SE: StdioOut,
    FS: Filesystem,
{
    let args: Vec<&str> = env.args.iter().skip(1).map(|s| s.as_str()).collect();

    let file = match args.as_slice() {
        [file] => *file,
        ["--", file] => *file,
        _ => {
            env.stderr.write_line("usage: unlink [--] file")?;
            return Ok(ExitCode::from(64));
        }
    };

    // unlink does not work on directories
    match env.fs.lstat(file) {
        Ok(entry) => {
            if entry.file_type == FileType::Directory {
                env.stderr
                    .write_line(&format!("unlink: {}: is a directory", file))?;
                return Ok(ExitCode::from(1));
            }
        }
        Err(e) => {
            env.stderr
                .write_line(&format!("unlink: {}: {}", file, fs_error_message(&e)))?;
            return Ok(ExitCode::from(1));
        }
    }

    if let Err(e) = env.fs.unlink(file) {
        env.stderr
            .write_line(&format!("unlink: {}: {}", file, fs_error_message(&e)))?;
        return Ok(ExitCode::from(1));
    }

    Ok(ExitCode::from(0))
}

/// Check if a path is exactly "/".
fn is_slash(path: &str) -> bool {
    path == "/"
}

/// Check if the final component of a path is "." or "..".
fn is_dot_or_dotdot(path: &str) -> bool {
    let component = path.rsplit('/').next().unwrap_or(path);
    component == "." || component == ".."
}

/// Check if an error is "not found".
fn is_not_found(e: &FsError) -> bool {
    matches!(e, FsError::Io(io_err) if io_err.kind() == std::io::ErrorKind::NotFound)
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
        let env = make_env(vec!["rm"]);
        let result = bin(&env).unwrap();
        assert_eq!(64, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("usage:"));
    }

    #[test]
    fn no_args_with_f_succeeds() {
        let env = make_env(vec!["rm", "-f"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
    }

    #[test]
    fn remove_single_file() {
        let env = make_env(vec!["rm", "/foo"]);
        env.fs.write_string("/foo", "content").unwrap();
        assert!(env.fs.exists("/foo"));
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert!(!env.fs.exists("/foo"));
    }

    #[test]
    fn remove_multiple_files() {
        let env = make_env(vec!["rm", "/foo", "/bar", "/baz"]);
        env.fs.write_string("/foo", "a").unwrap();
        env.fs.write_string("/bar", "b").unwrap();
        env.fs.write_string("/baz", "c").unwrap();
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert!(!env.fs.exists("/foo"));
        assert!(!env.fs.exists("/bar"));
        assert!(!env.fs.exists("/baz"));
    }

    #[test]
    fn remove_nonexistent_fails() {
        let env = make_env(vec!["rm", "/nonexistent"]);
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("No such file or directory"));
    }

    #[test]
    fn remove_nonexistent_with_f_succeeds() {
        let env = make_env(vec!["rm", "-f", "/nonexistent"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
    }

    #[test]
    fn remove_directory_without_d_fails() {
        let env = make_env(vec!["rm", "/foo"]);
        env.fs.mkdir("/foo").unwrap();
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("is a directory"));
        assert!(env.fs.is_dir("/foo"));
    }

    // ========================================================================
    // -d flag tests
    // ========================================================================

    #[test]
    fn d_removes_empty_directory() {
        let env = make_env(vec!["rm", "-d", "/foo"]);
        env.fs.mkdir("/foo").unwrap();
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert!(!env.fs.exists("/foo"));
    }

    #[test]
    fn d_fails_on_nonempty_directory() {
        let env = make_env(vec!["rm", "-d", "/foo"]);
        env.fs.mkdir("/foo").unwrap();
        env.fs.write_string("/foo/bar", "content").unwrap();
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("Directory not empty"));
        assert!(env.fs.is_dir("/foo"));
    }

    // ========================================================================
    // -r/-R flag tests
    // ========================================================================

    #[test]
    fn r_removes_directory_tree() {
        let env = make_env(vec!["rm", "-r", "/foo"]);
        env.fs.mkdir("/foo").unwrap();
        env.fs.mkdir("/foo/bar").unwrap();
        env.fs.write_string("/foo/bar/baz", "content").unwrap();
        env.fs.write_string("/foo/qux", "content").unwrap();
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert!(!env.fs.exists("/foo"));
        assert!(!env.fs.exists("/foo/bar"));
        assert!(!env.fs.exists("/foo/bar/baz"));
        assert!(!env.fs.exists("/foo/qux"));
    }

    #[test]
    fn capital_r_removes_directory_tree() {
        let env = make_env(vec!["rm", "-R", "/foo"]);
        env.fs.mkdir("/foo").unwrap();
        env.fs.write_string("/foo/bar", "content").unwrap();
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert!(!env.fs.exists("/foo"));
    }

    #[test]
    fn r_removes_single_file() {
        let env = make_env(vec!["rm", "-r", "/foo"]);
        env.fs.write_string("/foo", "content").unwrap();
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert!(!env.fs.exists("/foo"));
    }

    #[test]
    fn r_deep_tree() {
        let env = make_env(vec!["rm", "-r", "/a"]);
        env.fs.mkdir("/a").unwrap();
        env.fs.mkdir("/a/b").unwrap();
        env.fs.mkdir("/a/b/c").unwrap();
        env.fs.mkdir("/a/b/c/d").unwrap();
        env.fs.write_string("/a/b/c/d/file", "content").unwrap();
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert!(!env.fs.exists("/a"));
    }

    #[test]
    fn rf_nonexistent_succeeds() {
        let env = make_env(vec!["rm", "-rf", "/nonexistent"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
    }

    // ========================================================================
    // -v flag tests
    // ========================================================================

    #[test]
    fn v_prints_removed_file() {
        let env = make_env(vec!["rm", "-v", "/foo"]);
        env.fs.write_string("/foo", "content").unwrap();
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert!(stdout.contains("/foo"));
    }

    #[test]
    fn rv_prints_all_removed() {
        let env = make_env(vec!["rm", "-rv", "/foo"]);
        env.fs.mkdir("/foo").unwrap();
        env.fs.write_string("/foo/bar", "content").unwrap();
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert!(stdout.contains("/foo/bar"));
        assert!(stdout.contains("/foo\n"));
    }

    // ========================================================================
    // Safety checks
    // ========================================================================

    #[test]
    fn reject_root() {
        let env = make_env(vec!["rm", "-rf", "/"]);
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("\"/\" may not be removed"));
    }

    #[test]
    fn reject_dot() {
        let env = make_env(vec!["rm", "-rf", "."]);
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("\".\" and \"..\" may not be removed"));
    }

    #[test]
    fn reject_dotdot() {
        let env = make_env(vec!["rm", "-rf", ".."]);
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("\".\" and \"..\" may not be removed"));
    }

    #[test]
    fn reject_path_ending_in_dot() {
        let env = make_env(vec!["rm", "-rf", "/foo/."]);
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
    }

    #[test]
    fn reject_path_ending_in_dotdot() {
        let env = make_env(vec!["rm", "-rf", "/foo/.."]);
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
    }

    // ========================================================================
    // unlink mode tests
    // ========================================================================

    #[test]
    fn unlink_removes_file() {
        let env = make_env(vec!["unlink", "/foo"]);
        env.fs.write_string("/foo", "content").unwrap();
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert!(!env.fs.exists("/foo"));
    }

    #[test]
    fn unlink_with_double_dash() {
        let env = make_env(vec!["unlink", "--", "/foo"]);
        env.fs.write_string("/foo", "content").unwrap();
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert!(!env.fs.exists("/foo"));
    }

    #[test]
    fn unlink_dash_filename() {
        let env = make_env(vec!["unlink", "-foo"]);
        env.fs.write_string("-foo", "content").unwrap();
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert!(!env.fs.exists("-foo"));
    }

    #[test]
    fn unlink_rejects_directory() {
        let env = make_env(vec!["unlink", "/foo"]);
        env.fs.mkdir("/foo").unwrap();
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("is a directory"));
        assert!(env.fs.is_dir("/foo"));
    }

    #[test]
    fn unlink_no_args_shows_usage() {
        let env = make_env(vec!["unlink"]);
        let result = bin(&env).unwrap();
        assert_eq!(64, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("usage:"));
    }

    #[test]
    fn unlink_too_many_args_shows_usage() {
        let env = make_env(vec!["unlink", "foo", "bar"]);
        let result = bin(&env).unwrap();
        assert_eq!(64, result.code());
    }

    #[test]
    fn unlink_nonexistent_fails() {
        let env = make_env(vec!["unlink", "/nonexistent"]);
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
    }

    // ========================================================================
    // Edge cases
    // ========================================================================

    #[test]
    fn multiple_files_partial_failure() {
        let env = make_env(vec!["rm", "/good", "/nonexistent", "/also_good"]);
        env.fs.write_string("/good", "a").unwrap();
        env.fs.write_string("/also_good", "b").unwrap();
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        assert!(!env.fs.exists("/good"));
        assert!(!env.fs.exists("/also_good"));
    }

    #[test]
    fn trailing_slashes_handled() {
        let env = make_env(vec!["rm", "-r", "/foo/"]);
        env.fs.mkdir("/foo").unwrap();
        env.fs.write_string("/foo/bar", "content").unwrap();
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert!(!env.fs.exists("/foo"));
    }

    #[test]
    fn relative_path() {
        let env = make_env(vec!["rm", "foo"]);
        env.fs.write_string("foo", "content").unwrap();
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert!(!env.fs.exists("foo"));
    }

    #[test]
    fn remove_symlink_not_target() {
        let env = make_env(vec!["rm", "/link"]);
        env.fs.write_string("/target", "content").unwrap();
        env.fs.symlink("/target", "/link").unwrap();
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert!(!env.fs.exists("/link"));
        assert!(env.fs.exists("/target"));
    }
}

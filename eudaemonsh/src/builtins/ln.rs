//! The ln utility: create links between files.

use getopts::Options;

use crate::{
    Environment, Error, ExitCode, FileType, Filesystem, Stderr, Stdin, Stdout, fs_error_message,
};

fn build_options() -> Options {
    let mut opts = Options::new();
    opts.optflag("F", "", "Remove empty directories (with -s and -f/-i)");
    opts.optflag("L", "", "Create hard link to symlink target (default)");
    opts.optflag("P", "", "Create hard link to symlink itself");
    opts.optflag("f", "", "Force: unlink existing target files");
    opts.optflag("h", "", "Do not follow symlinks (same as -n)");
    opts.optflag("n", "", "Do not follow symlinks (same as -h)");
    opts.optflag("s", "", "Create symbolic link");
    opts.optflag("v", "", "Verbose: print files as they are processed");
    opts
}

/// Configuration for the ln command.
struct LnConfig {
    /// Create symbolic links instead of hard links.
    symbolic: bool,
    /// Force: remove existing destination files.
    force: bool,
    /// Do not follow symlinks when checking target.
    no_follow: bool,
    /// Remove empty directories when target is a directory (with -s).
    remove_dirs: bool,
    /// Verbose output.
    verbose: bool,
    /// Follow symlinks when creating hard links (default true, -L).
    /// When false (-P), create hard link to symlink itself.
    follow_symlinks: bool,
}

/// The ln builtin: create links between files.
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
            env.stderr.write_line(&format!("ln: {}", e))?;
            return Ok(ExitCode::from(1));
        }
    };

    let mut config = LnConfig {
        symbolic: matches.opt_present("s"),
        force: matches.opt_present("f"),
        no_follow: matches.opt_present("h") || matches.opt_present("n"),
        remove_dirs: matches.opt_present("F"),
        verbose: matches.opt_present("v"),
        follow_symlinks: !matches.opt_present("P"),
    };

    // -L cancels -P
    if matches.opt_present("L") {
        config.follow_symlinks = true;
    }

    // -F only applies to symbolic links
    if !config.symbolic {
        config.remove_dirs = false;
    }

    // -F implies -f if neither -f nor -i is specified
    // (We don't support -i as it requires interactive input)
    if config.remove_dirs {
        config.force = true;
    }

    if matches.free.is_empty() {
        env.stderr.write_line(
            "usage: ln [-s [-F] | -L | -P] [-fhnv] source_file [target_file]\n       \
             ln [-s [-F] | -L | -P] [-fhnv] source_file ... target_dir",
        )?;
        return Ok(ExitCode::from(1));
    }

    match matches.free.len() {
        1 => {
            // ln source -> link in current directory with same name
            linkit(env, &config, &matches.free[0], ".", true)
        }
        2 => {
            // ln source target
            linkit(env, &config, &matches.free[0], &matches.free[1], false)
        }
        _ => {
            // ln source1 source2 ... target_dir
            let target_dir = &matches.free[matches.free.len() - 1];

            // Check if target is a symlink and -h is set
            if config.no_follow
                && env
                    .fs
                    .lstat(target_dir)
                    .map(|e| e.file_type == FileType::Symlink)
                    .unwrap_or(false)
            {
                env.stderr
                    .write_line(&format!("ln: {}: Not a directory", target_dir))?;
                return Ok(ExitCode::from(1));
            }

            // Target must be a directory
            match env.fs.stat(target_dir) {
                Ok(entry) if entry.file_type == FileType::Directory => {}
                Ok(_) => {
                    env.stderr.write_line(
                        "usage: ln [-s [-F] | -L | -P] [-fhnv] source_file [target_file]\n       \
                         ln [-s [-F] | -L | -P] [-fhnv] source_file ... target_dir",
                    )?;
                    return Ok(ExitCode::from(1));
                }
                Err(_) => {
                    env.stderr
                        .write_line(&format!("ln: {}: No such file or directory", target_dir))?;
                    return Ok(ExitCode::from(1));
                }
            }

            let mut exit_code: i8 = 0;
            for source in &matches.free[..matches.free.len() - 1] {
                let result = linkit(env, &config, source, target_dir, true);
                if let Ok(code) = result {
                    if code.code() != 0 {
                        exit_code = 1;
                    }
                } else {
                    exit_code = 1;
                }
            }
            Ok(ExitCode::from(exit_code))
        }
    }
}

/// Create a link from source to target.
///
/// If `is_target_dir` is true, the link will be created inside target with the
/// basename of source. Otherwise, target is the link name directly.
fn linkit<SI, SO, SE, FS>(
    env: &Environment<SI, SO, SE, FS>,
    config: &LnConfig,
    source: &str,
    target: &str,
    is_target_dir: bool,
) -> Result<ExitCode, Error>
where
    SI: Stdin,
    SO: Stdout,
    SE: Stderr,
    FS: Filesystem,
{
    // For hard links, source must exist
    if !config.symbolic {
        let stat_result = if config.follow_symlinks {
            env.fs.stat(source)
        } else {
            env.fs.lstat(source)
        };

        match stat_result {
            Ok(entry) if entry.file_type == FileType::Directory => {
                env.stderr
                    .write_line(&format!("ln: {}: Is a directory", source))?;
                return Ok(ExitCode::from(1));
            }
            Ok(_) => {}
            Err(_) => {
                env.stderr
                    .write_line(&format!("ln: {}: No such file or directory", source))?;
                return Ok(ExitCode::from(1));
            }
        }
    }

    // Determine the actual target path
    let target_path = compute_target_path(env, config, source, target, is_target_dir)?;

    // Check if target exists
    let target_exists = env.fs.lstat(&target_path).is_ok();

    // For hard links, check if source and target are the same directory entry
    if !config.symbolic && target_exists && same_dirent(env, source, &target_path) {
        env.stderr.write_line(&format!(
            "ln: {} and {} are the same directory entry",
            source, target_path
        ))?;
        return Ok(ExitCode::from(1));
    }

    // Handle existing target
    if target_exists {
        if config.force {
            let target_entry = env.fs.lstat(&target_path)?;
            if config.remove_dirs && target_entry.file_type == FileType::Directory {
                if let Err(e) = env.fs.rmdir(&target_path) {
                    env.stderr.write_line(&format!(
                        "ln: {}: {}",
                        target_path,
                        fs_error_message(&e)
                    ))?;
                    return Ok(ExitCode::from(1));
                }
            } else if let Err(e) = env.fs.unlink(&target_path) {
                env.stderr
                    .write_line(&format!("ln: {}: {}", target_path, fs_error_message(&e)))?;
                return Ok(ExitCode::from(1));
            }
        } else {
            env.stderr
                .write_line(&format!("ln: {}: File exists", target_path))?;
            return Ok(ExitCode::from(1));
        }
    }

    // Create the link
    let result = if config.symbolic {
        env.fs.symlink(source, &target_path)
    } else {
        env.fs.link(source, &target_path)
    };

    if let Err(e) = result {
        env.stderr
            .write_line(&format!("ln: {}: {}", target_path, fs_error_message(&e)))?;
        return Ok(ExitCode::from(1));
    }

    if config.verbose {
        let link_char = if config.symbolic { '-' } else { '=' };
        env.stdout
            .write_line(&format!("{} {}> {}", target_path, link_char, source))?;
    }

    Ok(ExitCode::from(0))
}

/// Compute the target path for the link.
fn compute_target_path<SI, SO, SE, FS>(
    env: &Environment<SI, SO, SE, FS>,
    config: &LnConfig,
    source: &str,
    target: &str,
    is_target_dir: bool,
) -> Result<String, Error>
where
    SI: Stdin,
    SO: Stdout,
    SE: Stderr,
    FS: Filesystem,
{
    // Check if we should append the source basename to target
    let should_append = if is_target_dir {
        true
    } else {
        // Check if target ends with / or is "." or ends with "/."
        let p = target
            .rfind('/')
            .map(|i| &target[i + 1..])
            .unwrap_or(target);
        if p.is_empty() || p == "." {
            true
        } else if !config.remove_dirs {
            // Check if target is a directory
            if config.no_follow {
                env.fs
                    .lstat(target)
                    .map(|e| e.file_type == FileType::Directory)
                    .unwrap_or(false)
            } else {
                env.fs
                    .stat(target)
                    .map(|e| e.file_type == FileType::Directory)
                    .unwrap_or(false)
            }
        } else {
            false
        }
    };

    if should_append {
        let basename = source
            .rfind('/')
            .map(|i| &source[i + 1..])
            .unwrap_or(source);
        if target == "." {
            Ok(basename.to_string())
        } else {
            Ok(format!("{}/{}", target.trim_end_matches('/'), basename))
        }
    } else {
        Ok(target.to_string())
    }
}

/// Check if two paths refer to the same directory entry by comparing device and inode.
fn same_dirent<SI, SO, SE, FS>(env: &Environment<SI, SO, SE, FS>, path1: &str, path2: &str) -> bool
where
    SI: Stdin,
    SO: Stdout,
    SE: Stderr,
    FS: Filesystem,
{
    if path1 == path2 {
        return true;
    }

    // Get the filename components - must match for same dirent
    let file1 = path1.rfind('/').map(|i| &path1[i + 1..]).unwrap_or(path1);
    let file2 = path2.rfind('/').map(|i| &path2[i + 1..]).unwrap_or(path2);

    if file1 != file2 {
        return false;
    }

    // Get the directory components
    let dir1 = path1.rfind('/').map(|i| &path1[..i]).unwrap_or(".");
    let dir2 = path2.rfind('/').map(|i| &path2[..i]).unwrap_or(".");

    let dir1 = if dir1.is_empty() { "/" } else { dir1 };
    let dir2 = if dir2.is_empty() { "/" } else { dir2 };

    // Compare device and inode of the directories
    let stat1 = env.fs.stat(dir1);
    let stat2 = env.fs.stat(dir2);

    match (stat1, stat2) {
        (Ok(s1), Ok(s2)) => s1.dev == s2.dev && s1.ino == s2.ino,
        _ => false,
    }
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
        let env = make_env(vec!["ln"]);
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("usage:"));
    }

    #[test]
    fn create_hard_link() {
        let env = make_env(vec!["ln", "/source", "/target"]);
        env.fs.write_string("/source", "content").unwrap();
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert!(env.fs.exists("/target"));
        assert_eq!(
            env.fs.read_to_string("/target").unwrap(),
            env.fs.read_to_string("/source").unwrap()
        );
    }

    #[test]
    fn create_symbolic_link() {
        let env = make_env(vec!["ln", "-s", "/source", "/target"]);
        env.fs.write_string("/source", "content").unwrap();
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let entry = env.fs.lstat("/target").unwrap();
        assert_eq!(FileType::Symlink, entry.file_type);
        assert_eq!("/source", env.fs.readlink("/target").unwrap());
    }

    #[test]
    fn symlink_to_nonexistent_source() {
        let env = make_env(vec!["ln", "-s", "/nonexistent", "/target"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let entry = env.fs.lstat("/target").unwrap();
        assert_eq!(FileType::Symlink, entry.file_type);
    }

    #[test]
    fn hard_link_to_nonexistent_fails() {
        let env = make_env(vec!["ln", "/nonexistent", "/target"]);
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("No such file or directory"));
    }

    #[test]
    fn hard_link_to_directory_fails() {
        let env = make_env(vec!["ln", "/dir", "/target"]);
        env.fs.mkdir("/dir").unwrap();
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("Is a directory"));
    }

    // ========================================================================
    // -f flag tests
    // ========================================================================

    #[test]
    fn force_overwrites_existing() {
        let env = make_env(vec!["ln", "-sf", "/source", "/target"]);
        env.fs.write_string("/source", "new content").unwrap();
        env.fs.write_string("/target", "old content").unwrap();
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let entry = env.fs.lstat("/target").unwrap();
        assert_eq!(FileType::Symlink, entry.file_type);
    }

    #[test]
    fn without_force_existing_fails() {
        let env = make_env(vec!["ln", "-s", "/source", "/target"]);
        env.fs.write_string("/source", "content").unwrap();
        env.fs.write_string("/target", "existing").unwrap();
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("File exists"));
    }

    // ========================================================================
    // -v flag tests
    // ========================================================================

    #[test]
    fn verbose_prints_link_info() {
        let env = make_env(vec!["ln", "-sv", "/source", "/target"]);
        env.fs.write_string("/source", "content").unwrap();
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert!(stdout.contains("/target"));
        assert!(stdout.contains("/source"));
        assert!(stdout.contains("->"));
    }

    #[test]
    fn verbose_hard_link_shows_equals() {
        let env = make_env(vec!["ln", "-v", "/source", "/target"]);
        env.fs.write_string("/source", "content").unwrap();
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert!(stdout.contains("=>"));
    }

    // ========================================================================
    // Target directory tests
    // ========================================================================

    #[test]
    fn link_into_directory() {
        let env = make_env(vec!["ln", "-s", "/source", "/dir"]);
        env.fs.write_string("/source", "content").unwrap();
        env.fs.mkdir("/dir").unwrap();
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert!(env.fs.lstat("/dir/source").is_ok());
    }

    #[test]
    fn multiple_sources_to_directory() {
        let env = make_env(vec!["ln", "-s", "/a", "/b", "/c", "/dir"]);
        env.fs.write_string("/a", "a").unwrap();
        env.fs.write_string("/b", "b").unwrap();
        env.fs.write_string("/c", "c").unwrap();
        env.fs.mkdir("/dir").unwrap();
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert!(env.fs.lstat("/dir/a").is_ok());
        assert!(env.fs.lstat("/dir/b").is_ok());
        assert!(env.fs.lstat("/dir/c").is_ok());
    }

    #[test]
    fn multiple_sources_to_non_directory_fails() {
        let env = make_env(vec!["ln", "-s", "/a", "/b", "/target"]);
        env.fs.write_string("/a", "a").unwrap();
        env.fs.write_string("/b", "b").unwrap();
        env.fs.write_string("/target", "file").unwrap();
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
    }

    #[test]
    fn single_source_creates_link_in_current_dir() {
        let env = make_env(vec!["ln", "-s", "/path/to/source"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert!(env.fs.lstat("source").is_ok());
    }

    // ========================================================================
    // -h/-n flag tests
    // ========================================================================

    #[test]
    fn no_follow_treats_symlink_as_file() {
        let env = make_env(vec!["ln", "-shf", "/new_source", "/link"]);
        env.fs.write_string("/old_source", "old").unwrap();
        env.fs.mkdir("/dir").unwrap();
        env.fs.symlink("/dir", "/link").unwrap();
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let target = env.fs.readlink("/link").unwrap();
        println!("target: {:?}", target);
        assert_eq!("/new_source", target);
    }

    // ========================================================================
    // Same directory entry tests
    // ========================================================================

    #[test]
    fn hard_link_to_self_fails() {
        let env = make_env(vec!["ln", "/file", "/file"]);
        env.fs.write_string("/file", "content").unwrap();
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("same directory entry"));
    }

    // ========================================================================
    // -F flag tests
    // ========================================================================

    #[test]
    fn f_flag_removes_empty_directory() {
        let env = make_env(vec!["ln", "-sF", "/source", "/emptydir"]);
        env.fs.write_string("/source", "content").unwrap();
        env.fs.mkdir("/emptydir").unwrap();
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let entry = env.fs.lstat("/emptydir").unwrap();
        assert_eq!(FileType::Symlink, entry.file_type);
    }

    #[test]
    fn f_flag_fails_on_nonempty_directory() {
        let env = make_env(vec!["ln", "-sF", "/source", "/dir"]);
        env.fs.write_string("/source", "content").unwrap();
        env.fs.mkdir("/dir").unwrap();
        env.fs.write_string("/dir/file", "inside").unwrap();
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
    }

    // ========================================================================
    // Edge cases
    // ========================================================================

    #[test]
    fn link_with_trailing_slash_target() {
        let env = make_env(vec!["ln", "-s", "/source", "/dir/"]);
        env.fs.write_string("/source", "content").unwrap();
        env.fs.mkdir("/dir").unwrap();
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert!(env.fs.lstat("/dir/source").is_ok());
    }

    #[test]
    fn link_to_dot_creates_in_current() {
        let env = make_env(vec!["ln", "-s", "/path/source", "."]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert!(env.fs.lstat("source").is_ok());
    }
}

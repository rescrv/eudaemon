//! The mv utility: move files.

use getopts::Options;

use crate::{Environment, Error, ExitCode, FileType, Filesystem, Stderr, Stdin, Stdout};

fn build_options() -> Options {
    let mut opts = Options::new();
    opts.optflag("f", "", "Do not prompt for confirmation before overwriting");
    opts.optflag(
        "h",
        "",
        "If target is a symlink to a directory, do not follow it",
    );
    opts.optflag("i", "", "Prompt before overwriting (not supported)");
    opts.optflag("n", "", "Do not overwrite an existing file");
    opts.optflag("v", "", "Verbose: show files after they are moved");
    opts
}

/// Configuration for the mv command.
#[allow(dead_code)]
struct MvConfig {
    /// Do not prompt for confirmation before overwriting (-f).
    /// This is a no-op since overwriting is the default behavior,
    /// but we accept it for compatibility with users accustomed to `mv -f`.
    force: bool,
    /// Do not follow symlinks to directories (-h).
    no_follow: bool,
    /// Do not overwrite existing files (-n).
    no_clobber: bool,
    /// Verbose output (-v).
    verbose: bool,
}

/// Extract a user-friendly message from an Error.
fn io_error_message(e: &Error) -> String {
    match e {
        Error::Io(io_err) => match io_err.kind() {
            std::io::ErrorKind::NotFound => "No such file or directory".to_string(),
            std::io::ErrorKind::DirectoryNotEmpty => "Directory not empty".to_string(),
            std::io::ErrorKind::NotADirectory => "Not a directory".to_string(),
            std::io::ErrorKind::IsADirectory => "Is a directory".to_string(),
            std::io::ErrorKind::PermissionDenied => "Permission denied".to_string(),
            std::io::ErrorKind::AlreadyExists => "File exists".to_string(),
            _ => io_err.to_string(),
        },
        _ => "Unknown error".to_string(),
    }
}

/// The mv builtin: move files.
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
            env.stderr.write_line(&format!("mv: {}", e))?;
            return Ok(ExitCode::from(1));
        }
    };

    // Handle flag precedence: last one wins
    // -f, -i, -n are mutually exclusive; last specified wins
    // Use opt_positions to determine which was specified last
    let f_positions = matches.opt_positions("f");
    let i_positions = matches.opt_positions("i");
    let n_positions = matches.opt_positions("n");

    let last_f = f_positions.last().copied();
    let last_i = i_positions.last().copied();
    let last_n = n_positions.last().copied();

    // Determine which flag was last (if any)
    let (force, no_clobber) = match (last_f, last_i, last_n) {
        (Some(f), Some(i), Some(n)) => {
            if f > i && f > n {
                (true, false)
            } else if n > f && n > i {
                (false, true)
            } else {
                (false, false)
            }
        }
        (Some(f), Some(i), None) => {
            if f > i {
                (true, false)
            } else {
                (false, false)
            }
        }
        (Some(f), None, Some(n)) => {
            if f > n {
                (true, false)
            } else {
                (false, true)
            }
        }
        (None, Some(i), Some(n)) => {
            if n > i {
                (false, true)
            } else {
                (false, false)
            }
        }
        (Some(_), None, None) => (true, false),
        (None, Some(_), None) => (false, false),
        (None, None, Some(_)) => (false, true),
        (None, None, None) => (false, false),
    };

    let config = MvConfig {
        force,
        no_follow: matches.opt_present("h"),
        no_clobber,
        verbose: matches.opt_present("v"),
    };

    if matches.free.len() < 2 {
        env.stderr.write_line(
            "usage: mv [-f | -i | -n] [-hv] source target\n       \
             mv [-f | -i | -n] [-v] source ... directory",
        )?;
        return Ok(ExitCode::from(1));
    }

    let last_arg = &matches.free[matches.free.len() - 1];
    let last_arg_trimmed = last_arg.trim_end_matches('/');
    let has_trailing_slash = last_arg.len() > 1 && last_arg.ends_with('/');

    // Determine if target is a directory
    let target_is_dir = if has_trailing_slash {
        // Trailing slash forces directory interpretation
        env.fs
            .stat(last_arg_trimmed)
            .map(|e| e.file_type == FileType::Directory)
            .unwrap_or(false)
    } else if config.no_follow {
        // With -h, check if it's a symlink to a directory
        match env.fs.lstat(last_arg_trimmed) {
            Ok(entry) if entry.file_type == FileType::Symlink => {
                // It's a symlink - check what it points to, but if we're in -h mode
                // with 2 args, treat it as a file (rename the symlink)
                if matches.free.len() == 2 {
                    false
                } else {
                    env.fs
                        .stat(last_arg_trimmed)
                        .map(|e| e.file_type == FileType::Directory)
                        .unwrap_or(false)
                }
            }
            Ok(entry) => entry.file_type == FileType::Directory,
            Err(_) => false,
        }
    } else {
        // Without -h, follow symlinks (use stat)
        env.fs
            .stat(last_arg_trimmed)
            .map(|e| e.file_type == FileType::Directory)
            .unwrap_or(false)
    };

    // If target doesn't exist or isn't a directory, we can only have 2 args
    if !target_is_dir && matches.free.len() > 2 {
        env.stderr
            .write_line(&format!("mv: {} is not a directory", last_arg))?;
        return Ok(ExitCode::from(1));
    }

    let mut exit_code: i8 = 0;

    if target_is_dir {
        // Move each source into the target directory
        for source in &matches.free[..matches.free.len() - 1] {
            let basename = source
                .trim_end_matches('/')
                .rsplit('/')
                .next()
                .unwrap_or(source);
            let target = format!("{}/{}", last_arg_trimmed, basename);

            if let Err(msg) = do_move(env, &config, source, &target) {
                env.stderr.write_line(&format!("mv: {}", msg))?;
                exit_code = 1;
            }
        }
    } else {
        // Simple rename: source to target
        let source = &matches.free[0];

        if let Err(msg) = do_move(env, &config, source, last_arg_trimmed) {
            env.stderr.write_line(&format!("mv: {}", msg))?;
            exit_code = 1;
        }
    }

    Ok(ExitCode::from(exit_code))
}

/// Perform a single move operation.
fn do_move<SI, SO, SE, FS>(
    env: &Environment<SI, SO, SE, FS>,
    config: &MvConfig,
    from: &str,
    to: &str,
) -> Result<(), String>
where
    SI: Stdin,
    SO: Stdout,
    SE: Stderr,
    FS: Filesystem,
{
    // Normalize paths by removing trailing slashes
    let from = from.trim_end_matches('/');
    let from = if from.is_empty() { "/" } else { from };

    // Check if source exists
    if env.fs.lstat(from).is_err() {
        return Err(format!("{}: No such file or directory", from));
    }

    // Check if destination exists
    let dest_exists = env.fs.lstat(to).is_ok();

    if dest_exists && config.no_clobber {
        if config.verbose {
            env.stdout
                .write_line(&format!("{} not overwritten", to))
                .ok();
        }
        return Ok(());
    }
    // With -f or default behavior, we proceed to overwrite

    // Try rename first (works for same filesystem)
    match env.fs.rename(from, to) {
        Ok(()) => {
            if config.verbose {
                env.stdout.write_line(&format!("{} -> {}", from, to)).ok();
            }
            Ok(())
        }
        Err(e) => {
            // If rename fails, we could try copy+delete for cross-filesystem moves
            // For now, just report the error
            Err(format!(
                "rename {} to {}: {}",
                from,
                to,
                io_error_message(&e)
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MockFilesystem, StringStderr, StringStdin, StringStdout};

    fn make_env(
        args: Vec<&str>,
    ) -> Environment<StringStdin, StringStdout, StringStderr, MockFilesystem> {
        let fs = MockFilesystem::new();
        fs.add_directory("/");
        Environment {
            stdin: StringStdin::new(""),
            stdout: StringStdout::new(),
            stderr: StringStderr::new(),
            fs,
            env: std::collections::HashMap::new(),
            args: args.into_iter().map(|s| s.to_string()).collect(),
            cwd: utf8path::Path::from("/"),
            exit_signaled: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }

    // ========================================================================
    // Basic usage tests
    // ========================================================================

    #[test]
    fn no_args_shows_usage() {
        let env = make_env(vec!["mv"]);
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("usage:"));
    }

    #[test]
    fn one_arg_shows_usage() {
        let env = make_env(vec!["mv", "/source"]);
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("usage:"));
    }

    #[test]
    fn rename_file() {
        let env = make_env(vec!["mv", "/old", "/new"]);
        env.fs.write_string("/old", "content").unwrap();
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert!(!env.fs.exists("/old"));
        assert!(env.fs.exists("/new"));
        assert_eq!("content", env.fs.read_to_string("/new").unwrap());
    }

    #[test]
    fn rename_directory() {
        let env = make_env(vec!["mv", "/olddir", "/newdir"]);
        env.fs.mkdir("/olddir").unwrap();
        env.fs.write_string("/olddir/file", "content").unwrap();
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert!(!env.fs.exists("/olddir"));
        assert!(env.fs.is_dir("/newdir"));
        assert!(env.fs.exists("/newdir/file"));
        assert_eq!("content", env.fs.read_to_string("/newdir/file").unwrap());
    }

    #[test]
    fn move_file_into_directory() {
        let env = make_env(vec!["mv", "/file", "/dir"]);
        env.fs.write_string("/file", "content").unwrap();
        env.fs.mkdir("/dir").unwrap();
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert!(!env.fs.exists("/file"));
        assert!(env.fs.exists("/dir/file"));
        assert_eq!("content", env.fs.read_to_string("/dir/file").unwrap());
    }

    #[test]
    fn move_multiple_files_into_directory() {
        let env = make_env(vec!["mv", "/a", "/b", "/c", "/dir"]);
        env.fs.write_string("/a", "a").unwrap();
        env.fs.write_string("/b", "b").unwrap();
        env.fs.write_string("/c", "c").unwrap();
        env.fs.mkdir("/dir").unwrap();
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert!(!env.fs.exists("/a"));
        assert!(!env.fs.exists("/b"));
        assert!(!env.fs.exists("/c"));
        assert!(env.fs.exists("/dir/a"));
        assert!(env.fs.exists("/dir/b"));
        assert!(env.fs.exists("/dir/c"));
    }

    #[test]
    fn move_nonexistent_fails() {
        let env = make_env(vec!["mv", "/nonexistent", "/target"]);
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("No such file or directory"));
    }

    #[test]
    fn move_multiple_to_non_directory_fails() {
        let env = make_env(vec!["mv", "/a", "/b", "/target"]);
        env.fs.write_string("/a", "a").unwrap();
        env.fs.write_string("/b", "b").unwrap();
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("is not a directory"));
    }

    // ========================================================================
    // -f flag tests
    // ========================================================================

    #[test]
    fn force_overwrites_existing() {
        let env = make_env(vec!["mv", "-f", "/source", "/target"]);
        env.fs.write_string("/source", "new content").unwrap();
        env.fs.write_string("/target", "old content").unwrap();
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert!(!env.fs.exists("/source"));
        assert_eq!("new content", env.fs.read_to_string("/target").unwrap());
    }

    #[test]
    fn default_overwrites_existing() {
        let env = make_env(vec!["mv", "/source", "/target"]);
        env.fs.write_string("/source", "new content").unwrap();
        env.fs.write_string("/target", "old content").unwrap();
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert!(!env.fs.exists("/source"));
        assert_eq!("new content", env.fs.read_to_string("/target").unwrap());
    }

    // ========================================================================
    // -n flag tests
    // ========================================================================

    #[test]
    fn no_clobber_does_not_overwrite() {
        let env = make_env(vec!["mv", "-n", "/source", "/target"]);
        env.fs.write_string("/source", "new content").unwrap();
        env.fs.write_string("/target", "old content").unwrap();
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        // Source still exists because move was skipped
        assert!(env.fs.exists("/source"));
        assert_eq!("old content", env.fs.read_to_string("/target").unwrap());
    }

    #[test]
    fn no_clobber_verbose_shows_not_overwritten() {
        let env = make_env(vec!["mv", "-nv", "/source", "/target"]);
        env.fs.write_string("/source", "new content").unwrap();
        env.fs.write_string("/target", "old content").unwrap();
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert!(stdout.contains("not overwritten"));
    }

    // ========================================================================
    // -v flag tests
    // ========================================================================

    #[test]
    fn verbose_shows_move() {
        let env = make_env(vec!["mv", "-v", "/source", "/target"]);
        env.fs.write_string("/source", "content").unwrap();
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert!(stdout.contains("/source"));
        assert!(stdout.contains("/target"));
        assert!(stdout.contains("->"));
    }

    // ========================================================================
    // -h flag tests
    // ========================================================================

    #[test]
    fn h_flag_does_not_follow_symlink_to_directory() {
        let env = make_env(vec!["mv", "-h", "/source", "/link"]);
        env.fs.write_string("/source", "content").unwrap();
        env.fs.mkdir("/dir").unwrap();
        env.fs.symlink("/dir", "/link").unwrap();
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        // The symlink should be replaced, not followed
        assert!(!env.fs.exists("/source"));
        // /link should now be a regular file, not a symlink
        let entry = env.fs.lstat("/link").unwrap();
        assert_eq!(FileType::RegularFile, entry.file_type);
    }

    #[test]
    fn without_h_flag_follows_symlink_to_directory() {
        let env = make_env(vec!["mv", "/source", "/link"]);
        env.fs.write_string("/source", "content").unwrap();
        env.fs.mkdir("/dir").unwrap();
        env.fs.symlink("/dir", "/link").unwrap();
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        // Without -h, /link is followed, so /source moves into /dir
        assert!(!env.fs.exists("/source"));
        assert!(env.fs.exists("/dir/source"));
    }

    // ========================================================================
    // Flag precedence tests
    // ========================================================================

    #[test]
    fn f_overrides_n() {
        let env = make_env(vec!["mv", "-n", "-f", "/source", "/target"]);
        env.fs.write_string("/source", "new content").unwrap();
        env.fs.write_string("/target", "old content").unwrap();
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        // -f came last, so it should overwrite
        assert!(!env.fs.exists("/source"));
        assert_eq!("new content", env.fs.read_to_string("/target").unwrap());
    }

    #[test]
    fn n_overrides_f() {
        let env = make_env(vec!["mv", "-f", "-n", "/source", "/target"]);
        env.fs.write_string("/source", "new content").unwrap();
        env.fs.write_string("/target", "old content").unwrap();
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        // -n came last, so it should not overwrite
        assert!(env.fs.exists("/source"));
        assert_eq!("old content", env.fs.read_to_string("/target").unwrap());
    }

    // ========================================================================
    // Symlink tests
    // ========================================================================

    #[test]
    fn move_symlink() {
        let env = make_env(vec!["mv", "/link", "/newlink"]);
        env.fs.write_string("/target", "content").unwrap();
        env.fs.symlink("/target", "/link").unwrap();
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert!(!env.fs.exists("/link"));
        let entry = env.fs.lstat("/newlink").unwrap();
        assert_eq!(FileType::Symlink, entry.file_type);
        assert_eq!("/target", env.fs.readlink("/newlink").unwrap());
    }

    // ========================================================================
    // Edge cases
    // ========================================================================

    #[test]
    fn move_file_with_trailing_slash_source() {
        let env = make_env(vec!["mv", "/dir/", "/newdir"]);
        env.fs.mkdir("/dir").unwrap();
        env.fs.write_string("/dir/file", "content").unwrap();
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert!(!env.fs.exists("/dir"));
        assert!(env.fs.is_dir("/newdir"));
    }

    #[test]
    fn move_into_directory_with_trailing_slash() {
        let env = make_env(vec!["mv", "/file", "/dir/"]);
        env.fs.write_string("/file", "content").unwrap();
        env.fs.mkdir("/dir").unwrap();
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert!(!env.fs.exists("/file"));
        assert!(env.fs.exists("/dir/file"));
    }

    #[test]
    fn move_directory_into_itself_fails() {
        let env = make_env(vec!["mv", "/dir", "/dir/subdir"]);
        env.fs.mkdir("/dir").unwrap();
        env.fs.mkdir("/dir/subdir").unwrap();
        let result = bin(&env).unwrap();
        // This should fail because we're trying to move a directory into itself
        assert_eq!(1, result.code());
    }

    #[test]
    fn partial_failure_continues() {
        let env = make_env(vec!["mv", "/a", "/nonexistent", "/b", "/dir"]);
        env.fs.write_string("/a", "a").unwrap();
        env.fs.write_string("/b", "b").unwrap();
        env.fs.mkdir("/dir").unwrap();
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        // /a and /b should have been moved despite /nonexistent failing
        assert!(!env.fs.exists("/a"));
        assert!(!env.fs.exists("/b"));
        assert!(env.fs.exists("/dir/a"));
        assert!(env.fs.exists("/dir/b"));
    }
}

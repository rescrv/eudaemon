//! The cp utility: copy files.

use std::collections::VecDeque;

use getopts::Options;

use crate::{Environment, Error, ExitCode, FileType, Filesystem, Stderr, Stdin, Stdout};

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

fn build_options() -> Options {
    let mut opts = Options::new();
    opts.optflag("R", "recursive", "Copy directories recursively");
    opts.optflag("r", "", "Equivalent to -R (deprecated)");
    opts.optflag("a", "archive", "Archive mode: same as -RpP");
    opts.optflag("f", "force", "Force: remove existing destination files");
    opts.optflag(
        "i",
        "interactive",
        "Prompt before overwrite (not implemented)",
    );
    opts.optflag("l", "link", "Create hard links instead of copying");
    opts.optflag("n", "no-clobber", "Do not overwrite existing files");
    opts.optflag("p", "", "Preserve attributes (not fully implemented)");
    opts.optflag(
        "s",
        "symbolic-link",
        "Create symbolic links instead of copying",
    );
    opts.optflag("v", "verbose", "Be verbose");
    opts.optflag("H", "", "Follow symlinks on command line (with -R)");
    opts.optflag("L", "dereference", "Always follow symlinks (with -R)");
    opts.optflag(
        "P",
        "no-dereference",
        "Never follow symlinks (default with -R)",
    );
    opts
}

/// Configuration for the cp command.
struct CpConfig {
    /// Copy directories recursively.
    recursive: bool,
    /// Force: remove existing destination files.
    force: bool,
    /// Do not overwrite existing files.
    no_clobber: bool,
    /// Create hard links instead of copying.
    link: bool,
    /// Create symbolic links instead of copying.
    symlink: bool,
    /// Verbose output.
    verbose: bool,
    /// Follow symlinks on command line (with -R).
    follow_cli_symlinks: bool,
    /// Always follow symlinks.
    follow_all_symlinks: bool,
}

/// The cp builtin: copy files.
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
            env.stderr.write_line(&format!("cp: {}", e))?;
            return Ok(ExitCode::from(1));
        }
    };

    let recursive =
        matches.opt_present("R") || matches.opt_present("r") || matches.opt_present("a");
    let archive = matches.opt_present("a");

    // Determine symlink following behavior
    // Default: without -R, follow symlinks; with -R, don't follow
    let (follow_cli_symlinks, follow_all_symlinks) = if recursive {
        if matches.opt_present("L") {
            (true, true)
        } else if matches.opt_present("H") {
            (true, false)
        } else {
            // -P is default with -R, or explicit -a
            (false, false)
        }
    } else {
        // Without -R, always follow symlinks (as if -L)
        (true, true)
    };

    let mut config = CpConfig {
        recursive,
        force: matches.opt_present("f"),
        no_clobber: matches.opt_present("n"),
        link: matches.opt_present("l"),
        symlink: matches.opt_present("s"),
        verbose: matches.opt_present("v"),
        follow_cli_symlinks,
        follow_all_symlinks,
    };

    // Archive mode implies -P (no follow)
    if archive {
        config.follow_cli_symlinks = false;
        config.follow_all_symlinks = false;
    }

    // -f overrides -n, -n overrides -f (last one wins, but we just check presence)
    // For simplicity: if both present, -n wins (safer)
    if config.force && config.no_clobber {
        config.force = false;
    }

    // -l and -s are mutually exclusive
    if config.link && config.symlink {
        env.stderr
            .write_line("cp: the -l and -s options may not be specified together")?;
        return Ok(ExitCode::from(1));
    }

    if matches.free.len() < 2 {
        env.stderr.write_line(
            "usage: cp [-R [-H | -L | -P]] [-f | -i | -n] [-alpsvx] source_file target_file\n       \
             cp [-R [-H | -L | -P]] [-f | -i | -n] [-alpsvx] source_file ... target_directory",
        )?;
        return Ok(ExitCode::from(1));
    }

    let sources = &matches.free[..matches.free.len() - 1];
    let target = &matches.free[matches.free.len() - 1];

    // Determine the operation type
    let target_stat = env.fs.stat(target);
    let target_is_dir = target_stat
        .as_ref()
        .map(|e| e.file_type == FileType::Directory)
        .unwrap_or(false);
    let target_exists = target_stat.is_ok()
        || target_stat.as_ref().is_err_and(
            |e| matches!(e, Error::Io(io) if io.kind() == std::io::ErrorKind::NotADirectory),
        );

    // If target has trailing slash, it must be a directory
    let target_has_slash = target.ends_with('/');
    if target_has_slash && !target_is_dir {
        if target_exists {
            env.stderr
                .write_line(&format!("cp: {}: Not a directory", target))?;
        } else {
            env.stderr
                .write_line(&format!("cp: {}: No such file or directory", target))?;
        }
        return Ok(ExitCode::from(1));
    }

    // Multiple sources require target to be a directory
    if sources.len() > 1 && !target_is_dir {
        if target_exists {
            env.stderr
                .write_line(&format!("cp: {}: Not a directory", target))?;
        } else {
            env.stderr
                .write_line(&format!("cp: {}: No such file or directory", target))?;
        }
        return Ok(ExitCode::from(1));
    }

    let mut exit_code: i8 = 0;

    for source in sources {
        let result = if target_is_dir
            || (sources.len() == 1 && !target_exists && is_source_dir(env, source, &config))
        {
            copy_to_dir(env, &config, source, target, target_exists, target_is_dir)
        } else {
            copy_file_to_file(env, &config, source, target)
        };

        if let Err(e) = result {
            env.stderr.write_line(&format!("cp: {}", e))?;
            exit_code = 1;
        }
    }

    Ok(ExitCode::from(exit_code))
}

/// Check if source is a directory (following symlinks according to config).
fn is_source_dir<SI, SO, SE, FS>(
    env: &Environment<SI, SO, SE, FS>,
    source: &str,
    config: &CpConfig,
) -> bool
where
    SI: Stdin,
    SO: Stdout,
    SE: Stderr,
    FS: Filesystem,
{
    let stat = if config.follow_cli_symlinks || config.follow_all_symlinks {
        env.fs.stat(source)
    } else {
        env.fs.lstat(source)
    };
    stat.map(|e| e.file_type == FileType::Directory)
        .unwrap_or(false)
}

/// Copy a single file to another file.
fn copy_file_to_file<SI, SO, SE, FS>(
    env: &Environment<SI, SO, SE, FS>,
    config: &CpConfig,
    source: &str,
    target: &str,
) -> Result<(), String>
where
    SI: Stdin,
    SO: Stdout,
    SE: Stderr,
    FS: Filesystem,
{
    // Check if source exists and get its type
    let source_entry = if config.follow_all_symlinks || config.follow_cli_symlinks {
        env.fs.stat(source)
    } else {
        env.fs.lstat(source)
    }
    .map_err(|e| format!("{}: {}", source, io_error_message(&e)))?;

    // Can't copy a directory without -R
    if source_entry.file_type == FileType::Directory {
        if !config.recursive {
            return Err(format!("{}: is a directory (not copied)", source));
        }
        // This shouldn't happen in this function, but just in case
        return Err(format!("{}: is a directory", source));
    }

    // Check if source and target are the same file
    if let Ok(target_entry) = env.fs.lstat(target)
        && source_entry.dev == target_entry.dev
        && source_entry.ino == target_entry.ino
    {
        return Err(format!(
            "{} and {} are identical (not copied)",
            source, target
        ));
    }

    // Handle existing target
    let target_exists = env.fs.lstat(target).is_ok();
    if target_exists {
        if config.no_clobber {
            if config.verbose {
                env.stdout
                    .write_line(&format!("{} not overwritten", target))
                    .ok();
            }
            return Ok(());
        }
        if config.force {
            env.fs
                .unlink(target)
                .map_err(|e| format!("{}: {}", target, io_error_message(&e)))?;
        }
    }

    // Actually copy the file
    do_copy(env, config, source, target)?;

    if config.verbose {
        env.stdout
            .write_line(&format!("{} -> {}", source, target))
            .ok();
    }

    Ok(())
}

/// Copy source(s) into a target directory.
fn copy_to_dir<SI, SO, SE, FS>(
    env: &Environment<SI, SO, SE, FS>,
    config: &CpConfig,
    source: &str,
    target: &str,
    target_exists: bool,
    target_is_dir: bool,
) -> Result<(), String>
where
    SI: Stdin,
    SO: Stdout,
    SE: Stderr,
    FS: Filesystem,
{
    // Get source stat
    let source_entry = if config.follow_cli_symlinks || config.follow_all_symlinks {
        env.fs.stat(source)
    } else {
        env.fs.lstat(source)
    }
    .map_err(|e| format!("{}: {}", source, io_error_message(&e)))?;

    let source_is_dir = source_entry.file_type == FileType::Directory;

    // If source is a directory and target doesn't exist, create target as copy of source
    if source_is_dir && !target_exists && config.recursive {
        return copy_tree(env, config, source, target);
    }

    // If source is a directory but -R not given
    if source_is_dir && !config.recursive {
        return Err(format!("{}: is a directory (not copied)", source));
    }

    // Target must exist and be a directory at this point
    if !target_is_dir {
        return Err(format!("{}: Not a directory", target));
    }

    // Get the basename of source
    let basename = source
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or(source);

    let target_path = format!("{}/{}", target.trim_end_matches('/'), basename);

    if source_is_dir {
        // Recursive directory copy
        copy_tree(env, config, source, &target_path)
    } else {
        // Single file copy
        copy_file_to_file(env, config, source, &target_path)
    }
}

/// Copy a directory tree recursively.
fn copy_tree<SI, SO, SE, FS>(
    env: &Environment<SI, SO, SE, FS>,
    config: &CpConfig,
    source: &str,
    target: &str,
) -> Result<(), String>
where
    SI: Stdin,
    SO: Stdout,
    SE: Stderr,
    FS: Filesystem,
{
    // BFS traversal to copy directory tree
    // We need to process directories before their contents
    let mut queue: VecDeque<(String, String)> = VecDeque::new();
    queue.push_back((
        source.trim_end_matches('/').to_string(),
        target.trim_end_matches('/').to_string(),
    ));

    let mut result: Result<(), String> = Ok(());

    while let Some((src_path, dst_path)) = queue.pop_front() {
        let entry = if config.follow_all_symlinks {
            env.fs.stat(&src_path)
        } else {
            env.fs.lstat(&src_path)
        }
        .map_err(|e| format!("{}: {}", src_path, io_error_message(&e)))?;

        match entry.file_type {
            FileType::Directory => {
                // Check if destination exists
                let dst_exists = env.fs.lstat(&dst_path).is_ok();

                if dst_exists {
                    // Check if it's a directory
                    let dst_entry = env
                        .fs
                        .lstat(&dst_path)
                        .map_err(|e| format!("{}: {}", dst_path, io_error_message(&e)))?;
                    if dst_entry.file_type != FileType::Directory {
                        let msg = format!("{}: Not a directory", dst_path);
                        env.stderr.write_line(&format!("cp: {}", msg)).ok();
                        result = Err(msg);
                        continue;
                    }
                } else {
                    // Create the directory
                    env.fs
                        .mkdir(&dst_path)
                        .map_err(|e| format!("{}: {}", dst_path, io_error_message(&e)))?;

                    if config.verbose {
                        env.stdout
                            .write_line(&format!("{} -> {}", src_path, dst_path))
                            .ok();
                    }
                }

                // Queue children
                let children = env
                    .fs
                    .read_dir(&src_path)
                    .map_err(|e| format!("{}: {}", src_path, io_error_message(&e)))?;

                for (name, _) in children {
                    let child_src = format!("{}/{}", src_path, name);
                    let child_dst = format!("{}/{}", dst_path, name);
                    queue.push_back((child_src, child_dst));
                }
            }
            FileType::Symlink => {
                // Check if we should follow this symlink
                if config.follow_all_symlinks {
                    // Already handled by stat() above - won't reach here
                    unreachable!();
                }

                // Copy the symlink itself
                let link_target = env
                    .fs
                    .readlink(&src_path)
                    .map_err(|e| format!("{}: {}", src_path, io_error_message(&e)))?;

                // Handle existing destination
                let dst_exists = env.fs.lstat(&dst_path).is_ok();
                if dst_exists {
                    if config.no_clobber {
                        if config.verbose {
                            env.stdout
                                .write_line(&format!("{} not overwritten", dst_path))
                                .ok();
                        }
                        continue;
                    }
                    if config.force {
                        env.fs
                            .unlink(&dst_path)
                            .map_err(|e| format!("{}: {}", dst_path, io_error_message(&e)))?;
                    } else {
                        let msg = format!("{}: File exists", dst_path);
                        env.stderr.write_line(&format!("cp: {}", msg)).ok();
                        result = Err(msg);
                        continue;
                    }
                }

                env.fs
                    .symlink(&link_target, &dst_path)
                    .map_err(|e| format!("{}: {}", dst_path, io_error_message(&e)))?;

                if config.verbose {
                    env.stdout
                        .write_line(&format!("{} -> {}", src_path, dst_path))
                        .ok();
                }
            }
            FileType::RegularFile | FileType::Other => {
                // Copy regular file
                if let Err(e) = copy_file_to_file(env, config, &src_path, &dst_path) {
                    env.stderr.write_line(&format!("cp: {}", e)).ok();
                    result = Err(e);
                }
            }
        }
    }

    result
}

/// Perform the actual copy operation (or link/symlink).
fn do_copy<SI, SO, SE, FS>(
    env: &Environment<SI, SO, SE, FS>,
    config: &CpConfig,
    source: &str,
    target: &str,
) -> Result<(), String>
where
    SI: Stdin,
    SO: Stdout,
    SE: Stderr,
    FS: Filesystem,
{
    if config.link {
        // Create hard link
        env.fs
            .link(source, target)
            .map_err(|e| format!("{}: {}", target, io_error_message(&e)))
    } else if config.symlink {
        // Create symbolic link
        env.fs
            .symlink(source, target)
            .map_err(|e| format!("{}: {}", target, io_error_message(&e)))
    } else {
        // Actually copy the file contents
        // First check if source is a symlink and we should copy the link itself
        let source_lstat = env
            .fs
            .lstat(source)
            .map_err(|e| format!("{}: {}", source, io_error_message(&e)))?;

        if source_lstat.file_type == FileType::Symlink
            && !config.follow_all_symlinks
            && !config.follow_cli_symlinks
        {
            // Copy symlink
            let link_target = env
                .fs
                .readlink(source)
                .map_err(|e| format!("{}: {}", source, io_error_message(&e)))?;
            env.fs
                .symlink(&link_target, target)
                .map_err(|e| format!("{}: {}", target, io_error_message(&e)))
        } else {
            // Copy file contents
            let contents = env
                .fs
                .read_to_string(source)
                .map_err(|e| format!("{}: {}", source, io_error_message(&e)))?;
            env.fs
                .write_string(target, &contents)
                .map_err(|e| format!("{}: {}", target, io_error_message(&e)))
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
        let env = make_env(vec!["cp"]);
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("usage:"));
    }

    #[test]
    fn one_arg_shows_usage() {
        let env = make_env(vec!["cp", "/source"]);
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("usage:"));
    }

    #[test]
    fn copy_single_file() {
        let env = make_env(vec!["cp", "/source", "/target"]);
        env.fs.write_string("/source", "content").unwrap();
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert!(env.fs.exists("/target"));
        assert_eq!("content", env.fs.read_to_string("/target").unwrap());
    }

    #[test]
    fn copy_preserves_source() {
        let env = make_env(vec!["cp", "/source", "/target"]);
        env.fs.write_string("/source", "content").unwrap();
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert!(env.fs.exists("/source"));
        assert_eq!("content", env.fs.read_to_string("/source").unwrap());
    }

    #[test]
    fn copy_nonexistent_fails() {
        let env = make_env(vec!["cp", "/nonexistent", "/target"]);
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("No such file or directory"));
    }

    #[test]
    fn copy_to_same_file_fails() {
        let env = make_env(vec!["cp", "/file", "/file"]);
        env.fs.write_string("/file", "content").unwrap();
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("identical"));
    }

    // ========================================================================
    // Copy to directory tests
    // ========================================================================

    #[test]
    fn copy_file_to_directory() {
        let env = make_env(vec!["cp", "/source", "/dir"]);
        env.fs.write_string("/source", "content").unwrap();
        env.fs.mkdir("/dir").unwrap();
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert!(env.fs.exists("/dir/source"));
        assert_eq!("content", env.fs.read_to_string("/dir/source").unwrap());
    }

    #[test]
    fn copy_multiple_files_to_directory() {
        let env = make_env(vec!["cp", "/a", "/b", "/c", "/dir"]);
        env.fs.write_string("/a", "a").unwrap();
        env.fs.write_string("/b", "b").unwrap();
        env.fs.write_string("/c", "c").unwrap();
        env.fs.mkdir("/dir").unwrap();
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("a", env.fs.read_to_string("/dir/a").unwrap());
        assert_eq!("b", env.fs.read_to_string("/dir/b").unwrap());
        assert_eq!("c", env.fs.read_to_string("/dir/c").unwrap());
    }

    #[test]
    fn copy_multiple_files_to_non_directory_fails() {
        let env = make_env(vec!["cp", "/a", "/b", "/target"]);
        env.fs.write_string("/a", "a").unwrap();
        env.fs.write_string("/b", "b").unwrap();
        env.fs.write_string("/target", "file").unwrap();
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("Not a directory"));
    }

    #[test]
    fn copy_multiple_files_to_nonexistent_fails() {
        let env = make_env(vec!["cp", "/a", "/b", "/nonexistent"]);
        env.fs.write_string("/a", "a").unwrap();
        env.fs.write_string("/b", "b").unwrap();
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
    }

    // ========================================================================
    // -n (no-clobber) flag tests
    // ========================================================================

    #[test]
    fn no_clobber_does_not_overwrite() {
        let env = make_env(vec!["cp", "-n", "/source", "/target"]);
        env.fs.write_string("/source", "new").unwrap();
        env.fs.write_string("/target", "old").unwrap();
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("old", env.fs.read_to_string("/target").unwrap());
    }

    #[test]
    fn no_clobber_verbose_shows_message() {
        let env = make_env(vec!["cp", "-nv", "/source", "/target"]);
        env.fs.write_string("/source", "new").unwrap();
        env.fs.write_string("/target", "old").unwrap();
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert!(stdout.contains("not overwritten"));
    }

    // ========================================================================
    // -f (force) flag tests
    // ========================================================================

    #[test]
    fn force_overwrites_existing() {
        let env = make_env(vec!["cp", "-f", "/source", "/target"]);
        env.fs.write_string("/source", "new").unwrap();
        env.fs.write_string("/target", "old").unwrap();
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("new", env.fs.read_to_string("/target").unwrap());
    }

    #[test]
    fn without_force_overwrites_anyway() {
        // Default cp behavior: overwrite without -f
        let env = make_env(vec!["cp", "/source", "/target"]);
        env.fs.write_string("/source", "new").unwrap();
        env.fs.write_string("/target", "old").unwrap();
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("new", env.fs.read_to_string("/target").unwrap());
    }

    // ========================================================================
    // -v (verbose) flag tests
    // ========================================================================

    #[test]
    fn verbose_prints_copy_info() {
        let env = make_env(vec!["cp", "-v", "/source", "/target"]);
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
    // -l (link) flag tests
    // ========================================================================

    #[test]
    fn link_creates_hard_link() {
        let env = make_env(vec!["cp", "-l", "/source", "/target"]);
        env.fs.write_string("/source", "content").unwrap();
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert!(env.fs.exists("/target"));
        // Hard links share the same inode
        let source_stat = env.fs.stat("/source").unwrap();
        let target_stat = env.fs.stat("/target").unwrap();
        assert_eq!(source_stat.ino, target_stat.ino);
    }

    // ========================================================================
    // -s (symbolic-link) flag tests
    // ========================================================================

    #[test]
    fn symlink_creates_symbolic_link() {
        let env = make_env(vec!["cp", "-s", "/source", "/target"]);
        env.fs.write_string("/source", "content").unwrap();
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let entry = env.fs.lstat("/target").unwrap();
        assert_eq!(FileType::Symlink, entry.file_type);
        assert_eq!("/source", env.fs.readlink("/target").unwrap());
    }

    #[test]
    fn link_and_symlink_mutually_exclusive() {
        let env = make_env(vec!["cp", "-ls", "/source", "/target"]);
        env.fs.write_string("/source", "content").unwrap();
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("-l and -s"));
    }

    // ========================================================================
    // -R (recursive) flag tests
    // ========================================================================

    #[test]
    fn copy_directory_without_r_fails() {
        let env = make_env(vec!["cp", "/dir", "/target"]);
        env.fs.mkdir("/dir").unwrap();
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("is a directory"));
    }

    #[test]
    fn recursive_copies_directory() {
        let env = make_env(vec!["cp", "-R", "/src", "/dst"]);
        env.fs.mkdir("/src").unwrap();
        env.fs.write_string("/src/file", "content").unwrap();
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert!(env.fs.is_dir("/dst"));
        assert!(env.fs.exists("/dst/file"));
        assert_eq!("content", env.fs.read_to_string("/dst/file").unwrap());
    }

    #[test]
    fn recursive_copies_nested_directories() {
        let env = make_env(vec!["cp", "-R", "/src", "/dst"]);
        env.fs.mkdir("/src").unwrap();
        env.fs.mkdir("/src/sub").unwrap();
        env.fs.write_string("/src/sub/file", "content").unwrap();
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert!(env.fs.is_dir("/dst"));
        assert!(env.fs.is_dir("/dst/sub"));
        assert!(env.fs.exists("/dst/sub/file"));
        assert_eq!("content", env.fs.read_to_string("/dst/sub/file").unwrap());
    }

    #[test]
    fn recursive_into_existing_directory() {
        let env = make_env(vec!["cp", "-R", "/src", "/dst"]);
        env.fs.mkdir("/src").unwrap();
        env.fs.write_string("/src/file", "content").unwrap();
        env.fs.mkdir("/dst").unwrap();
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert!(env.fs.is_dir("/dst/src"));
        assert!(env.fs.exists("/dst/src/file"));
    }

    #[test]
    fn lowercase_r_same_as_uppercase() {
        let env = make_env(vec!["cp", "-r", "/src", "/dst"]);
        env.fs.mkdir("/src").unwrap();
        env.fs.write_string("/src/file", "content").unwrap();
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert!(env.fs.is_dir("/dst"));
        assert!(env.fs.exists("/dst/file"));
    }

    // ========================================================================
    // -a (archive) flag tests
    // ========================================================================

    #[test]
    fn archive_copies_directory() {
        let env = make_env(vec!["cp", "-a", "/src", "/dst"]);
        env.fs.mkdir("/src").unwrap();
        env.fs.write_string("/src/file", "content").unwrap();
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert!(env.fs.is_dir("/dst"));
        assert!(env.fs.exists("/dst/file"));
    }

    // ========================================================================
    // Symlink handling tests
    // ========================================================================

    #[test]
    fn copy_through_symlink_default() {
        // Without -R, symlinks are followed by default
        let env = make_env(vec!["cp", "/link", "/target"]);
        env.fs.write_string("/real", "content").unwrap();
        env.fs.symlink("/real", "/link").unwrap();
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        // Target should be a regular file, not a symlink
        let entry = env.fs.lstat("/target").unwrap();
        assert_eq!(FileType::RegularFile, entry.file_type);
        assert_eq!("content", env.fs.read_to_string("/target").unwrap());
    }

    #[test]
    fn recursive_copies_symlink_as_symlink() {
        // With -R (no -L), symlinks are copied as symlinks
        let env = make_env(vec!["cp", "-R", "/src", "/dst"]);
        env.fs.mkdir("/src").unwrap();
        env.fs.write_string("/real", "content").unwrap();
        env.fs.symlink("/real", "/src/link").unwrap();
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let entry = env.fs.lstat("/dst/link").unwrap();
        assert_eq!(FileType::Symlink, entry.file_type);
        assert_eq!("/real", env.fs.readlink("/dst/link").unwrap());
    }

    // ========================================================================
    // Edge cases
    // ========================================================================

    #[test]
    fn trailing_slash_requires_directory() {
        let env = make_env(vec!["cp", "/source", "/notdir/"]);
        env.fs.write_string("/source", "content").unwrap();
        env.fs.write_string("/notdir", "file").unwrap();
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("Not a directory"));
    }

    #[test]
    fn trailing_slash_nonexistent_fails() {
        let env = make_env(vec!["cp", "/source", "/nonexistent/"]);
        env.fs.write_string("/source", "content").unwrap();
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
    }

    #[test]
    fn copy_with_path_basename() {
        let env = make_env(vec!["cp", "/path/to/file", "/dir"]);
        env.fs.mkdir("/path").unwrap();
        env.fs.mkdir("/path/to").unwrap();
        env.fs.write_string("/path/to/file", "content").unwrap();
        env.fs.mkdir("/dir").unwrap();
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert!(env.fs.exists("/dir/file"));
    }
}

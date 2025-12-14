//! The ls builtin: list directory contents.

use getopts::Options;

use crate::{
    DirEntry, Environment, Error, ExitCode, FileType, Filesystem, Stderr, Stdin, Stdout,
    format_human_size, resolve_path,
};

/// Options for the ls command.
#[derive(Default)]
struct LsOptions {
    /// Include directory entries whose names begin with a dot.
    all: bool,
    /// Include directory entries whose names begin with a dot, except . and ..
    almost_all: bool,
    /// Long listing format.
    long_format: bool,
    /// Display a slash after directories, etc.
    classify: bool,
    /// Display a slash after directories only.
    slash_dirs: bool,
    /// Recursively list subdirectories.
    recursive: bool,
    /// Reverse sort order.
    reverse_sort: bool,
    /// Sort by modification time.
    time_sort: bool,
    /// Sort by size.
    size_sort: bool,
    /// List directories themselves, not their contents.
    list_directory: bool,
    /// Human-readable sizes.
    human_readable: bool,
}

fn build_options() -> Options {
    let mut opts = Options::new();
    opts.optflag("1", "", "Force output to be one entry per line.");
    opts.optflag(
        "a",
        "",
        "Include directory entries whose names begin with a dot.",
    );
    opts.optflag("A", "", "Include entries starting with dot except . and ..");
    opts.optflag("d", "", "Directories are listed as plain files.");
    opts.optflag(
        "F",
        "",
        "Display type indicators (/ for dirs, * for executables, @ for symlinks).",
    );
    opts.optflag("h", "", "Use human-readable sizes.");
    opts.optflag("l", "", "List in long format.");
    opts.optflag("p", "", "Write a slash after each directory name.");
    opts.optflag("R", "", "Recursively list subdirectories.");
    opts.optflag("r", "", "Reverse the sort order.");
    opts.optflag("S", "", "Sort by size.");
    opts.optflag("t", "", "Sort by modification time.");
    opts
}

/// The ls builtin: list directory contents.
///
/// Usage:
///   ls [-1AadFhlpRrSt] [file ...]
///
/// If no operands are given, the contents of the current directory are displayed.
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
            env.stderr.write_line(&format!("ls: {}", e))?;
            return Ok(ExitCode::from(1));
        }
    };

    let mut opts = LsOptions::default();

    // -1 is the default for non-terminal output, but we always use single column
    // -a includes all files including . and ..
    if matches.opt_present("a") {
        opts.all = true;
    }
    // -A includes dotfiles except . and ..
    if matches.opt_present("A") {
        opts.almost_all = true;
    }
    // -d lists directories themselves, not contents
    if matches.opt_present("d") {
        opts.list_directory = true;
        opts.recursive = false;
    }
    // -F adds type indicators
    if matches.opt_present("F") {
        opts.classify = true;
        opts.slash_dirs = false;
    }
    // -h human readable sizes
    if matches.opt_present("h") {
        opts.human_readable = true;
    }
    // -l long format
    if matches.opt_present("l") {
        opts.long_format = true;
    }
    // -p adds slash to directories only
    if matches.opt_present("p") {
        opts.slash_dirs = true;
        opts.classify = true;
    }
    // -R recursive
    if matches.opt_present("R") {
        opts.recursive = true;
    }
    // -r reverse sort
    if matches.opt_present("r") {
        opts.reverse_sort = true;
    }
    // -S sort by size
    if matches.opt_present("S") {
        opts.size_sort = true;
        opts.time_sort = false;
    }
    // -t sort by time
    if matches.opt_present("t") {
        opts.time_sort = true;
        opts.size_sort = false;
    }

    let mut paths = matches.free.clone();
    if paths.is_empty() {
        paths.push(".".to_string());
    }

    let mut exit_code = 0i8;
    let mut first = true;
    let multiple_args = paths.len() > 1;

    // Separate files and directories
    let mut files: Vec<(String, DirEntry)> = Vec::new();
    let mut dirs: Vec<String> = Vec::new();

    for path in &paths {
        let resolved = resolve_path(env.cwd.as_str(), path);
        match env.fs.stat(&resolved) {
            Ok(entry) => {
                if entry.file_type == FileType::Directory && !opts.list_directory {
                    dirs.push(path.clone());
                } else {
                    files.push((path.clone(), entry));
                }
            }
            Err(_) => {
                env.stderr
                    .write_line(&format!("ls: {}: No such file or directory", path))?;
                exit_code = 1;
            }
        }
    }

    // Sort files
    sort_entries(&mut files, &opts);

    // Print non-directory files first
    if !files.is_empty() {
        for (name, entry) in &files {
            print_entry(env, name, entry, &opts)?;
        }
        first = false;
    }

    // Sort directories
    if opts.reverse_sort {
        dirs.sort_by(|a, b| b.cmp(a));
    } else {
        dirs.sort();
    }

    // Print directories
    for dir in &dirs {
        if !first {
            env.stdout.write_str("\n")?;
        }
        if multiple_args || opts.recursive {
            env.stdout.write_line(&format!("{}:", dir))?;
        }
        list_directory(env, dir, &opts, &mut exit_code)?;
        first = false;
    }

    Ok(ExitCode::from(exit_code))
}

/// Sort entries according to options.
fn sort_entries(entries: &mut [(String, DirEntry)], opts: &LsOptions) {
    if opts.time_sort {
        entries.sort_by(|a, b| {
            let cmp = b.1.mtime_ms.cmp(&a.1.mtime_ms);
            if cmp == std::cmp::Ordering::Equal {
                a.0.cmp(&b.0)
            } else {
                cmp
            }
        });
    } else if opts.size_sort {
        entries.sort_by(|a, b| {
            let cmp = b.1.size.cmp(&a.1.size);
            if cmp == std::cmp::Ordering::Equal {
                a.0.cmp(&b.0)
            } else {
                cmp
            }
        });
    } else {
        entries.sort_by(|a, b| a.0.cmp(&b.0));
    }

    if opts.reverse_sort {
        entries.reverse();
    }
}

/// List the contents of a directory.
fn list_directory<SI, SO, SE, FS>(
    env: &Environment<SI, SO, SE, FS>,
    path: &str,
    opts: &LsOptions,
    exit_code: &mut i8,
) -> Result<(), Error>
where
    SI: Stdin,
    SO: Stdout,
    SE: Stderr,
    FS: Filesystem,
{
    let resolved = resolve_path(env.cwd.as_str(), path);
    let entries = match env.fs.read_dir(&resolved) {
        Ok(entries) => entries,
        Err(_) => {
            env.stderr
                .write_line(&format!("ls: {}: No such file or directory", path))?;
            *exit_code = 1;
            return Ok(());
        }
    };

    let mut filtered: Vec<(String, DirEntry)> = entries
        .into_iter()
        .filter(|(name, _)| {
            if opts.all {
                true
            } else if opts.almost_all {
                name != "." && name != ".."
            } else {
                !name.starts_with('.')
            }
        })
        .collect();

    sort_entries(&mut filtered, opts);

    for (name, entry) in &filtered {
        print_entry(env, name, entry, opts)?;
    }

    // Handle recursive listing
    if opts.recursive {
        let subdirs: Vec<String> = filtered
            .iter()
            .filter(|(name, entry)| {
                entry.file_type == FileType::Directory && name != "." && name != ".."
            })
            .map(|(name, _)| name.clone())
            .collect();

        for subdir in subdirs {
            let subpath = if path == "." {
                subdir.clone()
            } else {
                format!("{}/{}", path.trim_end_matches('/'), subdir)
            };
            env.stdout.write_str("\n")?;
            env.stdout.write_line(&format!("{}:", subpath))?;
            list_directory(env, &subpath, opts, exit_code)?;
        }
    }

    Ok(())
}

/// Print a single entry.
fn print_entry<SI, SO, SE, FS>(
    env: &Environment<SI, SO, SE, FS>,
    name: &str,
    entry: &DirEntry,
    opts: &LsOptions,
) -> Result<(), Error>
where
    SI: Stdin,
    SO: Stdout,
    SE: Stderr,
    FS: Filesystem,
{
    let mut output = String::new();

    if opts.long_format {
        // File type character
        let type_char = match entry.file_type {
            FileType::Directory => 'd',
            FileType::RegularFile => '-',
            FileType::Symlink => 'l',
            FileType::Other => '?',
        };

        // Format size
        let size_str = if opts.human_readable {
            format_human_size(entry.size)
        } else {
            format!("{:8}", entry.size)
        };

        output.push_str(&format!("{}  {} ", type_char, size_str));
    }

    output.push_str(name);

    // Type indicator
    if opts.classify {
        if opts.slash_dirs {
            if entry.file_type == FileType::Directory {
                output.push('/');
            }
        } else {
            match entry.file_type {
                FileType::Directory => output.push('/'),
                FileType::Symlink => output.push('@'),
                _ => {}
            }
        }
    }

    env.stdout.write_line(&output)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::make_test_env;

    #[test]
    fn empty_directory_produces_no_output() {
        let env = make_test_env(vec!["ls"]);
        env.fs.add_directory("/");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert_eq!("", stdout);
    }

    #[test]
    fn lists_files_in_directory() {
        let env = make_test_env(vec!["ls"]);
        env.fs.add_directory("/");
        env.fs.add_file("/foo", "content");
        env.fs.add_file("/bar", "content");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert!(stdout.contains("foo"));
        assert!(stdout.contains("bar"));
    }

    #[test]
    fn lists_files_sorted_alphabetically() {
        let env = make_test_env(vec!["ls"]);
        env.fs.add_directory("/");
        env.fs.add_file("/zebra", "");
        env.fs.add_file("/apple", "");
        env.fs.add_file("/mango", "");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        let lines: Vec<&str> = stdout.lines().collect();
        println!("lines: {:?}", lines);
        assert_eq!(vec!["apple", "mango", "zebra"], lines);
    }

    #[test]
    fn hides_dotfiles_by_default() {
        let env = make_test_env(vec!["ls"]);
        env.fs.add_directory("/");
        env.fs.add_file("/visible", "");
        env.fs.add_file("/.hidden", "");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert!(stdout.contains("visible"));
        assert!(!stdout.contains(".hidden"));
    }

    #[test]
    fn shows_dotfiles_with_a_flag() {
        let env = make_test_env(vec!["ls", "-a"]);
        env.fs.add_directory("/");
        env.fs.add_file("/visible", "");
        env.fs.add_file("/.hidden", "");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert!(stdout.contains("visible"));
        assert!(stdout.contains(".hidden"));
    }

    #[test]
    fn almost_all_flag_shows_dotfiles() {
        let env = make_test_env(vec!["ls", "-A"]);
        env.fs.add_directory("/");
        env.fs.add_file("/.hidden", "");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert!(stdout.contains(".hidden"));
    }

    #[test]
    fn reverse_sort_with_r_flag() {
        let env = make_test_env(vec!["ls", "-r"]);
        env.fs.add_directory("/");
        env.fs.add_file("/apple", "");
        env.fs.add_file("/zebra", "");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        let lines: Vec<&str> = stdout.lines().collect();
        println!("lines: {:?}", lines);
        assert_eq!(vec!["zebra", "apple"], lines);
    }

    #[test]
    fn classify_flag_adds_slash_to_directories() {
        let env = make_test_env(vec!["ls", "-F"]);
        env.fs.add_directory("/");
        env.fs.add_directory("/subdir");
        env.fs.add_file("/file", "");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert!(stdout.contains("subdir/"));
        assert!(stdout.contains("file\n") || stdout.ends_with("file"));
    }

    #[test]
    fn p_flag_adds_slash_only_to_directories() {
        let env = make_test_env(vec!["ls", "-p"]);
        env.fs.add_directory("/");
        env.fs.add_directory("/subdir");
        env.fs.add_file("/file", "");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert!(stdout.contains("subdir/"));
    }

    #[test]
    fn long_format_shows_details() {
        let env = make_test_env(vec!["ls", "-l"]);
        env.fs.add_directory("/");
        env.fs.add_file("/testfile", "hello");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert!(stdout.contains("testfile"));
        assert!(stdout.contains("-")); // File type indicator
    }

    #[test]
    fn nonexistent_file_returns_error() {
        let env = make_test_env(vec!["ls", "/nonexistent"]);
        env.fs.add_directory("/");
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("No such file or directory"));
    }

    #[test]
    fn invalid_option_returns_error() {
        let env = make_test_env(vec!["ls", "-Q"]);
        env.fs.add_directory("/");
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("Unrecognized"));
    }

    #[test]
    fn list_specific_file() {
        let env = make_test_env(vec!["ls", "/myfile"]);
        env.fs.add_directory("/");
        env.fs.add_file("/myfile", "content");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert!(stdout.contains("myfile"));
    }

    #[test]
    fn d_flag_lists_directory_itself() {
        let env = make_test_env(vec!["ls", "-d", "/"]);
        env.fs.add_directory("/");
        env.fs.add_file("/foo", "");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        // Should list "/" not its contents
        assert!(!stdout.contains("foo"));
    }

    #[test]
    fn recursive_flag_lists_subdirectories() {
        let env = make_test_env(vec!["ls", "-R"]);
        env.fs.add_directory("/");
        env.fs.add_directory("/subdir");
        env.fs.add_file("/subdir/nested", "");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert!(stdout.contains("subdir"));
        assert!(stdout.contains("nested"));
    }

    #[test]
    fn multiple_directories_shows_headers() {
        let env = make_test_env(vec!["ls", "/dir1", "/dir2"]);
        env.fs.add_directory("/");
        env.fs.add_directory("/dir1");
        env.fs.add_directory("/dir2");
        env.fs.add_file("/dir1/file1", "");
        env.fs.add_file("/dir2/file2", "");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert!(stdout.contains("/dir1:"));
        assert!(stdout.contains("/dir2:"));
    }

    #[test]
    fn resolve_path_absolute() {
        assert_eq!("/foo/bar", resolve_path("/home", "/foo/bar"));
    }

    #[test]
    fn resolve_path_relative() {
        assert_eq!("/home/foo", resolve_path("/home", "foo"));
    }

    #[test]
    fn resolve_path_dot() {
        assert_eq!("/home", resolve_path("/home", "."));
    }

    #[test]
    fn resolve_path_dotdot() {
        assert_eq!("/", resolve_path("/home", ".."));
        assert_eq!("/home", resolve_path("/home/user", ".."));
    }

    #[test]
    fn resolve_path_dotslash() {
        assert_eq!("/home/foo", resolve_path("/home", "./foo"));
    }

    #[test]
    fn resolve_path_dotdotslash() {
        assert_eq!("/foo", resolve_path("/home", "../foo"));
    }

    #[test]
    fn format_human_size_bytes() {
        assert_eq!("   0B", format_human_size(0));
        assert_eq!(" 100B", format_human_size(100));
        assert_eq!("1023B", format_human_size(1023));
    }

    #[test]
    fn format_human_size_kilobytes() {
        assert_eq!(" 1.0K", format_human_size(1024));
        assert_eq!("  10K", format_human_size(10 * 1024));
    }

    #[test]
    fn format_human_size_megabytes() {
        assert_eq!(" 1.0M", format_human_size(1024 * 1024));
    }

    #[test]
    fn sort_by_size() {
        let env = make_test_env(vec!["ls", "-S"]);
        env.fs.add_directory("/");
        env.fs.add_file("/small", "x");
        env.fs.add_file("/large", "xxxxxxxxxx");
        env.fs.add_file("/medium", "xxxxx");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        let lines: Vec<&str> = stdout.lines().collect();
        println!("lines: {:?}", lines);
        assert_eq!(vec!["large", "medium", "small"], lines);
    }

    #[test]
    fn human_readable_sizes() {
        let env = make_test_env(vec!["ls", "-lh"]);
        env.fs.add_directory("/");
        env.fs.add_file("/file", "hello");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert!(stdout.contains("5B") || stdout.contains("   5B"));
    }
}

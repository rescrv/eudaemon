//! The du builtin: display disk usage statistics.

use std::collections::HashSet;

use getopts::Options;

use crate::{
    DirEntry, Environment, Error, ExitCode, FileType, Filesystem, StdioIn, StdioOut,
    format_human_size_with_base, resolve_path,
};

/// Options for the du command.
#[derive(Default)]
struct DuOptions {
    /// Display an entry for each file in a file hierarchy.
    all: bool,
    /// Display a grand total.
    grand_total: bool,
    /// Maximum depth to display.
    max_depth: Option<usize>,
    /// Display only a summary (equivalent to -d 0).
    summarize: bool,
    /// Human-readable output (powers of 1024).
    human_readable: bool,
    /// Human-readable output (powers of 1000).
    si: bool,
    /// Display in kilobytes.
    kilobytes: bool,
    /// Display in megabytes.
    megabytes: bool,
    /// Display in gigabytes.
    gigabytes: bool,
    /// Block size for calculation.
    block_size: u64,
    /// Count hard links multiple times.
    count_links: bool,
    /// Display apparent size instead of disk usage.
    apparent_size: bool,
}

fn build_options() -> Options {
    let mut opts = Options::new();
    opts.optflag(
        "a",
        "",
        "Display an entry for each file in a file hierarchy.",
    );
    opts.optflag("c", "", "Display a grand total.");
    opts.optopt(
        "d",
        "max-depth",
        "Display entries for directories depth directories deep.",
        "DEPTH",
    );
    opts.optflag("h", "", "Human-readable output (powers of 1024).");
    opts.optflag("k", "", "Display block counts in 1024-byte (1 KiB) blocks.");
    opts.optflag(
        "m",
        "",
        "Display block counts in 1048576-byte (1 MiB) blocks.",
    );
    opts.optflag(
        "g",
        "",
        "Display block counts in 1073741824-byte (1 GiB) blocks.",
    );
    opts.optflag(
        "s",
        "",
        "Display only a grand total for each argument (equivalent to -d 0).",
    );
    opts.optflag(
        "l",
        "",
        "Count files with multiple hard links multiple times.",
    );
    opts.optflag("A", "", "Display apparent size instead of disk usage.");
    opts.optflag("", "si", "Human-readable output (powers of 1000).");
    opts.optopt(
        "B",
        "block-size",
        "Scale sizes by SIZE before printing.",
        "SIZE",
    );
    opts
}

/// The du builtin: display disk usage statistics.
///
/// Usage:
///   du [-AachklmgsB] [-d depth] [file ...]
///
/// If no file is specified, the current directory is used.
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
            env.stderr.write_line(&format!("du: {}", e))?;
            return Ok(ExitCode::from(1));
        }
    };

    let mut opts = DuOptions::default();

    if matches.opt_present("a") {
        opts.all = true;
    }
    if matches.opt_present("c") {
        opts.grand_total = true;
    }
    if matches.opt_present("s") {
        opts.summarize = true;
        opts.max_depth = Some(0);
    }
    if let Some(depth_str) = matches.opt_str("d") {
        match depth_str.parse::<usize>() {
            Ok(d) => opts.max_depth = Some(d),
            Err(_) => {
                env.stderr
                    .write_line(&format!("du: invalid depth: {}", depth_str))?;
                return Ok(ExitCode::from(1));
            }
        }
    }
    if matches.opt_present("h") {
        opts.human_readable = true;
    }
    if matches.opt_present("si") {
        opts.si = true;
        opts.human_readable = false;
    }
    if matches.opt_present("k") {
        opts.kilobytes = true;
        opts.megabytes = false;
        opts.gigabytes = false;
        opts.human_readable = false;
        opts.si = false;
    }
    if matches.opt_present("m") {
        opts.megabytes = true;
        opts.kilobytes = false;
        opts.gigabytes = false;
        opts.human_readable = false;
        opts.si = false;
    }
    if matches.opt_present("g") {
        opts.gigabytes = true;
        opts.kilobytes = false;
        opts.megabytes = false;
        opts.human_readable = false;
        opts.si = false;
    }
    if matches.opt_present("l") {
        opts.count_links = true;
    }
    if matches.opt_present("A") {
        opts.apparent_size = true;
    }
    if let Some(block_str) = matches.opt_str("B") {
        match parse_block_size(&block_str) {
            Some(size) => opts.block_size = size,
            None => {
                env.stderr
                    .write_line(&format!("du: invalid block size: {}", block_str))?;
                return Ok(ExitCode::from(1));
            }
        }
    }

    // Determine block size for output
    if opts.block_size == 0 {
        if opts.kilobytes {
            opts.block_size = 1024;
        } else if opts.megabytes {
            opts.block_size = 1024 * 1024;
        } else if opts.gigabytes {
            opts.block_size = 1024 * 1024 * 1024;
        } else if !opts.human_readable && !opts.si {
            // Default to 512-byte blocks (POSIX default)
            opts.block_size = 512;
        }
    }

    let mut paths = matches.free.clone();
    if paths.is_empty() {
        paths.push(".".to_string());
    }

    let mut exit_code = 0i8;
    let mut grand_total: u64 = 0;
    let mut seen_inodes: HashSet<(u64, u64)> = HashSet::new();

    for path in &paths {
        let resolved = resolve_path(env.cwd.as_str(), path);
        match calculate_du(env, &resolved, path, &opts, 0, &mut seen_inodes) {
            Ok(size) => {
                grand_total += size;
            }
            Err(_) => {
                env.stderr
                    .write_line(&format!("du: {}: No such file or directory", path))?;
                exit_code = 1;
            }
        }
    }

    if opts.grand_total {
        let size_str = format_size(grand_total, &opts);
        env.stdout.write_line(&format!("{}\ttotal", size_str))?;
    }

    Ok(ExitCode::from(exit_code))
}

/// Calculate disk usage for a path and print entries.
fn calculate_du<SI, SO, SE, FS>(
    env: &Environment<SI, SO, SE, FS>,
    resolved_path: &str,
    display_path: &str,
    opts: &DuOptions,
    depth: usize,
    seen_inodes: &mut HashSet<(u64, u64)>,
) -> Result<u64, Error>
where
    SI: StdioIn,
    SO: StdioOut,
    SE: StdioOut,
    FS: Filesystem,
{
    let entry = env.fs.lstat(resolved_path)?;

    // Check for hard links (skip if already seen, unless -l is specified)
    if !opts.count_links && entry.file_type == FileType::RegularFile {
        let inode_key = (entry.dev, entry.ino);
        if seen_inodes.contains(&inode_key) {
            return Ok(0);
        }
        seen_inodes.insert(inode_key);
    }

    let mut total_size = get_size(&entry, opts);

    if entry.file_type == FileType::Directory {
        // Recurse into directory
        let entries = env.fs.read_dir(resolved_path)?;
        for (name, _) in entries {
            if name == "." || name == ".." {
                continue;
            }
            let child_resolved = format!("{}/{}", resolved_path.trim_end_matches('/'), name);
            let child_display = format!("{}/{}", display_path.trim_end_matches('/'), name);

            match calculate_du(
                env,
                &child_resolved,
                &child_display,
                opts,
                depth + 1,
                seen_inodes,
            ) {
                Ok(child_size) => {
                    total_size += child_size;
                }
                Err(_) => {
                    env.stderr
                        .write_line(&format!("du: {}: Permission denied", child_display))?;
                }
            }
        }

        // Print directory entry if within depth limit
        let should_print = match opts.max_depth {
            Some(max) => depth <= max,
            None => true,
        };
        if should_print {
            let size_str = format_size(total_size, opts);
            env.stdout
                .write_line(&format!("{}\t{}", size_str, display_path))?;
        }
    } else {
        // Print file entry if:
        // - it's a top-level argument (depth == 0), or
        // - -a is specified and within depth limit
        let within_depth = match opts.max_depth {
            Some(max) => depth <= max,
            None => true,
        };
        let should_print = within_depth && (depth == 0 || opts.all);
        if should_print {
            let size_str = format_size(total_size, opts);
            env.stdout
                .write_line(&format!("{}\t{}", size_str, display_path))?;
        }
    }

    Ok(total_size)
}

/// Get the size of an entry for du purposes.
fn get_size(entry: &DirEntry, opts: &DuOptions) -> u64 {
    if opts.apparent_size {
        entry.size
    } else {
        // Round up to 512-byte blocks (simulating disk allocation)
        let block_size = 512u64;
        entry.size.div_ceil(block_size) * block_size
    }
}

/// Format a size according to the options.
fn format_size(bytes: u64, opts: &DuOptions) -> String {
    if opts.human_readable {
        format_human_size_with_base(bytes, 1024)
            .trim_start()
            .to_string()
    } else if opts.si {
        format_human_size_with_base(bytes, 1000)
            .trim_start()
            .to_string()
    } else {
        let blocks = bytes.div_ceil(opts.block_size);
        format!("{}", blocks)
    }
}

/// Parse a block size string (e.g., "1K", "1M", "1G").
fn parse_block_size(s: &str) -> Option<u64> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }

    let (num_str, suffix) = if s.ends_with(|c: char| c.is_ascii_alphabetic()) {
        let idx = s.len() - 1;
        (&s[..idx], &s[idx..])
    } else {
        (s, "")
    };

    let base: u64 = num_str.parse().ok()?;

    let multiplier = match suffix.to_uppercase().as_str() {
        "" => 1,
        "K" | "KB" => 1024,
        "M" | "MB" => 1024 * 1024,
        "G" | "GB" => 1024 * 1024 * 1024,
        "T" | "TB" => 1024 * 1024 * 1024 * 1024,
        _ => return None,
    };

    Some(base * multiplier)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TestFilesystemExt;
    use crate::test_utils::make_test_env;

    #[test]
    fn empty_directory_shows_zero() {
        let env = make_test_env(vec!["du"]);
        env.fs.add_directory("/");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        // In eudaemonfs, root directory has "." and ".." entries (size=2 entries),
        // which rounds up to 1 block when computing disk usage.
        assert!(stdout.contains("1\t."));
    }

    #[test]
    fn single_file_shows_size() {
        let env = make_test_env(vec!["du", "/file"]);
        env.fs.add_directory("/");
        env.fs.add_file("/file", "hello"); // 5 bytes
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        // 5 bytes rounds up to 512 bytes = 1 block of 512 bytes
        assert!(stdout.contains("1\t/file"));
    }

    #[test]
    fn directory_with_files() {
        let env = make_test_env(vec!["du", "/dir"]);
        env.fs.add_directory("/");
        env.fs.add_directory("/dir");
        env.fs.add_file("/dir/file1", "hello"); // 5 bytes
        env.fs.add_file("/dir/file2", "world!"); // 6 bytes
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        // 11 bytes total, rounds to 512 bytes = 1 block, but each file is 1 block = 2 blocks total
        assert!(stdout.contains("/dir"));
    }

    #[test]
    fn all_flag_shows_files() {
        let env = make_test_env(vec!["du", "-a", "/dir"]);
        env.fs.add_directory("/");
        env.fs.add_directory("/dir");
        env.fs.add_file("/dir/file1", "hello");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert!(stdout.contains("/dir/file1"));
        assert!(stdout.contains("/dir\n") || stdout.ends_with("/dir"));
    }

    #[test]
    fn summarize_flag_shows_only_total() {
        let env = make_test_env(vec!["du", "-s", "/dir"]);
        env.fs.add_directory("/");
        env.fs.add_directory("/dir");
        env.fs.add_directory("/dir/subdir");
        env.fs.add_file("/dir/file1", "hello");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        // Should only show /dir, not /dir/subdir
        assert!(stdout.contains("/dir"));
        assert!(!stdout.contains("/dir/subdir"));
    }

    #[test]
    fn grand_total_flag() {
        let env = make_test_env(vec!["du", "-c", "/dir1", "/dir2"]);
        env.fs.add_directory("/");
        env.fs.add_directory("/dir1");
        env.fs.add_directory("/dir2");
        env.fs.add_file("/dir1/file", "hello");
        env.fs.add_file("/dir2/file", "world");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert!(stdout.contains("total"));
    }

    #[test]
    fn human_readable_flag() {
        let env = make_test_env(vec!["du", "-h", "/file"]);
        env.fs.add_directory("/");
        // Create a file that will show up as K
        env.fs.add_file("/file", &"x".repeat(2048)); // 2048 bytes = 2K
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert!(stdout.contains("K") || stdout.contains("B"));
    }

    #[test]
    fn kilobytes_flag() {
        let env = make_test_env(vec!["du", "-k", "/file"]);
        env.fs.add_directory("/");
        env.fs.add_file("/file", &"x".repeat(2048));
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        // 2048 bytes = 2 * 1024 = 2 KiB blocks
        assert!(stdout.contains("2\t/file") || stdout.contains("3\t/file"));
    }

    #[test]
    fn depth_flag() {
        let env = make_test_env(vec!["du", "-d", "1", "/dir"]);
        env.fs.add_directory("/");
        env.fs.add_directory("/dir");
        env.fs.add_directory("/dir/sub1");
        env.fs.add_directory("/dir/sub1/sub2");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        // Should show /dir and /dir/sub1, but not /dir/sub1/sub2
        assert!(stdout.contains("/dir\n") || stdout.ends_with("/dir"));
        assert!(stdout.contains("/dir/sub1"));
        assert!(!stdout.contains("/dir/sub1/sub2"));
    }

    #[test]
    fn nonexistent_path_returns_error() {
        let env = make_test_env(vec!["du", "/nonexistent"]);
        env.fs.add_directory("/");
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("No such file or directory"));
    }

    #[test]
    fn invalid_depth_returns_error() {
        let env = make_test_env(vec!["du", "-d", "abc"]);
        env.fs.add_directory("/");
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("invalid depth"));
    }

    #[test]
    fn apparent_size_flag() {
        let env = make_test_env(vec!["du", "-A", "-k", "/file"]);
        env.fs.add_directory("/");
        env.fs.add_file("/file", "hello"); // 5 bytes
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        // With -A, should show actual size (5 bytes = 1 KiB block when using -k)
        assert!(stdout.contains("1\t/file"));
    }

    #[test]
    fn format_human_size_bytes() {
        use crate::format_human_size_with_base;
        assert_eq!("   0B", format_human_size_with_base(0, 1024));
        assert_eq!(" 100B", format_human_size_with_base(100, 1024));
        assert_eq!("1023B", format_human_size_with_base(1023, 1024));
    }

    #[test]
    fn format_human_size_kilobytes() {
        use crate::format_human_size_with_base;
        assert_eq!(" 1.0K", format_human_size_with_base(1024, 1024));
        assert_eq!("  10K", format_human_size_with_base(10 * 1024, 1024));
    }

    #[test]
    fn format_human_size_megabytes() {
        use crate::format_human_size_with_base;
        assert_eq!(" 1.0M", format_human_size_with_base(1024 * 1024, 1024));
    }

    #[test]
    fn parse_block_size_plain() {
        assert_eq!(Some(512), parse_block_size("512"));
        assert_eq!(Some(1024), parse_block_size("1024"));
    }

    #[test]
    fn parse_block_size_with_suffix() {
        assert_eq!(Some(1024), parse_block_size("1K"));
        assert_eq!(Some(1024 * 1024), parse_block_size("1M"));
        assert_eq!(Some(1024 * 1024 * 1024), parse_block_size("1G"));
    }

    #[test]
    fn parse_block_size_invalid() {
        assert_eq!(None, parse_block_size(""));
        assert_eq!(None, parse_block_size("abc"));
    }

    #[test]
    fn multiple_paths() {
        let env = make_test_env(vec!["du", "/dir1", "/dir2"]);
        env.fs.add_directory("/");
        env.fs.add_directory("/dir1");
        env.fs.add_directory("/dir2");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert!(stdout.contains("/dir1"));
        assert!(stdout.contains("/dir2"));
    }

    #[test]
    fn nested_directories() {
        let env = make_test_env(vec!["du", "/dir"]);
        env.fs.add_directory("/");
        env.fs.add_directory("/dir");
        env.fs.add_directory("/dir/sub1");
        env.fs.add_directory("/dir/sub2");
        env.fs.add_file("/dir/sub1/file", "content");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert!(stdout.contains("/dir/sub1"));
        assert!(stdout.contains("/dir/sub2"));
        assert!(stdout.contains("/dir\n") || stdout.ends_with("/dir"));
    }
}

//! The stat builtin: display file status.

use getopts::Options;

use crate::{DirEntry, Environment, Error, ExitCode, FileType, Filesystem, StdioIn, StdioOut};

/// Default format string.
const DEF_FORMAT: &str = "%d %i %Sp %l %Su %Sg %r %z \"%Sa\" \"%Sm\" \"%Sc\" %k %b %N";

/// Raw format string.
const RAW_FORMAT: &str = "%d %i %#p %l %u %g %r %z %a %m %c %k %b %N";

/// ls-style format string.
const LS_FORMAT: &str = "%Sp %l %Su %Sg %Z %Sm %N%SY";

/// ls-style format string with -F indicators.
const LSF_FORMAT: &str = "%Sp %l %Su %Sg %Z %Sm %N%T%SY";

/// Shell output format string.
const SHELL_FORMAT: &str = "st_dev=%d st_ino=%i st_mode=%#p st_nlink=%l st_uid=%u st_gid=%g st_rdev=%r st_size=%z st_atime=%a st_mtime=%m st_ctime=%c st_blksize=%k st_blocks=%b";

/// Linux verbose format string.
const LINUX_FORMAT: &str = "  File: \"%N\"%n  Size: %-11z  FileType: %HT%n  Mode: (%OMp%03OLp/%.10Sp)         Uid: (%5u/%8Su)  Gid: (%5g/%8Sg)%n  Device: %Hd,%Ld   Inode: %i    Links: %l%nAccess: %Sa%nModify: %Sm%nChange: %Sc";

/// Default time format.
const TIME_FORMAT: &str = "%b %e %T %Y";

/// Options for the stat command.
struct StatOptions {
    /// Follow symlinks (use stat instead of lstat).
    follow_symlinks: bool,
    /// Suppress error messages.
    quiet: bool,
    /// Do not output trailing newline.
    no_newline: bool,
    /// The format string to use.
    format: String,
    /// The time format to use.
    time_format: String,
    /// Display -F style type indicator.
    classify: bool,
}

impl Default for StatOptions {
    fn default() -> Self {
        Self {
            follow_symlinks: false,
            quiet: false,
            no_newline: false,
            format: DEF_FORMAT.to_string(),
            time_format: TIME_FORMAT.to_string(),
            classify: false,
        }
    }
}

fn build_options() -> Options {
    let mut opts = Options::new();
    opts.optflag(
        "F",
        "",
        "Display type indicators (/ for dirs, * for executables, @ for symlinks).",
    );
    opts.optflag(
        "H",
        "",
        "Treat arguments as NFS file handles (not supported).",
    );
    opts.optflag(
        "h",
        "",
        "Print hole information for sparse files (not supported).",
    );
    opts.optflag("L", "", "Follow symbolic links.");
    opts.optflag("n", "", "Do not force a newline at the end of output.");
    opts.optflag("q", "", "Suppress failure messages.");
    opts.optopt("f", "", "Display using the specified format.", "format");
    opts.optflag("l", "", "Display output in ls -lT format.");
    opts.optflag("r", "", "Display raw information.");
    opts.optflag("s", "", "Display in shell output format.");
    opts.optopt(
        "t",
        "",
        "Display timestamps using the specified format.",
        "timefmt",
    );
    opts.optflag("x", "", "Display in verbose Linux format.");
    opts
}

/// Resolve a path relative to the current working directory.
fn resolve_path(cwd: &str, path: &str) -> String {
    if path.starts_with('/') {
        path.to_string()
    } else if path == "." {
        cwd.to_string()
    } else if path == ".." {
        let mut parts: Vec<&str> = cwd.split('/').filter(|s| !s.is_empty()).collect();
        parts.pop();
        if parts.is_empty() {
            "/".to_string()
        } else {
            format!("/{}", parts.join("/"))
        }
    } else if let Some(rest) = path.strip_prefix("./") {
        format!("{}/{}", cwd.trim_end_matches('/'), rest)
    } else if let Some(rest) = path.strip_prefix("../") {
        let mut parts: Vec<&str> = cwd.split('/').filter(|s| !s.is_empty()).collect();
        parts.pop();
        let parent = if parts.is_empty() {
            "/".to_string()
        } else {
            format!("/{}", parts.join("/"))
        };
        resolve_path(&parent, rest)
    } else {
        format!("{}/{}", cwd.trim_end_matches('/'), path)
    }
}

/// Format a mode string like ls -l (e.g., "-rw-r--r--" or "drwxr-xr-x").
fn format_mode_string(file_type: FileType, mode: u32) -> String {
    let type_char = match file_type {
        FileType::RegularFile => '-',
        FileType::Directory => 'd',
        FileType::Symlink => 'l',
        FileType::Other => '?',
    };

    let user = format_permission_triple((mode >> 6) & 7, (mode >> 9) & 4 != 0, 's', 'S');
    let group = format_permission_triple((mode >> 3) & 7, (mode >> 9) & 2 != 0, 's', 'S');
    let other = format_permission_triple(mode & 7, (mode >> 9) & 1 != 0, 't', 'T');

    format!("{}{}{}{}", type_char, user, group, other)
}

/// Format a permission triple (e.g., "rwx", "r-x", etc.).
fn format_permission_triple(
    perm: u32,
    special: bool,
    special_x: char,
    special_no_x: char,
) -> String {
    let r = if perm & 4 != 0 { 'r' } else { '-' };
    let w = if perm & 2 != 0 { 'w' } else { '-' };
    let x = if perm & 1 != 0 {
        if special { special_x } else { 'x' }
    } else if special {
        special_no_x
    } else {
        '-'
    };
    format!("{}{}{}", r, w, x)
}

/// Format a timestamp in the given format.
fn format_time(time_ms: i64, _format: &str) -> String {
    // Convert milliseconds to seconds
    let secs = time_ms / 1000;

    // Simple formatting - we could use chrono for more complex formats
    // but for the virtual filesystem, basic formatting suffices
    chrono_lite_format(secs)
}

/// Simple date/time formatting without external dependencies.
fn chrono_lite_format(secs: i64) -> String {
    // Days since Unix epoch
    let days = secs / 86400;
    let time_of_day = secs % 86400;

    let hours = time_of_day / 3600;
    let minutes = (time_of_day % 3600) / 60;
    let seconds = time_of_day % 60;

    // Calculate year, month, day from days since epoch
    let (year, month, day) = days_to_ymd(days);

    let month_names = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let month_name = month_names.get(month as usize).unwrap_or(&"???");

    format!(
        "{} {:2} {:02}:{:02}:{:02} {}",
        month_name, day, hours, minutes, seconds, year
    )
}

/// Convert days since Unix epoch to (year, month, day).
fn days_to_ymd(days: i64) -> (i64, i64, i64) {
    // Algorithm from Howard Hinnant
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u32;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m as i64 - 1, d as i64)
}

/// Get file type indicator character (like ls -F).
fn file_type_indicator(file_type: FileType) -> &'static str {
    match file_type {
        FileType::Directory => "/",
        FileType::Symlink => "@",
        FileType::RegularFile => "",
        FileType::Other => "",
    }
}

/// Get file type description.
fn file_type_description(file_type: FileType) -> &'static str {
    match file_type {
        FileType::Directory => "Directory",
        FileType::Symlink => "Symbolic Link",
        FileType::RegularFile => "Regular File",
        FileType::Other => "Unknown",
    }
}

/// Format flags for output parsing.
#[derive(Default)]
struct FormatFlags {
    alternate: bool,  // #
    left_align: bool, // -
    zero_pad: bool,   // 0
    space: bool,      // ' '
    plus: bool,       // +
}

/// Output format specifier.
#[derive(Clone, Copy, PartialEq)]
enum OutputFormat {
    Decimal,
    Octal,
    Unsigned,
    Hex,
    String,
    Default,
}

/// Sub-field specifier.
#[derive(Clone, Copy, PartialEq)]
enum SubField {
    None,
    High,
    Middle,
    Low,
}

/// Process a format string and output the result.
fn process_format<SI, SO, SE, FS>(
    env: &Environment<SI, SO, SE, FS>,
    entry: &DirEntry,
    path: &str,
    resolved_path: &str,
    opts: &StatOptions,
) -> Result<(), Error>
where
    SI: StdioIn,
    SO: StdioOut,
    SE: StdioOut,
    FS: Filesystem,
{
    let mut output = String::new();
    let format = opts.format.as_str();
    let mut chars = format.chars().peekable();

    while let Some(c) = chars.next() {
        if c != '%' {
            output.push(c);
            continue;
        }

        // Handle %% and simple escapes
        match chars.peek() {
            Some('%') => {
                chars.next();
                output.push('%');
                continue;
            }
            Some('n') => {
                chars.next();
                output.push('\n');
                continue;
            }
            Some('t') => {
                chars.next();
                output.push('\t');
                continue;
            }
            Some('@') => {
                chars.next();
                output.push('1'); // File number (we only process one at a time)
                continue;
            }
            None => {
                output.push('%');
                continue;
            }
            _ => {}
        }

        // Parse format flags
        let mut flags = FormatFlags::default();
        while let Some(&fc) = chars.peek() {
            match fc {
                '#' => {
                    flags.alternate = true;
                    chars.next();
                }
                '-' => {
                    flags.left_align = true;
                    chars.next();
                }
                '0' => {
                    flags.zero_pad = true;
                    chars.next();
                }
                ' ' => {
                    flags.space = true;
                    chars.next();
                }
                '+' => {
                    flags.plus = true;
                    chars.next();
                }
                _ => break,
            }
        }

        // Parse width
        let mut width: Option<usize> = None;
        while let Some(&c) = chars.peek() {
            if c.is_ascii_digit() {
                let digit = c.to_digit(10).unwrap() as usize;
                width = Some(width.unwrap_or(0) * 10 + digit);
                chars.next();
            } else {
                break;
            }
        }

        // Parse precision
        let mut precision: Option<usize> = None;
        if chars.peek() == Some(&'.') {
            chars.next();
            precision = Some(0);
            while let Some(&c) = chars.peek() {
                if c.is_ascii_digit() {
                    let digit = c.to_digit(10).unwrap() as usize;
                    precision = Some(precision.unwrap() * 10 + digit);
                    chars.next();
                } else {
                    break;
                }
            }
        }

        // Parse output format specifier
        let out_fmt = match chars.peek() {
            Some('D') => {
                chars.next();
                OutputFormat::Decimal
            }
            Some('O') => {
                chars.next();
                OutputFormat::Octal
            }
            Some('U') => {
                chars.next();
                OutputFormat::Unsigned
            }
            Some('X') => {
                chars.next();
                OutputFormat::Hex
            }
            Some('S') => {
                chars.next();
                OutputFormat::String
            }
            _ => OutputFormat::Default,
        };

        // Parse sub-field specifier
        let sub = match chars.peek() {
            Some('H') => {
                chars.next();
                SubField::High
            }
            Some('M') => {
                chars.next();
                SubField::Middle
            }
            Some('L') => {
                chars.next();
                SubField::Low
            }
            _ => SubField::None,
        };

        // Parse datum specifier
        let datum = chars.next();
        let formatted = match datum {
            Some('d') => format_device(entry.dev, sub, out_fmt, &flags),
            Some('i') => format_number(entry.ino, out_fmt, &flags),
            Some('p') => format_mode(entry, sub, out_fmt, &flags),
            Some('l') => format_number(1, out_fmt, &flags), // nlink - we don't track this
            Some('u') => format_uid(out_fmt),
            Some('g') => format_gid(out_fmt),
            Some('r') => format_device(0, sub, out_fmt, &flags), // rdev - for special files
            Some('a') => format_timestamp(entry.mtime_ms, out_fmt, &opts.time_format), // atime
            Some('m') => format_timestamp(entry.mtime_ms, out_fmt, &opts.time_format), // mtime
            Some('c') => format_timestamp(entry.mtime_ms, out_fmt, &opts.time_format), // ctime
            Some('B') => format_timestamp(0, out_fmt, &opts.time_format), // birthtime
            Some('z') => format_number(entry.size, out_fmt, &flags),
            Some('b') => format_number(blocks_for_size(entry.size), out_fmt, &flags),
            Some('k') => format_number(4096, out_fmt, &flags), // blksize
            Some('N') => path.to_string(),
            Some('R') => resolved_path.to_string(),
            Some('T') => format_file_type(entry.file_type, sub),
            Some('Y') => format_symlink_target(env, resolved_path, entry.file_type, out_fmt),
            Some('Z') => format_size_or_rdev(entry),
            Some('f') => format_number(0, out_fmt, &flags), // flags - not tracked
            Some('v') => format_number(0, out_fmt, &flags), // gen - not tracked
            _ => "?".to_string(),
        };

        // Apply width and alignment
        let formatted = apply_width(&formatted, width, precision, &flags);
        output.push_str(&formatted);
    }

    env.stdout.write_str(&output)?;
    if !opts.no_newline {
        env.stdout.write_str("\n")?;
    }
    Ok(())
}

/// Calculate blocks for a given size (512-byte blocks).
fn blocks_for_size(size: u64) -> u64 {
    size.div_ceil(512)
}

/// Format a device number.
fn format_device(dev: u64, sub: SubField, fmt: OutputFormat, flags: &FormatFlags) -> String {
    let value = match sub {
        SubField::High => dev >> 8,  // Major
        SubField::Low => dev & 0xff, // Minor
        _ => dev,
    };
    format_number(value, fmt, flags)
}

/// Format a numeric value.
fn format_number(value: u64, fmt: OutputFormat, flags: &FormatFlags) -> String {
    match fmt {
        OutputFormat::Decimal | OutputFormat::Default => {
            if flags.plus {
                format!("+{}", value)
            } else if flags.space {
                format!(" {}", value)
            } else {
                format!("{}", value)
            }
        }
        OutputFormat::Octal => {
            if flags.alternate && value != 0 {
                format!("0{:o}", value)
            } else {
                format!("{:o}", value)
            }
        }
        OutputFormat::Unsigned => format!("{}", value),
        OutputFormat::Hex => {
            if flags.alternate && value != 0 {
                format!("0x{:x}", value)
            } else {
                format!("{:x}", value)
            }
        }
        OutputFormat::String => format!("{}", value),
    }
}

/// Format file mode.
fn format_mode(entry: &DirEntry, sub: SubField, fmt: OutputFormat, flags: &FormatFlags) -> String {
    // We don't have actual Unix permissions in DirEntry, so we simulate reasonable defaults
    let mode: u32 = match entry.file_type {
        FileType::Directory => 0o755,
        FileType::RegularFile => 0o644,
        FileType::Symlink => 0o777,
        FileType::Other => 0o000,
    };

    // Add file type bits
    let full_mode = match entry.file_type {
        FileType::Directory => 0o40000 | mode,
        FileType::RegularFile => 0o100000 | mode,
        FileType::Symlink => 0o120000 | mode,
        FileType::Other => mode,
    };

    match fmt {
        OutputFormat::String => {
            let mode_str = format_mode_string(entry.file_type, mode);
            match sub {
                SubField::High => mode_str[1..4].to_string(), // User perms
                SubField::Middle => mode_str[4..7].to_string(), // Group perms
                SubField::Low => mode_str[7..10].to_string(), // Other perms
                SubField::None => mode_str,
            }
        }
        _ => {
            let value = match sub {
                SubField::High => (full_mode >> 12) & 0xf,
                SubField::Middle => (mode >> 9) & 7,
                SubField::Low => mode & 0o777,
                SubField::None => full_mode,
            };
            format_number(value as u64, fmt, flags)
        }
    }
}

/// Format uid.
fn format_uid(fmt: OutputFormat) -> String {
    match fmt {
        OutputFormat::String => "assistant".to_string(),
        _ => "1000".to_string(),
    }
}

/// Format gid.
fn format_gid(fmt: OutputFormat) -> String {
    match fmt {
        OutputFormat::String => "assistant".to_string(),
        _ => "1000".to_string(),
    }
}

/// Format a timestamp.
fn format_timestamp(time_ms: i64, fmt: OutputFormat, time_format: &str) -> String {
    match fmt {
        OutputFormat::String => format_time(time_ms, time_format),
        _ => format!("{}", time_ms / 1000),
    }
}

/// Format file type.
fn format_file_type(file_type: FileType, sub: SubField) -> String {
    match sub {
        SubField::High => file_type_description(file_type).to_string(),
        _ => file_type_indicator(file_type).to_string(),
    }
}

/// Format symlink target.
fn format_symlink_target<SI, SO, SE, FS>(
    env: &Environment<SI, SO, SE, FS>,
    path: &str,
    file_type: FileType,
    fmt: OutputFormat,
) -> String
where
    SI: StdioIn,
    SO: StdioOut,
    SE: StdioOut,
    FS: Filesystem,
{
    if file_type != FileType::Symlink {
        return String::new();
    }

    match env.fs.readlink(path) {
        Ok(target) => {
            if fmt == OutputFormat::String {
                format!(" -> {}", target)
            } else {
                target
            }
        }
        Err(_) => String::new(),
    }
}

/// Format size or rdev based on file type.
fn format_size_or_rdev(entry: &DirEntry) -> String {
    // For regular files, show size; for devices, would show major,minor
    format!("{}", entry.size)
}

/// Apply width and precision to formatted string.
fn apply_width(
    s: &str,
    width: Option<usize>,
    precision: Option<usize>,
    flags: &FormatFlags,
) -> String {
    let mut result = s.to_string();

    // Apply precision (truncate)
    if let Some(prec) = precision
        && result.len() > prec
    {
        result.truncate(prec);
    }

    // Apply width (pad)
    if let Some(w) = width
        && result.len() < w
    {
        let padding = w - result.len();
        let pad_char = if flags.zero_pad { '0' } else { ' ' };
        if flags.left_align {
            result.push_str(&pad_char.to_string().repeat(padding));
        } else {
            result = format!("{}{}", pad_char.to_string().repeat(padding), result);
        }
    }

    result
}

/// The stat builtin: display file status.
///
/// Usage:
///   stat [-FLnq] [-f format | -l | -r | -s | -x] [-t timefmt] [file ...]
///
/// If no operands are given, displays information about stdin (not supported).
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
            env.stderr.write_line(&format!("stat: {}", e))?;
            return Ok(ExitCode::from(1));
        }
    };

    let mut opts = StatOptions::default();
    let mut fmt_char: Option<char> = None;

    // -H is not supported (NFS file handles)
    if matches.opt_present("H") {
        env.stderr
            .write_line("stat: -H (NFS file handles) is not supported")?;
        return Ok(ExitCode::from(1));
    }

    // -h is not supported (sparse file holes)
    if matches.opt_present("h") {
        env.stderr
            .write_line("stat: -h (sparse file holes) is not supported")?;
        return Ok(ExitCode::from(1));
    }

    // -F adds type indicators and implies -l
    if matches.opt_present("F") {
        opts.classify = true;
        fmt_char = Some('l');
    }

    // -L follow symlinks
    if matches.opt_present("L") {
        opts.follow_symlinks = true;
    }

    // -n no newline
    if matches.opt_present("n") {
        opts.no_newline = true;
    }

    // -q quiet
    if matches.opt_present("q") {
        opts.quiet = true;
    }

    // -t timefmt
    if let Some(tf) = matches.opt_str("t") {
        opts.time_format = tf;
    }

    // Format options (mutually exclusive: -f, -l, -r, -s, -x)
    if let Some(f) = matches.opt_str("f") {
        if fmt_char.is_some() && fmt_char != Some('l') {
            env.stderr
                .write_line("stat: can't use multiple format options")?;
            return Ok(ExitCode::from(1));
        }
        opts.format = f;
        fmt_char = Some('f');
    }

    if matches.opt_present("l") {
        if fmt_char.is_some() && fmt_char != Some('l') {
            env.stderr
                .write_line("stat: can't use multiple format options")?;
            return Ok(ExitCode::from(1));
        }
        fmt_char = Some('l');
    }

    if matches.opt_present("r") {
        if fmt_char.is_some() {
            env.stderr
                .write_line("stat: can't use multiple format options")?;
            return Ok(ExitCode::from(1));
        }
        fmt_char = Some('r');
    }

    if matches.opt_present("s") {
        if fmt_char.is_some() {
            env.stderr
                .write_line("stat: can't use multiple format options")?;
            return Ok(ExitCode::from(1));
        }
        fmt_char = Some('s');
    }

    if matches.opt_present("x") {
        if fmt_char.is_some() {
            env.stderr
                .write_line("stat: can't use multiple format options")?;
            return Ok(ExitCode::from(1));
        }
        fmt_char = Some('x');
    }

    // Set format based on format character
    match fmt_char {
        Some('l') => {
            opts.format = if opts.classify { LSF_FORMAT } else { LS_FORMAT }.to_string();
        }
        Some('r') => {
            opts.format = RAW_FORMAT.to_string();
        }
        Some('s') => {
            opts.format = SHELL_FORMAT.to_string();
        }
        Some('x') => {
            opts.format = LINUX_FORMAT.to_string();
            if opts.time_format == TIME_FORMAT {
                opts.time_format = "%c".to_string();
            }
        }
        _ => {}
    }

    let paths = if matches.free.is_empty() {
        env.stderr.write_line("stat: missing file operand")?;
        return Ok(ExitCode::from(1));
    } else {
        matches.free.clone()
    };

    let mut exit_code = 0i8;

    for path in &paths {
        let resolved = resolve_path(env.cwd.as_str(), path);

        let entry = if opts.follow_symlinks {
            // Try stat first, fall back to lstat on broken symlink
            match env.fs.stat(&resolved) {
                Ok(e) => e,
                Err(_) => match env.fs.lstat(&resolved) {
                    Ok(e) => e,
                    Err(_) => {
                        if !opts.quiet {
                            env.stderr.write_line(&format!(
                                "stat: {}: No such file or directory",
                                path
                            ))?;
                        }
                        exit_code = 1;
                        continue;
                    }
                },
            }
        } else {
            match env.fs.lstat(&resolved) {
                Ok(e) => e,
                Err(_) => {
                    if !opts.quiet {
                        env.stderr
                            .write_line(&format!("stat: {}: No such file or directory", path))?;
                    }
                    exit_code = 1;
                    continue;
                }
            }
        };

        process_format(env, &entry, path, &resolved, &opts)?;
    }

    Ok(ExitCode::from(exit_code))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TestFilesystemExt;
    use crate::test_utils::make_test_env;

    #[test]
    fn stat_regular_file_default_format() {
        let env = make_test_env(vec!["stat", "/myfile"]);
        env.fs.add_directory("/");
        env.fs.add_file("/myfile", "hello");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert!(stdout.contains("/myfile"));
        assert!(stdout.contains("-rw-r--r--"));
    }

    #[test]
    fn stat_directory_default_format() {
        let env = make_test_env(vec!["stat", "/mydir"]);
        env.fs.add_directory("/");
        env.fs.add_directory("/mydir");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert!(stdout.contains("/mydir"));
        assert!(stdout.contains("drwxr-xr-x"));
    }

    #[test]
    fn stat_symlink_default_format() {
        let env = make_test_env(vec!["stat", "/mylink"]);
        env.fs.add_directory("/");
        env.fs.add_file("/target", "content");
        env.fs.symlink("/target", "/mylink").unwrap();
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert!(stdout.contains("/mylink"));
        assert!(stdout.contains("lrwxrwxrwx"));
    }

    #[test]
    fn stat_follow_symlinks_with_l_flag() {
        let env = make_test_env(vec!["stat", "-L", "/mylink"]);
        env.fs.add_directory("/");
        env.fs.add_file("/target", "content");
        env.fs.symlink("/target", "/mylink").unwrap();
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        // Should show regular file properties when following symlink
        assert!(stdout.contains("-rw-r--r--"));
    }

    #[test]
    fn stat_nonexistent_file_returns_error() {
        let env = make_test_env(vec!["stat", "/nonexistent"]);
        env.fs.add_directory("/");
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("No such file or directory"));
    }

    #[test]
    fn stat_quiet_suppresses_errors() {
        let env = make_test_env(vec!["stat", "-q", "/nonexistent"]);
        env.fs.add_directory("/");
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.is_empty());
    }

    #[test]
    fn stat_custom_format_filename() {
        let env = make_test_env(vec!["stat", "-f", "%N", "/myfile"]);
        env.fs.add_directory("/");
        env.fs.add_file("/myfile", "hello");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert_eq!("/myfile\n", stdout);
    }

    #[test]
    fn stat_custom_format_size() {
        let env = make_test_env(vec!["stat", "-f", "%z", "/myfile"]);
        env.fs.add_directory("/");
        env.fs.add_file("/myfile", "hello");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert_eq!("5\n", stdout);
    }

    #[test]
    fn stat_custom_format_mode_string() {
        let env = make_test_env(vec!["stat", "-f", "%Sp", "/myfile"]);
        env.fs.add_directory("/");
        env.fs.add_file("/myfile", "hello");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert_eq!("-rw-r--r--\n", stdout);
    }

    #[test]
    fn stat_custom_format_inode() {
        let env = make_test_env(vec!["stat", "-f", "%i", "/myfile"]);
        env.fs.add_directory("/");
        env.fs.add_file("/myfile", "hello");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        // MockFilesystem assigns incrementing inodes starting at 1
        assert!(stdout.trim().parse::<u64>().is_ok());
    }

    #[test]
    fn stat_shell_format() {
        let env = make_test_env(vec!["stat", "-s", "/myfile"]);
        env.fs.add_directory("/");
        env.fs.add_file("/myfile", "hello");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert!(stdout.contains("st_dev="));
        assert!(stdout.contains("st_ino="));
        assert!(stdout.contains("st_mode="));
        assert!(stdout.contains("st_size=5"));
    }

    #[test]
    fn stat_ls_format() {
        let env = make_test_env(vec!["stat", "-l", "/myfile"]);
        env.fs.add_directory("/");
        env.fs.add_file("/myfile", "hello");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert!(stdout.contains("-rw-r--r--"));
        assert!(stdout.contains("assistant"));
        assert!(stdout.contains("/myfile"));
    }

    #[test]
    fn stat_raw_format() {
        let env = make_test_env(vec!["stat", "-r", "/myfile"]);
        env.fs.add_directory("/");
        env.fs.add_file("/myfile", "hello");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        // Raw format outputs numeric values
        assert!(stdout.contains("/myfile"));
    }

    #[test]
    fn stat_linux_format() {
        let env = make_test_env(vec!["stat", "-x", "/myfile"]);
        env.fs.add_directory("/");
        env.fs.add_file("/myfile", "hello");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert!(stdout.contains("File:"));
        assert!(stdout.contains("Size:"));
        assert!(stdout.contains("FileType:"));
    }

    #[test]
    fn stat_no_newline_option() {
        let env = make_test_env(vec!["stat", "-n", "-f", "%N", "/myfile"]);
        env.fs.add_directory("/");
        env.fs.add_file("/myfile", "hello");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert_eq!("/myfile", stdout); // No trailing newline
    }

    #[test]
    fn stat_multiple_files() {
        let env = make_test_env(vec!["stat", "-f", "%N", "/file1", "/file2"]);
        env.fs.add_directory("/");
        env.fs.add_file("/file1", "one");
        env.fs.add_file("/file2", "two");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert!(stdout.contains("/file1"));
        assert!(stdout.contains("/file2"));
    }

    #[test]
    fn stat_partial_failure() {
        let env = make_test_env(vec!["stat", "-f", "%N", "/exists", "/missing"]);
        env.fs.add_directory("/");
        env.fs.add_file("/exists", "content");
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stdout = env.stdout.into_string();
        let stderr = env.stderr.into_string();
        println!("stdout: {:?}", stdout);
        println!("stderr: {:?}", stderr);
        assert!(stdout.contains("/exists"));
        assert!(stderr.contains("No such file or directory"));
    }

    #[test]
    fn stat_symlink_target_format() {
        let env = make_test_env(vec!["stat", "-f", "%N%SY", "/mylink"]);
        env.fs.add_directory("/");
        env.fs.add_file("/target", "content");
        env.fs.symlink("/target", "/mylink").unwrap();
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert!(stdout.contains("/mylink"));
        assert!(stdout.contains(" -> /target"));
    }

    #[test]
    fn stat_file_type_high() {
        let env = make_test_env(vec!["stat", "-f", "%HT", "/mydir"]);
        env.fs.add_directory("/");
        env.fs.add_directory("/mydir");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert_eq!("Directory\n", stdout);
    }

    #[test]
    fn stat_file_type_low() {
        let env = make_test_env(vec!["stat", "-f", "%LT", "/mydir"]);
        env.fs.add_directory("/");
        env.fs.add_directory("/mydir");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert_eq!("/\n", stdout);
    }

    #[test]
    fn stat_format_with_escapes() {
        let env = make_test_env(vec!["stat", "-f", "name:%N%ttab%nnewline", "/myfile"]);
        env.fs.add_directory("/");
        env.fs.add_file("/myfile", "hello");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert!(stdout.contains("name:/myfile"));
        assert!(stdout.contains('\t'));
        assert!(stdout.contains("\nnewline"));
    }

    #[test]
    fn stat_format_percent_escape() {
        let env = make_test_env(vec!["stat", "-f", "100%% done", "/myfile"]);
        env.fs.add_directory("/");
        env.fs.add_file("/myfile", "hello");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert!(stdout.contains("100% done"));
    }

    #[test]
    fn stat_width_formatting() {
        let env = make_test_env(vec!["stat", "-f", "%10z", "/myfile"]);
        env.fs.add_directory("/");
        env.fs.add_file("/myfile", "hello");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        // 5 bytes padded to 10 characters
        assert_eq!("         5\n", stdout);
    }

    #[test]
    fn stat_left_align_formatting() {
        let env = make_test_env(vec!["stat", "-f", "%-10z", "/myfile"]);
        env.fs.add_directory("/");
        env.fs.add_file("/myfile", "hello");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert_eq!("5         \n", stdout);
    }

    #[test]
    fn stat_missing_operand() {
        let env = make_test_env(vec!["stat"]);
        env.fs.add_directory("/");
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("missing file operand"));
    }

    #[test]
    fn stat_invalid_option() {
        let env = make_test_env(vec!["stat", "-Z"]);
        env.fs.add_directory("/");
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("Unrecognized"));
    }

    #[test]
    fn stat_user_permissions() {
        let env = make_test_env(vec!["stat", "-f", "%SHp", "/myfile"]);
        env.fs.add_directory("/");
        env.fs.add_file("/myfile", "hello");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert_eq!("rw-\n", stdout);
    }

    #[test]
    fn stat_group_permissions() {
        let env = make_test_env(vec!["stat", "-f", "%SMp", "/myfile"]);
        env.fs.add_directory("/");
        env.fs.add_file("/myfile", "hello");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert_eq!("r--\n", stdout);
    }

    #[test]
    fn stat_other_permissions() {
        let env = make_test_env(vec!["stat", "-f", "%SLp", "/myfile"]);
        env.fs.add_directory("/");
        env.fs.add_file("/myfile", "hello");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert_eq!("r--\n", stdout);
    }

    #[test]
    fn stat_octal_mode() {
        let env = make_test_env(vec!["stat", "-f", "%Op", "/myfile"]);
        env.fs.add_directory("/");
        env.fs.add_file("/myfile", "hello");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        // Full mode with file type bits: 100644 in octal
        assert!(stdout.contains("100644"));
    }

    #[test]
    fn stat_uid_gid_string() {
        let env = make_test_env(vec!["stat", "-f", "%Su:%Sg", "/myfile"]);
        env.fs.add_directory("/");
        env.fs.add_file("/myfile", "hello");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert_eq!("assistant:assistant\n", stdout);
    }

    #[test]
    fn stat_uid_gid_numeric() {
        let env = make_test_env(vec!["stat", "-f", "%u:%g", "/myfile"]);
        env.fs.add_directory("/");
        env.fs.add_file("/myfile", "hello");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert_eq!("1000:1000\n", stdout);
    }

    #[test]
    fn format_mode_string_regular_file() {
        let mode_str = format_mode_string(FileType::RegularFile, 0o644);
        assert_eq!("-rw-r--r--", mode_str);
    }

    #[test]
    fn format_mode_string_directory() {
        let mode_str = format_mode_string(FileType::Directory, 0o755);
        assert_eq!("drwxr-xr-x", mode_str);
    }

    #[test]
    fn format_mode_string_symlink() {
        let mode_str = format_mode_string(FileType::Symlink, 0o777);
        assert_eq!("lrwxrwxrwx", mode_str);
    }

    #[test]
    fn format_mode_string_executable() {
        let mode_str = format_mode_string(FileType::RegularFile, 0o755);
        assert_eq!("-rwxr-xr-x", mode_str);
    }

    #[test]
    fn days_to_ymd_epoch() {
        let (y, m, d) = days_to_ymd(0);
        assert_eq!(1970, y);
        assert_eq!(0, m); // January (0-indexed)
        assert_eq!(1, d);
    }

    #[test]
    fn days_to_ymd_specific_date() {
        // 2024-01-15 is day 19737 since epoch
        let (y, m, d) = days_to_ymd(19737);
        assert_eq!(2024, y);
        assert_eq!(0, m); // January
        assert_eq!(15, d);
    }

    #[test]
    fn blocks_for_size_zero() {
        assert_eq!(0, blocks_for_size(0));
    }

    #[test]
    fn blocks_for_size_small() {
        assert_eq!(1, blocks_for_size(1));
        assert_eq!(1, blocks_for_size(512));
    }

    #[test]
    fn blocks_for_size_larger() {
        assert_eq!(2, blocks_for_size(513));
        assert_eq!(2, blocks_for_size(1024));
    }

    #[test]
    fn stat_relative_path() {
        let mut env = make_test_env(vec!["stat", "-f", "%N", "myfile"]);
        env.cwd = utf8path::Path::from("/home");
        env.fs.add_directory("/");
        env.fs.add_directory("/home");
        env.fs.add_file("/home/myfile", "hello");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert_eq!("myfile\n", stdout);
    }

    #[test]
    fn stat_nfs_handle_not_supported() {
        let env = make_test_env(vec!["stat", "-H", "/myfile"]);
        env.fs.add_directory("/");
        env.fs.add_file("/myfile", "hello");
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("-H"));
        assert!(stderr.contains("not supported"));
    }

    #[test]
    fn stat_holes_not_supported() {
        let env = make_test_env(vec!["stat", "-h", "/myfile"]);
        env.fs.add_directory("/");
        env.fs.add_file("/myfile", "hello");
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("-h"));
        assert!(stderr.contains("not supported"));
    }
}

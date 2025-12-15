use getopts::Options;

use crate::{Environment, Error, ExitCode, Filesystem, FsError, Stderr, Stdin, Stdout};

/// Parse a size string with optional K/M/G/T suffix.
/// Returns the size in bytes.
fn parse_size(s: &str) -> Result<u64, String> {
    if s.is_empty() {
        return Err("empty size".to_string());
    }

    let s = s.trim();
    let (num_part, multiplier) = match s.chars().last() {
        Some('k' | 'K') => (&s[..s.len() - 1], 1024u64),
        Some('m' | 'M') => (&s[..s.len() - 1], 1024u64 * 1024),
        Some('g' | 'G') => (&s[..s.len() - 1], 1024u64 * 1024 * 1024),
        Some('t' | 'T') => (&s[..s.len() - 1], 1024u64 * 1024 * 1024 * 1024),
        _ => (s, 1u64),
    };

    let num: u64 = num_part
        .parse()
        .map_err(|_| format!("invalid size: {}", s))?;

    num.checked_mul(multiplier)
        .ok_or_else(|| format!("size too large: {}", s))
}

/// Size specification for truncate.
#[derive(Debug, Clone, Copy)]
enum SizeSpec {
    /// Absolute size.
    Absolute(u64),
    /// Extend by this amount.
    Extend(u64),
    /// Reduce by this amount (to minimum of 0).
    Reduce(u64),
    /// Round up to multiple of this amount.
    RoundUp(u64),
    /// Round down to multiple of this amount.
    RoundDown(u64),
}

/// Parse a size specification with optional +/-/%// prefix.
fn parse_size_spec(s: &str) -> Result<SizeSpec, String> {
    if s.is_empty() {
        return Err("empty size".to_string());
    }

    let first = s.chars().next().unwrap();
    match first {
        '+' => {
            let size = parse_size(&s[1..])?;
            Ok(SizeSpec::Extend(size))
        }
        '-' => {
            let size = parse_size(&s[1..])?;
            Ok(SizeSpec::Reduce(size))
        }
        '%' => {
            let size = parse_size(&s[1..])?;
            if size == 0 {
                return Err("round size cannot be zero".to_string());
            }
            Ok(SizeSpec::RoundUp(size))
        }
        '/' => {
            let size = parse_size(&s[1..])?;
            if size == 0 {
                return Err("round size cannot be zero".to_string());
            }
            Ok(SizeSpec::RoundDown(size))
        }
        _ => {
            let size = parse_size(s)?;
            Ok(SizeSpec::Absolute(size))
        }
    }
}

/// Calculate the target size given current size and size spec.
fn calculate_target_size(current: u64, spec: SizeSpec) -> u64 {
    match spec {
        SizeSpec::Absolute(size) => size,
        SizeSpec::Extend(delta) => current.saturating_add(delta),
        SizeSpec::Reduce(delta) => current.saturating_sub(delta),
        SizeSpec::RoundUp(multiple) => {
            if current.is_multiple_of(multiple) {
                current
            } else {
                ((current / multiple) + 1) * multiple
            }
        }
        SizeSpec::RoundDown(multiple) => (current / multiple) * multiple,
    }
}

fn build_options() -> Options {
    let mut opts = Options::new();
    opts.optflag("c", "", "Do not create files if they do not exist");
    opts.optflag("d", "", "Zero a region in the file (punch hole)");
    opts.optopt("l", "", "Length of region for -d", "length");
    opts.optopt("o", "", "Offset for -d (default 0)", "offset");
    opts.optopt("r", "", "Truncate to size of reference file", "rfile");
    opts.optopt("s", "", "Size specification [+|-|%|/]size[K|M|G|T]", "size");
    opts
}

/// The truncate builtin: resize files.
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
            env.stderr.write_line(&format!("truncate: {}", e))?;
            return Ok(ExitCode::from(1));
        }
    };

    let no_create = matches.opt_present("c");
    let do_dealloc = matches.opt_present("d");
    let ref_file = matches.opt_str("r");
    let size_arg = matches.opt_str("s");
    let offset_arg = matches.opt_str("o");
    let length_arg = matches.opt_str("l");

    // Exactly one of -r, -s, or -d must be specified
    let have_ref = ref_file.is_some();
    let have_size = size_arg.is_some();
    let mode_count = have_ref as u8 + have_size as u8 + do_dealloc as u8;

    if mode_count == 0 {
        env.stderr
            .write_line("truncate: one of -d, -r, or -s must be specified")?;
        return Ok(ExitCode::from(1));
    }

    if mode_count > 1 {
        env.stderr
            .write_line("truncate: -d, -r, and -s are mutually exclusive")?;
        return Ok(ExitCode::from(1));
    }

    if matches.free.is_empty() {
        env.stderr.write_line("truncate: no files specified")?;
        return Ok(ExitCode::from(1));
    }

    // Handle deallocation mode
    if do_dealloc {
        // -l is required for -d
        let length = match length_arg {
            Some(ref l) => match parse_size(l) {
                Ok(len) if len > 0 => len,
                Ok(_) => {
                    env.stderr
                        .write_line("truncate: -l length must be greater than 0")?;
                    return Ok(ExitCode::from(1));
                }
                Err(e) => {
                    env.stderr
                        .write_line(&format!("truncate: invalid length: {}", e))?;
                    return Ok(ExitCode::from(1));
                }
            },
            None => {
                env.stderr
                    .write_line("truncate: -l is required when using -d")?;
                return Ok(ExitCode::from(1));
            }
        };

        // -o defaults to 0
        let offset = match offset_arg {
            Some(ref o) => match parse_size(o) {
                Ok(off) => off,
                Err(e) => {
                    env.stderr
                        .write_line(&format!("truncate: invalid offset: {}", e))?;
                    return Ok(ExitCode::from(1));
                }
            },
            None => 0,
        };

        let mut exit_code: i8 = 0;

        for file in &matches.free {
            if no_create && !env.fs.exists(file) {
                continue;
            }

            // For dealloc, file must exist (we don't create files in -d mode)
            if !env.fs.exists(file) {
                env.stderr
                    .write_line(&format!("truncate: {}: No such file or directory", file))?;
                exit_code = 1;
                continue;
            }

            if let Err(FsError::Io(e)) = env.fs.punch_hole(file, offset, length) {
                env.stderr
                    .write_line(&format!("truncate: {}: {}", file, e))?;
                exit_code = 1;
            }
        }

        return Ok(ExitCode::from(exit_code));
    }

    // Determine the size spec for -r or -s modes
    let size_spec = if let Some(ref rfile) = ref_file {
        match env.fs.metadata(rfile) {
            Ok(meta) => SizeSpec::Absolute(meta.size),
            Err(FsError::Io(e)) => {
                env.stderr
                    .write_line(&format!("truncate: {}: {}", rfile, e))?;
                return Ok(ExitCode::from(1));
            }
        }
    } else {
        match parse_size_spec(size_arg.as_ref().unwrap()) {
            Ok(spec) => spec,
            Err(e) => {
                env.stderr.write_line(&format!("truncate: {}", e))?;
                return Ok(ExitCode::from(1));
            }
        }
    };

    let mut exit_code: i8 = 0;

    for file in &matches.free {
        // For relative/rounding sizes, we need the current file size
        let needs_current_size = !matches!(size_spec, SizeSpec::Absolute(_));

        if no_create {
            // Don't create files
            if !env.fs.exists(file) {
                // Silently skip non-existent files with -c
                continue;
            }

            let target_size = if needs_current_size {
                match env.fs.metadata(file) {
                    Ok(meta) => calculate_target_size(meta.size, size_spec),
                    Err(FsError::Io(e)) => {
                        env.stderr
                            .write_line(&format!("truncate: {}: {}", file, e))?;
                        exit_code = 1;
                        continue;
                    }
                }
            } else if let SizeSpec::Absolute(size) = size_spec {
                size
            } else {
                unreachable!()
            };

            match env.fs.truncate_existing(file, target_size) {
                Ok(true) => {}
                Ok(false) => {
                    // File doesn't exist, silently skip with -c
                }
                Err(FsError::Io(e)) => {
                    env.stderr
                        .write_line(&format!("truncate: {}: {}", file, e))?;
                    exit_code = 1;
                }
            }
        } else {
            // Create files if they don't exist
            let target_size = if needs_current_size {
                let current_size = if env.fs.exists(file) {
                    match env.fs.metadata(file) {
                        Ok(meta) => meta.size,
                        Err(FsError::Io(e)) => {
                            env.stderr
                                .write_line(&format!("truncate: {}: {}", file, e))?;
                            exit_code = 1;
                            continue;
                        }
                    }
                } else {
                    0 // New file starts at size 0
                };
                calculate_target_size(current_size, size_spec)
            } else if let SizeSpec::Absolute(size) = size_spec {
                size
            } else {
                unreachable!()
            };

            if let Err(FsError::Io(e)) = env.fs.truncate(file, target_size) {
                env.stderr
                    .write_line(&format!("truncate: {}: {}", file, e))?;
                exit_code = 1;
            }
        }
    }

    Ok(ExitCode::from(exit_code))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::make_test_env;

    // ========================================================================
    // parse_size tests
    // ========================================================================

    #[test]
    fn parse_size_plain_number() {
        assert_eq!(parse_size("0").unwrap(), 0);
        assert_eq!(parse_size("1").unwrap(), 1);
        assert_eq!(parse_size("100").unwrap(), 100);
        assert_eq!(parse_size("1234567890").unwrap(), 1234567890);
    }

    #[test]
    fn parse_size_kilobytes() {
        assert_eq!(parse_size("1K").unwrap(), 1024);
        assert_eq!(parse_size("1k").unwrap(), 1024);
        assert_eq!(parse_size("10K").unwrap(), 10240);
    }

    #[test]
    fn parse_size_megabytes() {
        assert_eq!(parse_size("1M").unwrap(), 1024 * 1024);
        assert_eq!(parse_size("1m").unwrap(), 1024 * 1024);
        assert_eq!(parse_size("10M").unwrap(), 10 * 1024 * 1024);
    }

    #[test]
    fn parse_size_gigabytes() {
        assert_eq!(parse_size("1G").unwrap(), 1024 * 1024 * 1024);
        assert_eq!(parse_size("1g").unwrap(), 1024 * 1024 * 1024);
    }

    #[test]
    fn parse_size_terabytes() {
        assert_eq!(parse_size("1T").unwrap(), 1024u64 * 1024 * 1024 * 1024);
        assert_eq!(parse_size("1t").unwrap(), 1024u64 * 1024 * 1024 * 1024);
    }

    #[test]
    fn parse_size_empty() {
        assert!(parse_size("").is_err());
    }

    #[test]
    fn parse_size_invalid() {
        assert!(parse_size("abc").is_err());
        assert!(parse_size("-1").is_err());
        assert!(parse_size("1X").is_err());
    }

    // ========================================================================
    // parse_size_spec tests
    // ========================================================================

    #[test]
    fn parse_size_spec_absolute() {
        assert!(matches!(
            parse_size_spec("100").unwrap(),
            SizeSpec::Absolute(100)
        ));
        assert!(matches!(
            parse_size_spec("1K").unwrap(),
            SizeSpec::Absolute(1024)
        ));
    }

    #[test]
    fn parse_size_spec_extend() {
        assert!(matches!(
            parse_size_spec("+100").unwrap(),
            SizeSpec::Extend(100)
        ));
        assert!(matches!(
            parse_size_spec("+1M").unwrap(),
            SizeSpec::Extend(1048576)
        ));
    }

    #[test]
    fn parse_size_spec_reduce() {
        assert!(matches!(
            parse_size_spec("-100").unwrap(),
            SizeSpec::Reduce(100)
        ));
        assert!(matches!(
            parse_size_spec("-1K").unwrap(),
            SizeSpec::Reduce(1024)
        ));
    }

    #[test]
    fn parse_size_spec_round_up() {
        assert!(matches!(
            parse_size_spec("%100").unwrap(),
            SizeSpec::RoundUp(100)
        ));
        assert!(matches!(
            parse_size_spec("%1K").unwrap(),
            SizeSpec::RoundUp(1024)
        ));
    }

    #[test]
    fn parse_size_spec_round_down() {
        assert!(matches!(
            parse_size_spec("/100").unwrap(),
            SizeSpec::RoundDown(100)
        ));
        assert!(matches!(
            parse_size_spec("/1K").unwrap(),
            SizeSpec::RoundDown(1024)
        ));
    }

    #[test]
    fn parse_size_spec_round_zero_error() {
        assert!(parse_size_spec("%0").is_err());
        assert!(parse_size_spec("/0").is_err());
    }

    // ========================================================================
    // calculate_target_size tests
    // ========================================================================

    #[test]
    fn calculate_absolute() {
        assert_eq!(calculate_target_size(100, SizeSpec::Absolute(50)), 50);
        assert_eq!(calculate_target_size(100, SizeSpec::Absolute(200)), 200);
        assert_eq!(calculate_target_size(100, SizeSpec::Absolute(100)), 100);
    }

    #[test]
    fn calculate_extend() {
        assert_eq!(calculate_target_size(100, SizeSpec::Extend(50)), 150);
        assert_eq!(calculate_target_size(0, SizeSpec::Extend(100)), 100);
    }

    #[test]
    fn calculate_reduce() {
        assert_eq!(calculate_target_size(100, SizeSpec::Reduce(50)), 50);
        assert_eq!(calculate_target_size(100, SizeSpec::Reduce(100)), 0);
        assert_eq!(calculate_target_size(100, SizeSpec::Reduce(200)), 0);
    }

    #[test]
    fn calculate_round_up() {
        assert_eq!(calculate_target_size(100, SizeSpec::RoundUp(100)), 100);
        assert_eq!(calculate_target_size(101, SizeSpec::RoundUp(100)), 200);
        assert_eq!(calculate_target_size(199, SizeSpec::RoundUp(100)), 200);
        assert_eq!(calculate_target_size(200, SizeSpec::RoundUp(100)), 200);
        assert_eq!(calculate_target_size(0, SizeSpec::RoundUp(100)), 0);
    }

    #[test]
    fn calculate_round_down() {
        assert_eq!(calculate_target_size(100, SizeSpec::RoundDown(100)), 100);
        assert_eq!(calculate_target_size(101, SizeSpec::RoundDown(100)), 100);
        assert_eq!(calculate_target_size(199, SizeSpec::RoundDown(100)), 100);
        assert_eq!(calculate_target_size(200, SizeSpec::RoundDown(100)), 200);
        assert_eq!(calculate_target_size(99, SizeSpec::RoundDown(100)), 0);
    }

    // ========================================================================
    // truncate builtin tests - basic usage
    // ========================================================================

    #[test]
    fn no_options_shows_error() {
        let env = make_test_env(vec!["truncate", "file.txt"]);
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("-d, -r, or -s must be specified"));
    }

    #[test]
    fn no_files_shows_error() {
        let env = make_test_env(vec!["truncate", "-s", "100"]);
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("no files specified"));
    }

    #[test]
    fn both_r_and_s_shows_error() {
        let env = make_test_env(vec!["truncate", "-r", "ref.txt", "-s", "100", "file.txt"]);
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("mutually exclusive"));
    }

    #[test]
    fn d_and_s_shows_error() {
        let env = make_test_env(vec!["truncate", "-d", "-s", "100", "-l", "10", "file.txt"]);
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("mutually exclusive"));
    }

    #[test]
    fn d_and_r_shows_error() {
        let env = make_test_env(vec![
            "truncate", "-d", "-r", "ref.txt", "-l", "10", "file.txt",
        ]);
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("mutually exclusive"));
    }

    // ========================================================================
    // truncate -s tests
    // ========================================================================

    #[test]
    fn truncate_s_creates_file() {
        let env = make_test_env(vec!["truncate", "-s", "10", "newfile.txt"]);
        assert!(!env.fs.exists("newfile.txt"));
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert!(env.fs.exists("newfile.txt"));
        assert_eq!(env.fs.metadata("newfile.txt").unwrap().size, 10);
    }

    #[test]
    fn truncate_s_absolute_shrinks() {
        let env = make_test_env(vec!["truncate", "-s", "5", "file.txt"]);
        env.fs.add_file("file.txt", "0123456789");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!(env.fs.metadata("file.txt").unwrap().size, 5);
        assert_eq!(env.fs.read_to_string("file.txt").unwrap(), "01234");
    }

    #[test]
    fn truncate_s_absolute_extends() {
        let env = make_test_env(vec!["truncate", "-s", "15", "file.txt"]);
        env.fs.add_file("file.txt", "0123456789");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!(env.fs.metadata("file.txt").unwrap().size, 15);
        assert_eq!(
            env.fs.read_to_string("file.txt").unwrap(),
            "0123456789\0\0\0\0\0"
        );
    }

    #[test]
    fn truncate_s_with_suffix() {
        let env = make_test_env(vec!["truncate", "-s", "1K", "file.txt"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!(env.fs.metadata("file.txt").unwrap().size, 1024);
    }

    #[test]
    fn truncate_s_extend() {
        let env = make_test_env(vec!["truncate", "-s", "+5", "file.txt"]);
        env.fs.add_file("file.txt", "0123456789");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!(env.fs.metadata("file.txt").unwrap().size, 15);
    }

    #[test]
    fn truncate_s_reduce() {
        let env = make_test_env(vec!["truncate", "-s", "-5", "file.txt"]);
        env.fs.add_file("file.txt", "0123456789");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!(env.fs.metadata("file.txt").unwrap().size, 5);
    }

    #[test]
    fn truncate_s_reduce_below_zero() {
        let env = make_test_env(vec!["truncate", "-s", "-100", "file.txt"]);
        env.fs.add_file("file.txt", "0123456789");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!(env.fs.metadata("file.txt").unwrap().size, 0);
    }

    #[test]
    fn truncate_s_round_up() {
        let env = make_test_env(vec!["truncate", "-s", "%100", "file.txt"]);
        env.fs.add_file("file.txt", "0123456789"); // 10 bytes
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!(env.fs.metadata("file.txt").unwrap().size, 100);
    }

    #[test]
    fn truncate_s_round_up_already_multiple() {
        let env = make_test_env(vec!["truncate", "-s", "%10", "file.txt"]);
        env.fs.add_file("file.txt", "0123456789"); // 10 bytes
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!(env.fs.metadata("file.txt").unwrap().size, 10);
    }

    #[test]
    fn truncate_s_round_down() {
        let env = make_test_env(vec!["truncate", "-s", "/100", "file.txt"]);
        env.fs.add_file("file.txt", "x".repeat(150).as_str()); // 150 bytes
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!(env.fs.metadata("file.txt").unwrap().size, 100);
    }

    #[test]
    fn truncate_s_round_down_to_zero() {
        let env = make_test_env(vec!["truncate", "-s", "/100", "file.txt"]);
        env.fs.add_file("file.txt", "0123456789"); // 10 bytes
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!(env.fs.metadata("file.txt").unwrap().size, 0);
    }

    // ========================================================================
    // truncate -c tests
    // ========================================================================

    #[test]
    fn truncate_c_does_not_create() {
        let env = make_test_env(vec!["truncate", "-c", "-s", "100", "nonexistent.txt"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert!(!env.fs.exists("nonexistent.txt"));
    }

    #[test]
    fn truncate_c_modifies_existing() {
        let env = make_test_env(vec!["truncate", "-c", "-s", "5", "file.txt"]);
        env.fs.add_file("file.txt", "0123456789");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!(env.fs.metadata("file.txt").unwrap().size, 5);
    }

    #[test]
    fn truncate_c_extend_nonexistent() {
        let env = make_test_env(vec!["truncate", "-c", "-s", "+100", "nonexistent.txt"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert!(!env.fs.exists("nonexistent.txt"));
    }

    // ========================================================================
    // truncate -r tests
    // ========================================================================

    #[test]
    fn truncate_r_reference_file() {
        let env = make_test_env(vec!["truncate", "-r", "ref.txt", "target.txt"]);
        env.fs.add_file("ref.txt", "0123456789"); // 10 bytes
        env.fs.add_file("target.txt", "abc"); // 3 bytes
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!(env.fs.metadata("target.txt").unwrap().size, 10);
    }

    #[test]
    fn truncate_r_creates_file() {
        let env = make_test_env(vec!["truncate", "-r", "ref.txt", "newfile.txt"]);
        env.fs.add_file("ref.txt", "0123456789"); // 10 bytes
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert!(env.fs.exists("newfile.txt"));
        assert_eq!(env.fs.metadata("newfile.txt").unwrap().size, 10);
    }

    #[test]
    fn truncate_r_nonexistent_ref() {
        let env = make_test_env(vec!["truncate", "-r", "nonexistent.txt", "file.txt"]);
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("nonexistent.txt"));
    }

    #[test]
    fn truncate_r_with_c_flag() {
        let env = make_test_env(vec!["truncate", "-c", "-r", "ref.txt", "nonexistent.txt"]);
        env.fs.add_file("ref.txt", "0123456789");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert!(!env.fs.exists("nonexistent.txt"));
    }

    // ========================================================================
    // multiple files tests
    // ========================================================================

    #[test]
    fn truncate_multiple_files() {
        let env = make_test_env(vec!["truncate", "-s", "5", "a.txt", "b.txt", "c.txt"]);
        env.fs.add_file("a.txt", "0123456789");
        env.fs.add_file("b.txt", "0123456789");
        env.fs.add_file("c.txt", "0123456789");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!(env.fs.metadata("a.txt").unwrap().size, 5);
        assert_eq!(env.fs.metadata("b.txt").unwrap().size, 5);
        assert_eq!(env.fs.metadata("c.txt").unwrap().size, 5);
    }

    #[test]
    fn truncate_multiple_files_creates_missing() {
        let env = make_test_env(vec!["truncate", "-s", "10", "existing.txt", "new.txt"]);
        env.fs.add_file("existing.txt", "abc");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!(env.fs.metadata("existing.txt").unwrap().size, 10);
        assert!(env.fs.exists("new.txt"));
        assert_eq!(env.fs.metadata("new.txt").unwrap().size, 10);
    }

    #[test]
    fn truncate_extend_new_file() {
        let env = make_test_env(vec!["truncate", "-s", "+100", "newfile.txt"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert!(env.fs.exists("newfile.txt"));
        assert_eq!(env.fs.metadata("newfile.txt").unwrap().size, 100);
    }

    // ========================================================================
    // error handling tests
    // ========================================================================

    #[test]
    fn truncate_invalid_size() {
        let env = make_test_env(vec!["truncate", "-s", "abc", "file.txt"]);
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("invalid size"));
    }

    #[test]
    fn truncate_invalid_size_negative() {
        let env = make_test_env(vec!["truncate", "-s", "--5", "file.txt"]);
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
    }

    // ========================================================================
    // truncate -d (deallocation/hole punch) tests
    // ========================================================================

    #[test]
    fn dealloc_requires_length() {
        let env = make_test_env(vec!["truncate", "-d", "file.txt"]);
        env.fs.add_file("file.txt", "0123456789");
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("-l is required"));
    }

    #[test]
    fn dealloc_length_must_be_positive() {
        let env = make_test_env(vec!["truncate", "-d", "-l", "0", "file.txt"]);
        env.fs.add_file("file.txt", "0123456789");
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("greater than 0"));
    }

    #[test]
    fn dealloc_punches_hole_at_start() {
        let env = make_test_env(vec!["truncate", "-d", "-l", "5", "file.txt"]);
        env.fs.add_file("file.txt", "0123456789");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!(
            env.fs.read_to_string("file.txt").unwrap(),
            "\0\0\0\0\x0056789"
        );
    }

    #[test]
    fn dealloc_punches_hole_at_offset() {
        let env = make_test_env(vec!["truncate", "-d", "-o", "3", "-l", "4", "file.txt"]);
        env.fs.add_file("file.txt", "0123456789");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!(
            env.fs.read_to_string("file.txt").unwrap(),
            "012\0\0\0\x00789"
        );
    }

    #[test]
    fn dealloc_punches_hole_at_end() {
        let env = make_test_env(vec!["truncate", "-d", "-o", "7", "-l", "3", "file.txt"]);
        env.fs.add_file("file.txt", "0123456789");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!(env.fs.read_to_string("file.txt").unwrap(), "0123456\0\0\0");
    }

    #[test]
    fn dealloc_extends_file_if_needed() {
        let env = make_test_env(vec!["truncate", "-d", "-o", "8", "-l", "5", "file.txt"]);
        env.fs.add_file("file.txt", "0123456789"); // 10 bytes
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        // offset 8, length 5 -> extends to 13 bytes
        let contents = env.fs.read_to_string("file.txt").unwrap();
        println!("contents: {:?}", contents);
        assert_eq!(contents.len(), 13);
        assert_eq!(contents, "01234567\0\0\0\0\0");
    }

    #[test]
    fn dealloc_with_suffix() {
        let env = make_test_env(vec!["truncate", "-d", "-o", "0", "-l", "5", "file.txt"]);
        env.fs.add_file("file.txt", "0123456789");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!(
            env.fs.read_to_string("file.txt").unwrap(),
            "\0\0\0\0\x0056789"
        );
    }

    #[test]
    fn dealloc_entire_file() {
        let env = make_test_env(vec!["truncate", "-d", "-l", "10", "file.txt"]);
        env.fs.add_file("file.txt", "0123456789");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!(
            env.fs.read_to_string("file.txt").unwrap(),
            "\0\0\0\0\0\0\0\0\0\0"
        );
    }

    #[test]
    fn dealloc_nonexistent_file_error() {
        let env = make_test_env(vec!["truncate", "-d", "-l", "10", "nonexistent.txt"]);
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("nonexistent.txt"));
    }

    #[test]
    fn dealloc_with_c_flag_skips_nonexistent() {
        let env = make_test_env(vec!["truncate", "-c", "-d", "-l", "10", "nonexistent.txt"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert!(!env.fs.exists("nonexistent.txt"));
    }

    #[test]
    fn dealloc_multiple_files() {
        let env = make_test_env(vec![
            "truncate", "-d", "-o", "2", "-l", "3", "a.txt", "b.txt",
        ]);
        env.fs.add_file("a.txt", "0123456789");
        env.fs.add_file("b.txt", "abcdefghij");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!(env.fs.read_to_string("a.txt").unwrap(), "01\0\0\x0056789");
        assert_eq!(env.fs.read_to_string("b.txt").unwrap(), "ab\0\0\0fghij");
    }

    #[test]
    fn dealloc_offset_with_suffix() {
        let env = make_test_env(vec!["truncate", "-d", "-o", "0", "-l", "5", "file.txt"]);
        env.fs.add_file("file.txt", "0123456789");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!(
            env.fs.read_to_string("file.txt").unwrap(),
            "\0\0\0\0\x0056789"
        );
    }

    #[test]
    fn dealloc_invalid_length() {
        let env = make_test_env(vec!["truncate", "-d", "-l", "abc", "file.txt"]);
        env.fs.add_file("file.txt", "0123456789");
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("invalid"));
    }

    #[test]
    fn dealloc_invalid_offset() {
        let env = make_test_env(vec!["truncate", "-d", "-o", "abc", "-l", "5", "file.txt"]);
        env.fs.add_file("file.txt", "0123456789");
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("invalid"));
    }
}

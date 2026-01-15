//! The touch builtin: change file access and modification times.

use getopts::Options;

use crate::{
    Environment, Error, ExitCode, Filesystem, FsError, StdioIn, StdioOut, TimeSpec, resolve_path,
};

/// Parse a time offset string of the form "[-][[hh]mm]SS" and return seconds.
fn parse_time_offset(arg: &str) -> Result<i64, String> {
    let (is_neg, digits) = if let Some(rest) = arg.strip_prefix('-') {
        (true, rest)
    } else {
        (false, arg)
    };

    if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_digit()) {
        return Err(format!(
            "Invalid offset spec, must be [-][[HH]MM]SS: {}",
            arg
        ));
    }

    let offset_secs = match digits.len() {
        2 => {
            // SS
            digits
                .parse::<i64>()
                .map_err(|_| format!("invalid offset: {}", arg))?
        }
        4 => {
            // MMSS
            let mm: i64 = digits[0..2]
                .parse()
                .map_err(|_| format!("invalid offset: {}", arg))?;
            let ss: i64 = digits[2..4]
                .parse()
                .map_err(|_| format!("invalid offset: {}", arg))?;
            mm * 60 + ss
        }
        6 => {
            // HHMMSS
            let hh: i64 = digits[0..2]
                .parse()
                .map_err(|_| format!("invalid offset: {}", arg))?;
            let mm: i64 = digits[2..4]
                .parse()
                .map_err(|_| format!("invalid offset: {}", arg))?;
            let ss: i64 = digits[4..6]
                .parse()
                .map_err(|_| format!("invalid offset: {}", arg))?;
            hh * 3600 + mm * 60 + ss
        }
        _ => {
            return Err(format!(
                "Invalid offset spec, must be [-][[HH]MM]SS: {}",
                arg
            ));
        }
    };

    Ok(if is_neg { -offset_secs } else { offset_secs })
}

/// Parse a time string of the form "[[CC]YY]MMDDhhmm[.SS]" (-t flag).
/// Returns milliseconds since UNIX epoch.
fn parse_time_arg1(arg: &str) -> Result<i64, String> {
    let now = std::time::SystemTime::now();
    let now_secs = now
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    // Get current year for defaults
    let current_year = 1970 + (now_secs / 31557600); // Approximate

    let (main_part, seconds) = if let Some(dot_pos) = arg.find('.') {
        let ss_part = &arg[dot_pos + 1..];
        if ss_part.len() != 2 || !ss_part.chars().all(|c| c.is_ascii_digit()) {
            return Err(format!(
                "out of range or illegal time specification: [[CC]YY]MMDDhhmm[.SS]: {}",
                arg
            ));
        }
        let ss: u32 = ss_part.parse().map_err(|_| {
            format!(
                "out of range or illegal time specification: [[CC]YY]MMDDhhmm[.SS]: {}",
                arg
            )
        })?;
        (&arg[..dot_pos], ss)
    } else {
        (arg, 0)
    };

    if !main_part.chars().all(|c| c.is_ascii_digit()) {
        return Err(format!(
            "out of range or illegal time specification: [[CC]YY]MMDDhhmm[.SS]: {}",
            arg
        ));
    }

    let (year, month, day, hour, minute) = match main_part.len() {
        8 => {
            // MMDDhhmm - use current year
            let mm: u32 = main_part[0..2].parse().map_err(|_| "invalid month")?;
            let dd: u32 = main_part[2..4].parse().map_err(|_| "invalid day")?;
            let hh: u32 = main_part[4..6].parse().map_err(|_| "invalid hour")?;
            let min: u32 = main_part[6..8].parse().map_err(|_| "invalid minute")?;
            (current_year as i32, mm, dd, hh, min)
        }
        10 => {
            // YYMMDDhhmm
            let yy: i32 = main_part[0..2].parse().map_err(|_| "invalid year")?;
            let year = if yy < 69 { 2000 + yy } else { 1900 + yy };
            let mm: u32 = main_part[2..4].parse().map_err(|_| "invalid month")?;
            let dd: u32 = main_part[4..6].parse().map_err(|_| "invalid day")?;
            let hh: u32 = main_part[6..8].parse().map_err(|_| "invalid hour")?;
            let min: u32 = main_part[8..10].parse().map_err(|_| "invalid minute")?;
            (year, mm, dd, hh, min)
        }
        12 => {
            // CCYYMMDDhhmm
            let year: i32 = main_part[0..4].parse().map_err(|_| "invalid year")?;
            let mm: u32 = main_part[4..6].parse().map_err(|_| "invalid month")?;
            let dd: u32 = main_part[6..8].parse().map_err(|_| "invalid day")?;
            let hh: u32 = main_part[8..10].parse().map_err(|_| "invalid hour")?;
            let min: u32 = main_part[10..12].parse().map_err(|_| "invalid minute")?;
            (year, mm, dd, hh, min)
        }
        _ => {
            return Err(format!(
                "out of range or illegal time specification: [[CC]YY]MMDDhhmm[.SS]: {}",
                arg
            ));
        }
    };

    // Validate ranges
    if !(1..=12).contains(&month) {
        return Err(format!("month out of range: {}", month));
    }
    if !(1..=31).contains(&day) {
        return Err(format!("day out of range: {}", day));
    }
    if hour > 23 {
        return Err(format!("hour out of range: {}", hour));
    }
    if minute > 59 {
        return Err(format!("minute out of range: {}", minute));
    }
    if seconds > 60 {
        return Err(format!("seconds out of range: {}", seconds));
    }

    // Convert to timestamp (simplified - doesn't handle all edge cases)
    let timestamp_ms = datetime_to_ms(year, month, day, hour, minute, seconds);
    Ok(timestamp_ms)
}

/// Parse the obsolescent time format "MMDDhhmm[YY]" (first positional arg).
/// Returns milliseconds since UNIX epoch.
fn parse_time_arg2(arg: &str, has_year: bool) -> Result<i64, String> {
    let now = std::time::SystemTime::now();
    let now_secs = now
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let current_year = 1970 + (now_secs / 31557600); // Approximate

    if !arg.chars().all(|c| c.is_ascii_digit()) {
        return Err(format!(
            "out of range or illegal time specification: MMDDhhmm[yy]: {}",
            arg
        ));
    }

    let mm: u32 = arg[0..2].parse().map_err(|_| "invalid month")?;
    let dd: u32 = arg[2..4].parse().map_err(|_| "invalid day")?;
    let hh: u32 = arg[4..6].parse().map_err(|_| "invalid hour")?;
    let min: u32 = arg[6..8].parse().map_err(|_| "invalid minute")?;

    let year = if has_year {
        let yy: i32 = arg[8..10].parse().map_err(|_| "invalid year")?;
        // Support 2000-2038, not 1902-1969
        if yy < 39 { 2000 + yy } else { 1900 + yy }
    } else {
        current_year as i32
    };

    // Validate ranges
    if !(1..=12).contains(&mm) {
        return Err(format!("month out of range: {}", mm));
    }
    if !(1..=31).contains(&dd) {
        return Err(format!("day out of range: {}", dd));
    }
    if hh > 23 {
        return Err(format!("hour out of range: {}", hh));
    }
    if min > 59 {
        return Err(format!("minute out of range: {}", min));
    }

    let timestamp_ms = datetime_to_ms(year, mm, dd, hh, min, 0);
    Ok(timestamp_ms)
}

/// Parse ISO 8601 datetime "YYYY-MM-DDThh:mm:SS[.frac][Z]" (-d flag).
/// Returns milliseconds since UNIX epoch.
fn parse_time_darg(arg: &str) -> Result<i64, String> {
    // Check for two colons (required)
    let colon_count = arg.chars().filter(|&c| c == ':').count();
    if colon_count < 2 {
        return Err(format!(
            "out of range or illegal time specification: YYYY-MM-DDThh:mm:SS[.frac][tz]: {}",
            arg
        ));
    }

    // Parse the basic datetime part
    let has_t = arg.contains('T');
    let sep = if has_t { 'T' } else { ' ' };

    let parts: Vec<&str> = arg.splitn(2, sep).collect();
    if parts.len() != 2 {
        return Err(format!(
            "out of range or illegal time specification: YYYY-MM-DDThh:mm:SS[.frac][tz]: {}",
            arg
        ));
    }

    let date_part = parts[0];
    let time_part = parts[1];

    // Parse date: YYYY-MM-DD
    let date_parts: Vec<&str> = date_part.split('-').collect();
    if date_parts.len() != 3 {
        return Err(format!("invalid date format: {}", date_part));
    }

    let year: i32 = date_parts[0].parse().map_err(|_| "invalid year")?;
    let month: u32 = date_parts[1].parse().map_err(|_| "invalid month")?;
    let day: u32 = date_parts[2].parse().map_err(|_| "invalid day")?;

    // Parse time: hh:mm:SS[.frac][Z]
    let is_utc = time_part.ends_with('Z');
    let time_str = if is_utc {
        &time_part[..time_part.len() - 1]
    } else {
        time_part
    };

    // Split off fractional seconds if present
    let (time_main, frac_ms) = if let Some(dot_pos) = time_str.find('.') {
        let frac_str = &time_str[dot_pos + 1..];
        let frac_ms = parse_fractional_seconds(frac_str)?;
        (&time_str[..dot_pos], frac_ms)
    } else if let Some(comma_pos) = time_str.find(',') {
        let frac_str = &time_str[comma_pos + 1..];
        let frac_ms = parse_fractional_seconds(frac_str)?;
        (&time_str[..comma_pos], frac_ms)
    } else {
        (time_str, 0i64)
    };

    let time_parts: Vec<&str> = time_main.split(':').collect();
    if time_parts.len() != 3 {
        return Err(format!("invalid time format: {}", time_main));
    }

    let hour: u32 = time_parts[0].parse().map_err(|_| "invalid hour")?;
    let minute: u32 = time_parts[1].parse().map_err(|_| "invalid minute")?;
    let second: u32 = time_parts[2].parse().map_err(|_| "invalid second")?;

    // Validate ranges
    if !(1..=12).contains(&month) {
        return Err(format!("month out of range: {}", month));
    }
    if !(1..=31).contains(&day) {
        return Err(format!("day out of range: {}", day));
    }
    if hour > 23 {
        return Err(format!("hour out of range: {}", hour));
    }
    if minute > 59 {
        return Err(format!("minute out of range: {}", minute));
    }
    if second > 60 {
        return Err(format!("second out of range: {}", second));
    }

    let mut timestamp_ms = datetime_to_ms(year, month, day, hour, minute, second);
    timestamp_ms += frac_ms;

    // Note: We treat all times as UTC for simplicity in the virtual filesystem.
    // A real implementation would handle timezone conversion.
    let _ = is_utc;

    Ok(timestamp_ms)
}

/// Parse fractional seconds string and return milliseconds.
fn parse_fractional_seconds(frac: &str) -> Result<i64, String> {
    if frac.is_empty() || !frac.chars().all(|c| c.is_ascii_digit()) {
        return Err("invalid fractional seconds".to_string());
    }

    // Pad or truncate to 3 digits for milliseconds
    let padded = if frac.len() >= 3 {
        &frac[..3]
    } else {
        // Pad with zeros
        return Ok(frac.parse::<i64>().unwrap_or(0) * 10i64.pow(3 - frac.len() as u32));
    };

    padded
        .parse()
        .map_err(|_| "invalid fractional seconds".to_string())
}

/// Convert datetime components to milliseconds since UNIX epoch.
/// This is a simplified implementation that doesn't handle all edge cases.
fn datetime_to_ms(year: i32, month: u32, day: u32, hour: u32, minute: u32, second: u32) -> i64 {
    // Days in each month (non-leap year)
    let days_in_month = [0, 31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];

    let is_leap = |y: i32| -> bool { (y % 4 == 0 && y % 100 != 0) || (y % 400 == 0) };

    // Calculate days since epoch (1970-01-01)
    let mut days: i64 = 0;

    // Add days for complete years
    for y in 1970..year {
        days += if is_leap(y) { 366 } else { 365 };
    }
    for y in (year..1970).rev() {
        days -= if is_leap(y) { 366 } else { 365 };
    }

    // Add days for complete months in current year
    for m in 1..month {
        days += days_in_month[m as usize] as i64;
        if m == 2 && is_leap(year) {
            days += 1;
        }
    }

    // Add days in current month
    days += (day - 1) as i64;

    // Convert to milliseconds
    let secs = days * 86400 + (hour as i64) * 3600 + (minute as i64) * 60 + (second as i64);
    secs * 1000
}

fn build_options() -> Options {
    let mut opts = Options::new();
    opts.optopt("A", "", "Adjust times by [-][[hh]mm]SS", "[-][[hh]mm]SS");
    opts.optflag("a", "", "Change access time only");
    opts.optflag("c", "", "Do not create file if it does not exist");
    opts.optopt(
        "d",
        "",
        "Set time to YYYY-MM-DDThh:mm:SS[.frac][Z]",
        "datetime",
    );
    opts.optflag("f", "", "Ignored for compatibility");
    opts.optflag("h", "", "Change times of symlink itself (implies -c)");
    opts.optflag("m", "", "Change modification time only");
    opts.optopt("r", "", "Use times from reference file", "file");
    opts.optopt("t", "", "Set time to [[CC]YY]MMDDhhmm[.SS]", "time");
    opts
}

/// The touch builtin: change file access and modification times.
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
            env.stderr.write_line(&format!("touch: {}", e))?;
            return Ok(ExitCode::from(1));
        }
    };

    let adjust_offset = matches.opt_str("A");
    let mut aflag = matches.opt_present("a");
    let mut cflag = matches.opt_present("c");
    let date_arg = matches.opt_str("d");
    let hflag = matches.opt_present("h");
    let mut mflag = matches.opt_present("m");
    let ref_file = matches.opt_str("r");
    let time_arg = matches.opt_str("t");

    // -h implies -c
    if hflag {
        cflag = true;
    }

    // If neither -a nor -m, do both
    if !aflag && !mflag {
        aflag = true;
        mflag = true;
    }

    // Parse the time offset if -A was given
    let offset_secs = if let Some(ref offset_str) = adjust_offset {
        match parse_time_offset(offset_str) {
            Ok(secs) => {
                // -A implies -c
                cflag = true;
                Some(secs)
            }
            Err(e) => {
                env.stderr.write_line(&format!("touch: {}", e))?;
                return Ok(ExitCode::from(1));
            }
        }
    } else {
        None
    };

    // Determine the base time to set
    let mut timeset = false;
    let mut base_time_ms: Option<i64> = None;

    // -d takes precedence
    if let Some(ref darg) = date_arg {
        match parse_time_darg(darg) {
            Ok(ms) => {
                timeset = true;
                base_time_ms = Some(ms);
            }
            Err(e) => {
                env.stderr.write_line(&format!("touch: {}", e))?;
                return Ok(ExitCode::from(1));
            }
        }
    }

    // -t overrides -d (last one wins in BSD touch, but we follow the order in code)
    if let Some(ref targ) = time_arg {
        match parse_time_arg1(targ) {
            Ok(ms) => {
                timeset = true;
                base_time_ms = Some(ms);
            }
            Err(e) => {
                env.stderr.write_line(&format!("touch: {}", e))?;
                return Ok(ExitCode::from(1));
            }
        }
    }

    // -r uses reference file times
    let ref_times: Option<(i64, i64)> = if let Some(ref rfile) = ref_file {
        let resolved_rfile = resolve_path(env.cwd.as_str(), rfile);
        match env.fs.stat(&resolved_rfile) {
            Ok(entry) => {
                timeset = true;
                Some((entry.atime_ms, entry.mtime_ms))
            }
            Err(FsError::Io(e)) => {
                env.stderr.write_line(&format!("touch: {}: {}", rfile, e))?;
                return Ok(ExitCode::from(1));
            }
        }
    } else {
        None
    };

    let mut args = matches.free.clone();

    // Handle obsolescent form: if no -r or -t, at least 2 args, and first arg is 8 or 10 digits
    if !timeset && args.len() > 1 {
        let first = &args[0];
        if first.chars().all(|c| c.is_ascii_digit()) && (first.len() == 8 || first.len() == 10) {
            let has_year = first.len() == 10;
            match parse_time_arg2(first, has_year) {
                Ok(ms) => {
                    timeset = true;
                    base_time_ms = Some(ms);
                    args.remove(0);
                }
                Err(_) => {
                    // Not a valid time, treat as filename
                }
            }
        }
    }

    if args.is_empty() {
        env.stderr.write_line(
            "usage: touch [-A [-][[hh]mm]SS] [-achm] [-r file] [-t [[CC]YY]MMDDhhmm[.SS]] [-d YYYY-MM-DDThh:mm:SS[.frac][tz]] file ...",
        )?;
        return Ok(ExitCode::from(1));
    }

    let mut exit_code: i8 = 0;

    for file in &args {
        let file = resolve_path(env.cwd.as_str(), file);
        // Check if file exists
        let file_exists = env.fs.exists(&file);
        let is_symlink = if file_exists {
            env.fs
                .lstat(&file)
                .map(|e| e.file_type == crate::FileType::Symlink)
                .unwrap_or(false)
        } else {
            false
        };

        if !file_exists {
            if cflag {
                // -c: don't create, silently skip
                continue;
            }
            // Create the file
            match env.fs.create_file(&file) {
                Ok(_) => {
                    // If not setting a specific time, we're done (file created with current time)
                    if !timeset && offset_secs.is_none() {
                        continue;
                    }
                }
                Err(FsError::Io(e)) => {
                    env.stderr.write_line(&format!("touch: {}: {}", file, e))?;
                    exit_code = 1;
                    continue;
                }
            }
        }

        // Determine the times to set
        let (mut atime_ms, mut mtime_ms) = if let Some((a, m)) = ref_times {
            (a, m)
        } else if let Some(t) = base_time_ms {
            (t, t)
        } else {
            // Use current time
            let now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0);
            (now_ms, now_ms)
        };

        // Apply offset if -A was specified
        if let Some(offset) = offset_secs {
            if timeset {
                // Offset from specified time
                if aflag {
                    atime_ms += offset * 1000;
                }
                if mflag {
                    mtime_ms += offset * 1000;
                }
            } else {
                // Offset from file's current times
                let current = if hflag {
                    env.fs.lstat(&file)
                } else {
                    env.fs.stat(&file)
                };
                match current {
                    Ok(entry) => {
                        if aflag {
                            atime_ms = entry.atime_ms + offset * 1000;
                        }
                        if mflag {
                            mtime_ms = entry.mtime_ms + offset * 1000;
                        }
                    }
                    Err(FsError::Io(e)) => {
                        env.stderr.write_line(&format!("touch: {}: {}", file, e))?;
                        exit_code = 1;
                        continue;
                    }
                }
            }
        }

        // Build TimeSpec for each time
        let atime_spec = if aflag {
            TimeSpec::Time(atime_ms)
        } else {
            TimeSpec::Omit
        };
        let mtime_spec = if mflag {
            TimeSpec::Time(mtime_ms)
        } else {
            TimeSpec::Omit
        };

        // Set the times
        let result = if hflag && is_symlink {
            env.fs.lset_times(&file, atime_spec, mtime_spec)
        } else {
            env.fs.set_times(&file, atime_spec, mtime_spec)
        };

        if let Err(FsError::Io(e)) = result {
            env.stderr.write_line(&format!("touch: {}: {}", file, e))?;
            exit_code = 1;
        }
    }

    Ok(ExitCode::from(exit_code))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TestFilesystemExt;
    use crate::test_utils::make_test_env;

    // ========================================================================
    // parse_time_offset tests
    // ========================================================================

    #[test]
    fn parse_offset_seconds_only() {
        assert_eq!(parse_time_offset("30").unwrap(), 30);
        assert_eq!(parse_time_offset("00").unwrap(), 0);
        assert_eq!(parse_time_offset("59").unwrap(), 59);
    }

    #[test]
    fn parse_offset_minutes_seconds() {
        assert_eq!(parse_time_offset("0130").unwrap(), 90); // 1 min 30 sec
        assert_eq!(parse_time_offset("1000").unwrap(), 600); // 10 min
    }

    #[test]
    fn parse_offset_hours_minutes_seconds() {
        assert_eq!(parse_time_offset("010000").unwrap(), 3600); // 1 hour
        assert_eq!(parse_time_offset("013030").unwrap(), 5430); // 1h 30m 30s
    }

    #[test]
    fn parse_offset_negative() {
        assert_eq!(parse_time_offset("-30").unwrap(), -30);
        assert_eq!(parse_time_offset("-0130").unwrap(), -90);
        assert_eq!(parse_time_offset("-010000").unwrap(), -3600);
    }

    #[test]
    fn parse_offset_invalid() {
        assert!(parse_time_offset("").is_err());
        assert!(parse_time_offset("abc").is_err());
        assert!(parse_time_offset("123").is_err()); // 3 digits invalid
        assert!(parse_time_offset("12345").is_err()); // 5 digits invalid
    }

    // ========================================================================
    // parse_time_arg1 tests (-t flag)
    // ========================================================================

    #[test]
    fn parse_time_arg1_ccyymmddmm() {
        // 2024-06-15 14:30:00
        let ms = parse_time_arg1("202406151430").unwrap();
        // Verify it's roughly correct (within the right year)
        let secs = ms / 1000;
        assert!(secs > 1718000000 && secs < 1720000000);
        println!("202406151430 -> {} ms, {} secs", ms, secs);
    }

    #[test]
    fn parse_time_arg1_with_seconds() {
        let ms = parse_time_arg1("202406151430.45").unwrap();
        let secs = ms / 1000;
        println!("202406151430.45 -> {} ms, {} secs", ms, secs);
        // Should be 45 seconds more than without .SS
        let ms_no_sec = parse_time_arg1("202406151430").unwrap();
        assert_eq!(ms - ms_no_sec, 45000);
    }

    #[test]
    fn parse_time_arg1_yy_format() {
        // YY < 69 -> 20YY
        let ms = parse_time_arg1("2406151430").unwrap();
        let secs = ms / 1000;
        println!("2406151430 -> {} ms, {} secs", ms, secs);
        assert!(secs > 1718000000); // Should be year 2024

        // YY >= 69 -> 19YY
        let ms_old = parse_time_arg1("9912310000").unwrap();
        let secs_old = ms_old / 1000;
        println!("9912310000 -> {} ms, {} secs", ms_old, secs_old);
        assert!(secs_old < 1000000000); // Should be year 1999
    }

    #[test]
    fn parse_time_arg1_invalid() {
        assert!(parse_time_arg1("").is_err());
        assert!(parse_time_arg1("abc").is_err());
        assert!(parse_time_arg1("1234567").is_err()); // Wrong length
        assert!(parse_time_arg1("202413151430").is_err()); // Month 13
        assert!(parse_time_arg1("202406321430").is_err()); // Day 32
    }

    // ========================================================================
    // parse_time_darg tests (-d flag)
    // ========================================================================

    #[test]
    fn parse_time_darg_basic() {
        let ms = parse_time_darg("2024-06-15T14:30:00").unwrap();
        let secs = ms / 1000;
        println!("2024-06-15T14:30:00 -> {} ms, {} secs", ms, secs);
        assert!(secs > 1718000000 && secs < 1720000000);
    }

    #[test]
    fn parse_time_darg_with_space() {
        let ms = parse_time_darg("2024-06-15 14:30:00").unwrap();
        let secs = ms / 1000;
        println!("2024-06-15 14:30:00 -> {} ms, {} secs", ms, secs);
        assert!(secs > 1718000000 && secs < 1720000000);
    }

    #[test]
    fn parse_time_darg_with_frac() {
        let ms = parse_time_darg("2024-06-15T14:30:00.123").unwrap();
        let ms_no_frac = parse_time_darg("2024-06-15T14:30:00").unwrap();
        assert_eq!(ms - ms_no_frac, 123);
    }

    #[test]
    fn parse_time_darg_with_z() {
        let ms = parse_time_darg("2024-06-15T14:30:00Z").unwrap();
        let secs = ms / 1000;
        println!("2024-06-15T14:30:00Z -> {} ms, {} secs", ms, secs);
        assert!(secs > 1718000000);
    }

    #[test]
    fn parse_time_darg_invalid() {
        assert!(parse_time_darg("").is_err());
        assert!(parse_time_darg("2024-06-15").is_err()); // No time
        assert!(parse_time_darg("14:30:00").is_err()); // No date
        assert!(parse_time_darg("2024-06-15T14:30").is_err()); // Only one colon
    }

    // ========================================================================
    // datetime_to_ms tests
    // ========================================================================

    #[test]
    fn datetime_to_ms_epoch() {
        assert_eq!(datetime_to_ms(1970, 1, 1, 0, 0, 0), 0);
    }

    #[test]
    fn datetime_to_ms_known_date() {
        // 2000-01-01 00:00:00 UTC = 946684800 seconds
        let ms = datetime_to_ms(2000, 1, 1, 0, 0, 0);
        assert_eq!(ms / 1000, 946684800);
    }

    // ========================================================================
    // touch builtin tests - basic usage
    // ========================================================================

    #[test]
    fn no_files_shows_usage() {
        let env = make_test_env(vec!["touch"]);
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("usage:"));
    }

    #[test]
    fn creates_new_file() {
        let env = make_test_env(vec!["touch", "newfile.txt"]);
        assert!(!env.fs.exists("/newfile.txt"));
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert!(env.fs.exists("/newfile.txt"));
    }

    #[test]
    fn creates_multiple_files() {
        let env = make_test_env(vec!["touch", "a.txt", "b.txt", "c.txt"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert!(env.fs.exists("/a.txt"));
        assert!(env.fs.exists("/b.txt"));
        assert!(env.fs.exists("/c.txt"));
    }

    #[test]
    fn c_flag_does_not_create() {
        let env = make_test_env(vec!["touch", "-c", "nonexistent.txt"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert!(!env.fs.exists("/nonexistent.txt"));
    }

    #[test]
    fn c_flag_updates_existing() {
        let env = make_test_env(vec!["touch", "-c", "existing.txt"]);
        env.fs
            .add_file_with_times("/existing.txt", "content", 1000, 2000);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stat = env.fs.stat("/existing.txt").unwrap();
        // Times should be updated to "now" (which in tests will be some recent time)
        assert!(stat.atime_ms > 2000);
        assert!(stat.mtime_ms > 2000);
    }

    // ========================================================================
    // touch -a and -m flag tests
    // ========================================================================

    #[test]
    fn a_flag_only_changes_atime() {
        let env = make_test_env(vec!["touch", "-a", "file.txt"]);
        env.fs
            .add_file_with_times("/file.txt", "content", 1000, 2000);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stat = env.fs.stat("/file.txt").unwrap();
        println!("atime_ms: {}, mtime_ms: {}", stat.atime_ms, stat.mtime_ms);
        assert!(stat.atime_ms > 1000); // atime updated
        assert_eq!(stat.mtime_ms, 2000); // mtime unchanged
    }

    #[test]
    fn m_flag_only_changes_mtime() {
        let env = make_test_env(vec!["touch", "-m", "file.txt"]);
        env.fs
            .add_file_with_times("/file.txt", "content", 1000, 2000);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stat = env.fs.stat("/file.txt").unwrap();
        println!("atime_ms: {}, mtime_ms: {}", stat.atime_ms, stat.mtime_ms);
        assert_eq!(stat.atime_ms, 1000); // atime unchanged
        assert!(stat.mtime_ms > 2000); // mtime updated
    }

    #[test]
    fn am_flags_change_both() {
        let env = make_test_env(vec!["touch", "-a", "-m", "file.txt"]);
        env.fs
            .add_file_with_times("/file.txt", "content", 1000, 2000);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stat = env.fs.stat("/file.txt").unwrap();
        assert!(stat.atime_ms > 1000);
        assert!(stat.mtime_ms > 2000);
    }

    // ========================================================================
    // touch -r flag tests
    // ========================================================================

    #[test]
    fn r_flag_copies_times_from_reference() {
        let env = make_test_env(vec!["touch", "-r", "ref.txt", "target.txt"]);
        env.fs
            .add_file_with_times("/ref.txt", "reference", 5000000, 6000000);
        env.fs
            .add_file_with_times("/target.txt", "target", 1000, 2000);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stat = env.fs.stat("/target.txt").unwrap();
        assert_eq!(stat.atime_ms, 5000000);
        assert_eq!(stat.mtime_ms, 6000000);
    }

    #[test]
    fn r_flag_creates_file_with_ref_times() {
        let env = make_test_env(vec!["touch", "-r", "ref.txt", "newfile.txt"]);
        env.fs
            .add_file_with_times("/ref.txt", "reference", 5000000, 6000000);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert!(env.fs.exists("/newfile.txt"));
        let stat = env.fs.stat("/newfile.txt").unwrap();
        assert_eq!(stat.atime_ms, 5000000);
        assert_eq!(stat.mtime_ms, 6000000);
    }

    #[test]
    fn r_flag_nonexistent_ref_fails() {
        let env = make_test_env(vec!["touch", "-r", "nonexistent.txt", "target.txt"]);
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("nonexistent.txt"));
    }

    // ========================================================================
    // touch -t flag tests
    // ========================================================================

    #[test]
    fn t_flag_sets_specific_time() {
        let env = make_test_env(vec!["touch", "-t", "202001011200", "file.txt"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stat = env.fs.stat("/file.txt").unwrap();
        // 2020-01-01 12:00:00 UTC
        let expected_ms = datetime_to_ms(2020, 1, 1, 12, 0, 0);
        assert_eq!(stat.atime_ms, expected_ms);
        assert_eq!(stat.mtime_ms, expected_ms);
    }

    #[test]
    fn t_flag_with_seconds() {
        let env = make_test_env(vec!["touch", "-t", "202001011200.30", "file.txt"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stat = env.fs.stat("/file.txt").unwrap();
        let expected_ms = datetime_to_ms(2020, 1, 1, 12, 0, 30);
        assert_eq!(stat.atime_ms, expected_ms);
        assert_eq!(stat.mtime_ms, expected_ms);
    }

    // ========================================================================
    // touch -d flag tests
    // ========================================================================

    #[test]
    fn d_flag_sets_iso_time() {
        let env = make_test_env(vec!["touch", "-d", "2020-01-01T12:00:00", "file.txt"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stat = env.fs.stat("/file.txt").unwrap();
        let expected_ms = datetime_to_ms(2020, 1, 1, 12, 0, 0);
        assert_eq!(stat.atime_ms, expected_ms);
        assert_eq!(stat.mtime_ms, expected_ms);
    }

    #[test]
    fn d_flag_with_fractional_seconds() {
        let env = make_test_env(vec!["touch", "-d", "2020-01-01T12:00:00.500", "file.txt"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stat = env.fs.stat("/file.txt").unwrap();
        let expected_ms = datetime_to_ms(2020, 1, 1, 12, 0, 0) + 500;
        assert_eq!(stat.atime_ms, expected_ms);
        assert_eq!(stat.mtime_ms, expected_ms);
    }

    // ========================================================================
    // touch -A flag tests
    // ========================================================================

    #[test]
    fn a_offset_adjusts_times() {
        let env = make_test_env(vec!["touch", "-A", "0100", "file.txt"]);
        env.fs
            .add_file_with_times("/file.txt", "content", 1000000, 2000000);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stat = env.fs.stat("/file.txt").unwrap();
        // 1 minute = 60 seconds = 60000 ms
        assert_eq!(stat.atime_ms, 1000000 + 60000);
        assert_eq!(stat.mtime_ms, 2000000 + 60000);
    }

    #[test]
    fn a_offset_negative() {
        let env = make_test_env(vec!["touch", "-A", "-0100", "file.txt"]);
        env.fs
            .add_file_with_times("/file.txt", "content", 1000000, 2000000);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stat = env.fs.stat("/file.txt").unwrap();
        assert_eq!(stat.atime_ms, 1000000 - 60000);
        assert_eq!(stat.mtime_ms, 2000000 - 60000);
    }

    #[test]
    fn a_offset_implies_c() {
        let env = make_test_env(vec!["touch", "-A", "0100", "nonexistent.txt"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert!(!env.fs.exists("/nonexistent.txt")); // -A implies -c
    }

    // ========================================================================
    // touch -h flag tests (symlinks)
    // ========================================================================

    #[test]
    fn h_flag_implies_c() {
        let env = make_test_env(vec!["touch", "-h", "nonexistent.txt"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert!(!env.fs.exists("/nonexistent.txt"));
    }

    #[test]
    fn h_flag_changes_symlink_times() {
        let env = make_test_env(vec!["touch", "-h", "-t", "202001011200", "link"]);
        env.fs.add_file("/target.txt", "content");
        env.fs.add_symlink("/link", "/target.txt");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let link_stat = env.fs.lstat("/link").unwrap();
        let expected_ms = datetime_to_ms(2020, 1, 1, 12, 0, 0);
        assert_eq!(link_stat.atime_ms, expected_ms);
        assert_eq!(link_stat.mtime_ms, expected_ms);
    }

    // ========================================================================
    // obsolescent form tests
    // ========================================================================

    #[test]
    fn obsolescent_8_digit_time() {
        // MMDDhhmm format with current year
        let env = make_test_env(vec!["touch", "01011200", "file.txt"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert!(env.fs.exists("/file.txt"));
        // The first arg is interpreted as time, so file.txt should be created
        let stat = env.fs.stat("/file.txt").unwrap();
        println!("stat: {:?}", stat);
        // Should be Jan 1, 12:00 of some year
    }

    #[test]
    fn obsolescent_10_digit_time() {
        // MMDDhhmmYY format
        let env = make_test_env(vec!["touch", "0101120024", "file.txt"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert!(env.fs.exists("/file.txt"));
        let stat = env.fs.stat("/file.txt").unwrap();
        println!("stat: {:?}", stat);
        // Should be Jan 1, 2024 12:00
    }

    // ========================================================================
    // error handling tests
    // ========================================================================

    #[test]
    fn invalid_t_arg_shows_error() {
        let env = make_test_env(vec!["touch", "-t", "invalid", "file.txt"]);
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("out of range or illegal"));
    }

    #[test]
    fn invalid_d_arg_shows_error() {
        let env = make_test_env(vec!["touch", "-d", "invalid", "file.txt"]);
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("out of range or illegal"));
    }

    #[test]
    fn invalid_a_offset_shows_error() {
        let env = make_test_env(vec!["touch", "-A", "invalid", "file.txt"]);
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("Invalid offset"));
    }

    // ========================================================================
    // path resolution tests
    // ========================================================================

    #[test]
    fn touch_relative_path_creates_in_cwd() {
        use crate::test_utils::TestEnvBuilder;
        let env = TestEnvBuilder::new()
            .args(vec!["touch", "newfile.txt"])
            .cwd("/home")
            .build();
        env.fs.add_directory("/");
        env.fs.add_directory("/home");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        // The file should be created at /home/newfile.txt, not at newfile.txt
        assert!(
            env.fs.exists("/home/newfile.txt"),
            "Expected file at /home/newfile.txt"
        );
        println!(
            "exists /home/newfile.txt: {}",
            env.fs.exists("/home/newfile.txt")
        );
        println!("exists newfile.txt: {}", env.fs.exists("newfile.txt"));
    }
}

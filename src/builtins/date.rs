use getopts::Options;

use chrono::DateTime;
use chrono::Datelike;
use chrono::NaiveDate;
use chrono::NaiveDateTime;
use chrono::NaiveTime;
use chrono::Timelike;
use chrono::Utc;

use crate::Environment;
use crate::Error;
use crate::ExitCode;
use crate::Filesystem;
use crate::Stderr;
use crate::Stdin;
use crate::Stdout;

/// ISO 8601 format precision levels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Iso8601Format {
    Date,
    Hours,
    Minutes,
    Seconds,
    Nanoseconds,
}

impl Iso8601Format {
    fn from_str(s: &str) -> Option<Self> {
        match s {
            "date" => Some(Self::Date),
            "hours" => Some(Self::Hours),
            "minutes" => Some(Self::Minutes),
            "seconds" => Some(Self::Seconds),
            "ns" => Some(Self::Nanoseconds),
            _ => None,
        }
    }
}

/// Options for the date command.
struct DateOptions {
    /// -u: Use UTC timezone.
    utc: bool,
    /// -R: Use RFC 2822 format.
    rfc2822: bool,
    /// -I: Use ISO 8601 format with specified precision.
    iso8601: Option<Iso8601Format>,
    /// -r: Reference time (either seconds since epoch or from file mtime).
    reference_time: Option<ReferenceTime>,
    /// -j: Do not set the date (parse only mode).
    no_set: bool,
    /// -f: Input format string for parsing.
    input_format: Option<String>,
    /// -v: Date adjustments to apply.
    adjustments: Vec<String>,
    /// -z: Output timezone.
    output_zone: Option<String>,
    /// Output format string (from +format argument).
    output_format: Option<String>,
    /// Date string to parse (when using -f or default format).
    date_string: Option<String>,
}

/// Reference time source for -r flag.
enum ReferenceTime {
    Seconds(i64),
    Nanoseconds(i64, u32),
}

fn build_options() -> Options {
    let mut opts = Options::new();
    opts.optopt(
        "f",
        "",
        "Use input_fmt as the format string to parse new_date",
        "input_fmt",
    );
    opts.optflagopt(
        "I",
        "",
        "Use ISO 8601 output format (date, hours, minutes, seconds, ns)",
        "FMT",
    );
    opts.optflag("j", "", "Do not try to set the date");
    opts.optflag("n", "", "Obsolete flag, accepted and ignored");
    opts.optflag("R", "", "Use RFC 2822 date and time output format");
    opts.optopt(
        "r",
        "",
        "Print date from seconds since epoch or file modification time",
        "seconds|filename",
    );
    opts.optflag("u", "", "Display or set the date in UTC");
    opts.optmulti(
        "v",
        "",
        "Adjust the date (e.g., +1d, -2w, 12m)",
        "[+|-]val[ymwdHMS]",
    );
    opts.optopt("z", "", "Output timezone", "output_zone");
    opts
}

/// The date builtin: display date and time.
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
            env.stderr.write_line(&format!("date: {}", e))?;
            return Ok(ExitCode::from(1));
        }
    };

    // Parse -I flag
    let iso8601 = if matches.opt_present("I") {
        let fmt_str = matches.opt_str("I");
        match fmt_str.as_deref() {
            None | Some("") => Some(Iso8601Format::Date),
            Some(s) => match Iso8601Format::from_str(s) {
                Some(fmt) => Some(fmt),
                None => {
                    env.stderr
                        .write_line(&format!("date: invalid argument '{}' for -I", s))?;
                    return Ok(ExitCode::from(1));
                }
            },
        }
    } else {
        None
    };

    let rfc2822 = matches.opt_present("R");

    // Check for multiple output formats
    if iso8601.is_some() && rfc2822 {
        env.stderr
            .write_line("date: multiple output formats specified")?;
        return Ok(ExitCode::from(1));
    }

    // Parse -r flag (seconds or filename)
    let reference_time = match matches.opt_str("r") {
        Some(arg) => {
            if let Ok(secs) = arg.parse::<i64>() {
                Some(ReferenceTime::Seconds(secs))
            } else {
                // Try to get file modification time
                match env.fs.stat(&arg) {
                    Ok(stat) => {
                        let secs = stat.mtime_ms / 1000;
                        let nsecs = ((stat.mtime_ms % 1000) * 1_000_000) as u32;
                        Some(ReferenceTime::Nanoseconds(secs, nsecs))
                    }
                    Err(_) => {
                        env.stderr
                            .write_line(&format!("date: {}: No such file or directory", arg))?;
                        return Ok(ExitCode::from(1));
                    }
                }
            }
        }
        None => None,
    };

    // Collect free arguments and find output format
    let mut output_format = None;
    let mut date_string = None;
    let mut found_format = false;

    for arg in &matches.free {
        if let Some(fmt) = arg.strip_prefix('+') {
            if found_format {
                env.stderr
                    .write_line("date: multiple output formats specified")?;
                return Ok(ExitCode::from(1));
            }
            if iso8601.is_some() {
                env.stderr
                    .write_line("date: multiple output formats specified")?;
                return Ok(ExitCode::from(1));
            }
            output_format = Some(fmt.to_string());
            found_format = true;
        } else {
            date_string = Some(arg.clone());
        }
    }

    let date_opts = DateOptions {
        utc: matches.opt_present("u"),
        rfc2822,
        iso8601,
        reference_time,
        no_set: matches.opt_present("j"),
        input_format: matches.opt_str("f"),
        adjustments: matches.opt_strs("v"),
        output_zone: matches.opt_str("z"),
        output_format,
        date_string,
    };

    // Setting date is not supported in this virtual environment
    if date_opts.date_string.is_some() && !date_opts.no_set && date_opts.input_format.is_none() {
        env.stderr
            .write_line("date: setting the date is not supported")?;
        return Ok(ExitCode::from(1));
    }

    // Output timezone is not supported in this virtual environment
    if date_opts.output_zone.is_some() {
        env.stderr
            .write_line("date: -z output timezone is not supported")?;
        return Ok(ExitCode::from(1));
    }

    // Get the base timestamp
    let (secs, nsecs) = match &date_opts.reference_time {
        Some(ReferenceTime::Seconds(s)) => (*s, 0u32),
        Some(ReferenceTime::Nanoseconds(s, ns)) => (*s, *ns),
        None => {
            let now = Utc::now();
            (now.timestamp(), now.timestamp_subsec_nanos())
        }
    };

    // Parse input date if -f is specified
    let (secs, nsecs) = if let Some(ref input_fmt) = date_opts.input_format {
        if let Some(ref date_str) = date_opts.date_string {
            match parse_date(date_str, input_fmt, secs, date_opts.utc) {
                Ok((s, ns)) => (s, ns),
                Err(msg) => {
                    env.stderr.write_line(&format!(
                        "date: Failed conversion of ``{}'' using format ``{}''",
                        date_str, input_fmt
                    ))?;
                    env.stderr.write_line(&format!("date: {}", msg))?;
                    return Ok(ExitCode::from(1));
                }
            }
        } else {
            env.stderr
                .write_line("date: -f requires a date string argument")?;
            return Ok(ExitCode::from(1));
        }
    } else {
        (secs, nsecs)
    };

    // Apply adjustments
    let (secs, nsecs) = if !date_opts.adjustments.is_empty() {
        match apply_adjustments(secs, nsecs, &date_opts.adjustments, date_opts.utc) {
            Ok((s, ns)) => (s, ns),
            Err(msg) => {
                env.stderr.write_line(&format!("date: {}", msg))?;
                return Ok(ExitCode::from(1));
            }
        }
    } else {
        (secs, nsecs)
    };

    // Format and output the date
    let output = format_date(secs, nsecs, &date_opts);

    env.stdout.write_line(&output)?;
    Ok(ExitCode::from(0))
}

/// Parse a date string using the given format.
fn parse_date(
    date_str: &str,
    format: &str,
    base_secs: i64,
    utc: bool,
) -> Result<(i64, u32), String> {
    // Convert strptime format to chrono format
    let chrono_fmt = strptime_to_chrono(format);

    // Try to parse with full datetime
    if let Ok(dt) = NaiveDateTime::parse_from_str(date_str, &chrono_fmt) {
        // Use UTC in this virtual environment regardless of utc flag
        let _ = utc;
        return Ok((
            dt.and_utc().timestamp(),
            dt.and_utc().timestamp_subsec_nanos(),
        ));
    }

    // Try to parse as date only, using base time for time components
    if let Ok(date) = NaiveDate::parse_from_str(date_str, &chrono_fmt) {
        let base_dt = DateTime::from_timestamp(base_secs, 0).unwrap_or_else(Utc::now);
        let time = NaiveTime::from_hms_opt(base_dt.hour(), base_dt.minute(), base_dt.second())
            .unwrap_or_default();
        let dt = date.and_time(time);
        return Ok((dt.and_utc().timestamp(), 0));
    }

    // Try to parse as time only, using base date for date components
    if let Ok(time) = NaiveTime::parse_from_str(date_str, &chrono_fmt) {
        let base_dt = DateTime::from_timestamp(base_secs, 0).unwrap_or_else(Utc::now);
        let date = NaiveDate::from_ymd_opt(base_dt.year(), base_dt.month(), base_dt.day())
            .unwrap_or_default();
        let dt = date.and_time(time);
        return Ok((dt.and_utc().timestamp(), 0));
    }

    Err("failed to parse date".to_string())
}

/// Convert strptime format specifiers to chrono format specifiers.
fn strptime_to_chrono(format: &str) -> String {
    // Many format specifiers are the same between strptime and chrono
    // but some need conversion
    let mut result = String::with_capacity(format.len());
    let mut chars = format.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '%' {
            if let Some(&next) = chars.peek() {
                match next {
                    // These are the same in both
                    'Y' | 'y' | 'm' | 'd' | 'H' | 'M' | 'S' | 'j' | 'U' | 'W' | 'w' | 'a' | 'A'
                    | 'b' | 'B' | 'p' | 'P' | 'Z' | 'z' | '%' | 'C' | 'e' | 'G' | 'g' | 'u'
                    | 'V' | 'F' | 'T' | 'R' | 'r' | 'D' | 'n' | 't' => {
                        result.push('%');
                        result.push(next);
                        chars.next();
                    }
                    // %I is 12-hour format, same in chrono
                    'I' => {
                        result.push('%');
                        result.push('I');
                        chars.next();
                    }
                    // %s is seconds since epoch
                    's' => {
                        result.push('%');
                        result.push('s');
                        chars.next();
                    }
                    // %c is locale's date and time - use a reasonable default
                    'c' => {
                        result.push_str("%a %b %e %H:%M:%S %Y");
                        chars.next();
                    }
                    // %x is locale's date - use a reasonable default
                    'x' => {
                        result.push_str("%m/%d/%y");
                        chars.next();
                    }
                    // %X is locale's time - use a reasonable default
                    'X' => {
                        result.push_str("%H:%M:%S");
                        chars.next();
                    }
                    // %+ is date(1) default format
                    '+' => {
                        result.push_str("%a %b %e %H:%M:%S %Z %Y");
                        chars.next();
                    }
                    _ => {
                        result.push('%');
                        result.push(next);
                        chars.next();
                    }
                }
            } else {
                result.push('%');
            }
        } else {
            result.push(c);
        }
    }

    result
}

/// Apply date adjustments from -v flags.
fn apply_adjustments(
    mut secs: i64,
    nsecs: u32,
    adjustments: &[String],
    utc: bool,
) -> Result<(i64, u32), String> {
    for adj in adjustments {
        let (sign, rest) = if let Some(stripped) = adj.strip_prefix('+') {
            (1i64, stripped)
        } else if let Some(stripped) = adj.strip_prefix('-') {
            (-1i64, stripped)
        } else {
            (0i64, adj.as_str()) // absolute set
        };

        if rest.is_empty() {
            return Err(format!("Cannot apply date adjustment: {}", adj));
        }

        let unit = rest.chars().last().unwrap();
        let value_str = &rest[..rest.len() - 1];

        // Check if it's a named day or month
        let (value, is_name) = if value_str.is_empty() && sign != 0 {
            // Just the unit, like "+d" means +1d
            (1i64, false)
        } else if let Ok(v) = value_str.parse::<i64>() {
            (v, false)
        } else {
            // Try to parse as named day or month
            match parse_named_time_unit(value_str, unit) {
                Some(v) => (v, true),
                None => return Err(format!("Cannot apply date adjustment: {}", adj)),
            }
        };

        let dt = DateTime::from_timestamp(secs, nsecs)
            .ok_or_else(|| format!("Invalid timestamp: {}", secs))?;

        let new_dt = match unit {
            'y' => {
                if sign == 0 {
                    // Absolute year
                    let year = if value < 69 {
                        2000 + value as i32
                    } else if value < 100 {
                        1900 + value as i32
                    } else if value > 1900 {
                        value as i32
                    } else {
                        1900 + value as i32
                    };
                    adjust_year_absolute(dt, year, utc)?
                } else {
                    adjust_year(dt, sign * value, utc)?
                }
            }
            'm' => {
                if sign == 0 {
                    adjust_month_absolute(dt, value as u32, utc)?
                } else if is_name {
                    adjust_to_month_name(dt, value as u32, sign, utc)?
                } else {
                    adjust_month(dt, sign * value, utc)?
                }
            }
            'w' => adjust_week(dt, sign * value, utc)?,
            'd' => {
                if sign == 0 {
                    adjust_day_absolute(dt, value as u32, utc)?
                } else if is_name {
                    adjust_to_weekday_name(dt, value as u32, sign, utc)?
                } else {
                    adjust_day(dt, sign * value, utc)?
                }
            }
            'H' => {
                if sign == 0 {
                    adjust_hour_absolute(dt, value as u32, utc)?
                } else {
                    adjust_hour(dt, sign * value, utc)?
                }
            }
            'M' => {
                if sign == 0 {
                    adjust_minute_absolute(dt, value as u32, utc)?
                } else {
                    adjust_minute(dt, sign * value, utc)?
                }
            }
            'S' => {
                if sign == 0 {
                    adjust_second_absolute(dt, value as u32, utc)?
                } else {
                    adjust_second(dt, sign * value, utc)?
                }
            }
            _ => return Err(format!("Cannot apply date adjustment: {}", adj)),
        };

        secs = new_dt.timestamp();
    }

    Ok((secs, nsecs))
}

/// Parse a named time unit (day name or month name).
fn parse_named_time_unit(name: &str, unit: char) -> Option<i64> {
    let name_lower = name.to_lowercase();

    if unit == 'd' || unit == 'w' {
        // Weekday names
        match name_lower.as_str() {
            "sun" | "sunday" => Some(0),
            "mon" | "monday" => Some(1),
            "tue" | "tuesday" => Some(2),
            "wed" | "wednesday" => Some(3),
            "thu" | "thursday" => Some(4),
            "fri" | "friday" => Some(5),
            "sat" | "saturday" => Some(6),
            _ => None,
        }
    } else if unit == 'm' {
        // Month names
        match name_lower.as_str() {
            "jan" | "january" => Some(1),
            "feb" | "february" => Some(2),
            "mar" | "march" => Some(3),
            "apr" | "april" => Some(4),
            "may" => Some(5),
            "jun" | "june" => Some(6),
            "jul" | "july" => Some(7),
            "aug" | "august" => Some(8),
            "sep" | "september" => Some(9),
            "oct" | "october" => Some(10),
            "nov" | "november" => Some(11),
            "dec" | "december" => Some(12),
            _ => None,
        }
    } else {
        None
    }
}

fn adjust_year(dt: DateTime<Utc>, delta: i64, _utc: bool) -> Result<DateTime<Utc>, String> {
    let new_year = dt.year() + delta as i32;
    let day = dt.day().min(days_in_month(new_year, dt.month()));
    match NaiveDate::from_ymd_opt(new_year, dt.month(), day) {
        Some(date) => {
            let time = dt.time();
            Ok(date.and_time(time).and_utc())
        }
        None => Err("Invalid date after year adjustment".to_string()),
    }
}

fn adjust_year_absolute(dt: DateTime<Utc>, year: i32, _utc: bool) -> Result<DateTime<Utc>, String> {
    let day = dt.day().min(days_in_month(year, dt.month()));
    match NaiveDate::from_ymd_opt(year, dt.month(), day) {
        Some(date) => {
            let time = dt.time();
            Ok(date.and_time(time).and_utc())
        }
        None => Err(format!("Invalid date for year {}", year)),
    }
}

fn adjust_month(dt: DateTime<Utc>, delta: i64, _utc: bool) -> Result<DateTime<Utc>, String> {
    let total_months = dt.year() * 12 + dt.month() as i32 - 1 + delta as i32;
    let new_year = total_months.div_euclid(12);
    let new_month = (total_months.rem_euclid(12) + 1) as u32;
    let day = dt.day().min(days_in_month(new_year, new_month));
    match NaiveDate::from_ymd_opt(new_year, new_month, day) {
        Some(date) => {
            let time = dt.time();
            Ok(date.and_time(time).and_utc())
        }
        None => Err("Invalid date after month adjustment".to_string()),
    }
}

fn adjust_month_absolute(
    dt: DateTime<Utc>,
    month: u32,
    _utc: bool,
) -> Result<DateTime<Utc>, String> {
    if !(1..=12).contains(&month) {
        return Err(format!("Invalid month: {}", month));
    }
    let day = dt.day().min(days_in_month(dt.year(), month));
    match NaiveDate::from_ymd_opt(dt.year(), month, day) {
        Some(date) => {
            let time = dt.time();
            Ok(date.and_time(time).and_utc())
        }
        None => Err(format!("Invalid date for month {}", month)),
    }
}

fn adjust_to_month_name(
    dt: DateTime<Utc>,
    target_month: u32,
    sign: i64,
    _utc: bool,
) -> Result<DateTime<Utc>, String> {
    let current_month = dt.month();
    let delta = if sign > 0 {
        if target_month > current_month {
            (target_month - current_month) as i64
        } else if target_month < current_month {
            (12 - current_month + target_month) as i64
        } else {
            0
        }
    } else if target_month < current_month {
        -((current_month - target_month) as i64)
    } else if target_month > current_month {
        -((current_month + 12 - target_month) as i64)
    } else {
        0
    };
    adjust_month(dt, delta, _utc)
}

fn adjust_week(dt: DateTime<Utc>, delta: i64, _utc: bool) -> Result<DateTime<Utc>, String> {
    adjust_day(dt, delta * 7, _utc)
}

fn adjust_day(dt: DateTime<Utc>, delta: i64, _utc: bool) -> Result<DateTime<Utc>, String> {
    let duration = chrono::Duration::days(delta);
    Ok(dt + duration)
}

fn adjust_day_absolute(dt: DateTime<Utc>, day: u32, _utc: bool) -> Result<DateTime<Utc>, String> {
    let max_day = days_in_month(dt.year(), dt.month());
    if day < 1 || day > max_day {
        return Err(format!("Invalid day {} for month", day));
    }
    match NaiveDate::from_ymd_opt(dt.year(), dt.month(), day) {
        Some(date) => {
            let time = dt.time();
            Ok(date.and_time(time).and_utc())
        }
        None => Err(format!("Invalid date for day {}", day)),
    }
}

fn adjust_to_weekday_name(
    dt: DateTime<Utc>,
    target_weekday: u32,
    sign: i64,
    _utc: bool,
) -> Result<DateTime<Utc>, String> {
    let current_weekday = dt.weekday().num_days_from_sunday();
    let delta = if sign > 0 {
        if target_weekday > current_weekday {
            (target_weekday - current_weekday) as i64
        } else if target_weekday < current_weekday {
            (7 - current_weekday + target_weekday) as i64
        } else {
            0
        }
    } else if target_weekday < current_weekday {
        -((current_weekday - target_weekday) as i64)
    } else if target_weekday > current_weekday {
        -((current_weekday + 7 - target_weekday) as i64)
    } else {
        0
    };
    adjust_day(dt, delta, _utc)
}

fn adjust_hour(dt: DateTime<Utc>, delta: i64, _utc: bool) -> Result<DateTime<Utc>, String> {
    let duration = chrono::Duration::hours(delta);
    Ok(dt + duration)
}

fn adjust_hour_absolute(dt: DateTime<Utc>, hour: u32, _utc: bool) -> Result<DateTime<Utc>, String> {
    if hour > 23 {
        return Err(format!("Invalid hour: {}", hour));
    }
    let date = dt.date_naive();
    match NaiveTime::from_hms_opt(hour, dt.minute(), dt.second()) {
        Some(time) => Ok(date.and_time(time).and_utc()),
        None => Err(format!("Invalid time for hour {}", hour)),
    }
}

fn adjust_minute(dt: DateTime<Utc>, delta: i64, _utc: bool) -> Result<DateTime<Utc>, String> {
    let duration = chrono::Duration::minutes(delta);
    Ok(dt + duration)
}

fn adjust_minute_absolute(
    dt: DateTime<Utc>,
    minute: u32,
    _utc: bool,
) -> Result<DateTime<Utc>, String> {
    if minute > 59 {
        return Err(format!("Invalid minute: {}", minute));
    }
    let date = dt.date_naive();
    match NaiveTime::from_hms_opt(dt.hour(), minute, dt.second()) {
        Some(time) => Ok(date.and_time(time).and_utc()),
        None => Err(format!("Invalid time for minute {}", minute)),
    }
}

fn adjust_second(dt: DateTime<Utc>, delta: i64, _utc: bool) -> Result<DateTime<Utc>, String> {
    let duration = chrono::Duration::seconds(delta);
    Ok(dt + duration)
}

fn adjust_second_absolute(
    dt: DateTime<Utc>,
    second: u32,
    _utc: bool,
) -> Result<DateTime<Utc>, String> {
    if second > 59 {
        return Err(format!("Invalid second: {}", second));
    }
    let date = dt.date_naive();
    match NaiveTime::from_hms_opt(dt.hour(), dt.minute(), second) {
        Some(time) => Ok(date.and_time(time).and_utc()),
        None => Err(format!("Invalid time for second {}", second)),
    }
}

fn days_in_month(year: i32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if year % 400 == 0 || (year % 4 == 0 && year % 100 != 0) {
                29
            } else {
                28
            }
        }
        _ => 30,
    }
}

/// Format the date according to the options.
fn format_date(secs: i64, nsecs: u32, opts: &DateOptions) -> String {
    let dt = DateTime::from_timestamp(secs, nsecs).unwrap_or_else(Utc::now);

    // Handle ISO 8601 format
    if let Some(iso_fmt) = opts.iso8601 {
        return format_iso8601(dt, nsecs, iso_fmt, opts.utc);
    }

    // Handle RFC 2822 format
    if opts.rfc2822 {
        return format_rfc2822(dt, opts.utc);
    }

    // Use custom format or default
    let format = opts
        .output_format
        .as_deref()
        .unwrap_or("%a %b %e %H:%M:%S %Z %Y");

    format_strftime(dt, nsecs, format, opts.utc)
}

/// Format date in ISO 8601 format.
fn format_iso8601(dt: DateTime<Utc>, nsecs: u32, precision: Iso8601Format, utc: bool) -> String {
    let mut result = dt.format("%Y-%m-%d").to_string();

    if precision == Iso8601Format::Date {
        return result;
    }

    result.push_str(&dt.format("T%H").to_string());

    if precision == Iso8601Format::Hours {
        result.push_str(&format_timezone_iso(utc));
        return result;
    }

    result.push_str(&dt.format(":%M").to_string());

    if precision == Iso8601Format::Minutes {
        result.push_str(&format_timezone_iso(utc));
        return result;
    }

    result.push_str(&dt.format(":%S").to_string());

    if precision == Iso8601Format::Seconds {
        result.push_str(&format_timezone_iso(utc));
        return result;
    }

    // Nanoseconds
    result.push_str(&format!(",{:09}", nsecs));
    result.push_str(&format_timezone_iso(utc));

    result
}

fn format_timezone_iso(_utc: bool) -> String {
    // In this virtual environment, we always use UTC
    "+00:00".to_string()
}

/// Format date in RFC 2822 format.
fn format_rfc2822(dt: DateTime<Utc>, _utc: bool) -> String {
    // In virtual environment, always use UTC
    dt.format("%a, %d %b %Y %H:%M:%S +0000").to_string()
}

/// Format date using strftime-style format string with %N extension for nanoseconds.
fn format_strftime(dt: DateTime<Utc>, nsecs: u32, format: &str, _utc: bool) -> String {
    // First, handle our custom %N extension for nanoseconds
    let format = expand_nanoseconds(format, nsecs);

    // Handle %+ which is the default date format
    let format = format.replace("%+", "%a %b %e %H:%M:%S %Z %Y");

    // Handle %Z for timezone name (always UTC in virtual environment)
    let format = format.replace("%Z", "UTC");

    // Handle %z for timezone offset (always +0000 in virtual environment)
    let format = format.replace("%z", "+0000");

    dt.format(&format).to_string()
}

/// Expand %N, %nN, and %-N format specifiers for nanoseconds.
fn expand_nanoseconds(format: &str, nsecs: u32) -> String {
    let mut result = String::with_capacity(format.len());
    let mut chars = format.chars().peekable();
    let mut prev_was_percent = false;

    while let Some(c) = chars.next() {
        if prev_was_percent {
            prev_was_percent = false;
            match c {
                'N' => {
                    // %N = 9 digits
                    result.push_str(&format!("{:09}", nsecs));
                }
                '-' => {
                    // Check for %-N
                    if chars.peek() == Some(&'N') {
                        chars.next();
                        // %-N = automatic precision (we'll use 9 for simplicity)
                        result.push_str(&format!("{:09}", nsecs));
                    } else {
                        result.push('%');
                        result.push('-');
                    }
                }
                '0'..='9' => {
                    // Collect all digits
                    let mut width_str = String::from(c);
                    while let Some(&next) = chars.peek() {
                        if next.is_ascii_digit() {
                            width_str.push(chars.next().unwrap());
                        } else {
                            break;
                        }
                    }
                    // Check if followed by N
                    if chars.peek() == Some(&'N') {
                        chars.next();
                        let width: usize = width_str.parse().unwrap_or(9);
                        if width <= 9 {
                            let divisor = 10u32.pow(9 - width as u32);
                            let value = nsecs / divisor;
                            result.push_str(&format!("{:0width$}", value, width = width));
                        } else {
                            // More than 9 digits, pad with zeros
                            result.push_str(&format!("{:09}", nsecs));
                            for _ in 9..width {
                                result.push('0');
                            }
                        }
                    } else {
                        // Not %nN, put back the %
                        result.push('%');
                        result.push_str(&width_str);
                    }
                }
                '%' => {
                    // Keep %% as %% for chrono to handle
                    result.push('%');
                    result.push('%');
                }
                _ => {
                    result.push('%');
                    result.push(c);
                }
            }
        } else if c == '%' {
            prev_was_percent = true;
        } else {
            result.push(c);
        }
    }

    if prev_was_percent {
        result.push('%');
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MockFilesystem, StringStderr, StringStdin, StringStdout};

    fn make_env(
        args: Vec<&str>,
    ) -> Environment<StringStdin, StringStdout, StringStderr, MockFilesystem> {
        Environment {
            stdin: StringStdin::new(""),
            stdout: StringStdout::new(),
            stderr: StringStderr::new(),
            fs: MockFilesystem::new(),
            env: std::collections::HashMap::new(),
            args: args.into_iter().map(|s| s.to_string()).collect(),
            cwd: utf8path::Path::from("/"),
            exit_signaled: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }

    // ========================================================================
    // Basic functionality tests
    // ========================================================================

    #[test]
    fn default_output_produces_something() {
        let env = make_env(vec!["date"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert!(!stdout.is_empty());
        assert!(stdout.ends_with('\n'));
    }

    #[test]
    fn seconds_since_epoch_format() {
        let env = make_env(vec!["date", "-r", "0", "+%s"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert_eq!("0\n", stdout);
    }

    #[test]
    fn specific_timestamp_format() {
        let env = make_env(vec!["date", "-r", "1000000000", "+%Y-%m-%d"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert_eq!("2001-09-09\n", stdout);
    }

    #[test]
    fn unix_epoch_year() {
        let env = make_env(vec!["date", "-r", "0", "+%Y"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("1970\n", env.stdout.into_string());
    }

    // ========================================================================
    // -u flag (UTC) tests
    // ========================================================================

    #[test]
    fn utc_flag_accepted() {
        let env = make_env(vec!["date", "-u", "-r", "0", "+%H:%M:%S"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert_eq!("00:00:00\n", stdout);
    }

    // ========================================================================
    // -R flag (RFC 2822) tests
    // ========================================================================

    #[test]
    fn rfc2822_format() {
        let env = make_env(vec!["date", "-R", "-r", "0"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert!(stdout.contains("Thu, 01 Jan 1970"));
        assert!(stdout.contains("+0000"));
    }

    // ========================================================================
    // -I flag (ISO 8601) tests
    // ========================================================================

    #[test]
    fn iso8601_date_only() {
        let env = make_env(vec!["date", "-I", "-r", "1000000000"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert_eq!("2001-09-09\n", stdout);
    }

    #[test]
    fn iso8601_date_explicit() {
        let env = make_env(vec!["date", "-Idate", "-r", "1000000000"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("2001-09-09\n", env.stdout.into_string());
    }

    #[test]
    fn iso8601_hours() {
        let env = make_env(vec!["date", "-Ihours", "-r", "1000000000"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert!(stdout.starts_with("2001-09-09T"));
        assert!(stdout.contains("+00:00") || stdout.contains("-"));
    }

    #[test]
    fn iso8601_minutes() {
        let env = make_env(vec!["date", "-Iminutes", "-r", "1000000000"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert!(stdout.contains(":46") || stdout.contains(":16")); // depends on timezone
    }

    #[test]
    fn iso8601_seconds() {
        let env = make_env(vec!["date", "-Iseconds", "-r", "1000000000"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert!(stdout.contains(":40+") || stdout.contains(":40-") || stdout.contains(":40\n"));
    }

    #[test]
    fn iso8601_nanoseconds() {
        let env = make_env(vec!["date", "-Ins", "-r", "1000000000"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert!(stdout.contains(","));
    }

    #[test]
    fn iso8601_invalid_format() {
        let env = make_env(vec!["date", "-Iinvalid"]);
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("invalid argument"));
    }

    // ========================================================================
    // Multiple format conflict tests
    // ========================================================================

    #[test]
    fn multiple_formats_iso_and_rfc() {
        let env = make_env(vec!["date", "-I", "-R"]);
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("multiple output formats"));
    }

    #[test]
    fn multiple_formats_iso_and_plus() {
        // Use -Idate to ensure -I gets a valid optional argument,
        // then +%Y is a separate output format
        let env = make_env(vec!["date", "-Idate", "+%Y"]);
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("multiple output formats"));
    }

    // ========================================================================
    // -r flag tests
    // ========================================================================

    #[test]
    fn reference_seconds() {
        let env = make_env(vec!["date", "-r", "86400", "+%Y-%m-%d"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("1970-01-02\n", env.stdout.into_string());
    }

    #[test]
    fn reference_negative_seconds() {
        let env = make_env(vec!["date", "-r", "-86400", "+%Y-%m-%d"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("1969-12-31\n", env.stdout.into_string());
    }

    #[test]
    fn reference_file() {
        let env = make_env(vec!["date", "-r", "test.txt", "+%s"]);
        env.fs.add_file("test.txt", "content");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert_eq!("0\n", stdout);
    }

    #[test]
    fn reference_file_not_found() {
        let env = make_env(vec!["date", "-r", "nonexistent.txt"]);
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("No such file"));
    }

    // ========================================================================
    // Format string tests
    // ========================================================================

    #[test]
    fn format_year_month_day() {
        let env = make_env(vec!["date", "-r", "1234567890", "+%Y-%m-%d"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert_eq!("2009-02-13\n", stdout);
    }

    #[test]
    fn format_hour_minute_second() {
        let env = make_env(vec!["date", "-r", "0", "+%H:%M:%S"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("00:00:00\n", env.stdout.into_string());
    }

    #[test]
    fn format_weekday() {
        let env = make_env(vec!["date", "-r", "0", "+%A"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("Thursday\n", env.stdout.into_string());
    }

    #[test]
    fn format_month_name() {
        let env = make_env(vec!["date", "-r", "0", "+%B"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("January\n", env.stdout.into_string());
    }

    #[test]
    fn format_nanoseconds() {
        let env = make_env(vec!["date", "-r", "0", "+%N"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert_eq!("000000000\n", stdout);
    }

    #[test]
    fn format_milliseconds() {
        let env = make_env(vec!["date", "-r", "0", "+%3N"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("000\n", env.stdout.into_string());
    }

    #[test]
    fn format_percent_literal() {
        let env = make_env(vec!["date", "-r", "0", "+%%"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("%\n", env.stdout.into_string());
    }

    #[test]
    fn format_with_text() {
        let env = make_env(vec!["date", "-r", "0", "+DATE: %Y-%m-%d"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("DATE: 1970-01-01\n", env.stdout.into_string());
    }

    #[test]
    fn format_newline() {
        let env = make_env(vec!["date", "-r", "0", "+%Y%n%m"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("1970\n01\n", env.stdout.into_string());
    }

    #[test]
    fn format_tab() {
        let env = make_env(vec!["date", "-r", "0", "+%Y%t%m"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("1970\t01\n", env.stdout.into_string());
    }

    // ========================================================================
    // -j flag tests (no set)
    // ========================================================================

    #[test]
    fn no_set_flag_accepted() {
        let env = make_env(vec!["date", "-j", "-r", "0", "+%Y"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("1970\n", env.stdout.into_string());
    }

    // ========================================================================
    // -f flag tests (input format)
    // ========================================================================

    #[test]
    fn input_format_parse() {
        let env = make_env(vec!["date", "-j", "-f", "%Y-%m-%d", "2020-06-15", "+%Y"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("2020\n", env.stdout.into_string());
    }

    #[test]
    fn input_format_convert() {
        let env = make_env(vec![
            "date",
            "-j",
            "-f",
            "%Y-%m-%d",
            "2020-06-15",
            "+%m/%d/%Y",
        ]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("06/15/2020\n", env.stdout.into_string());
    }

    #[test]
    fn input_format_missing_date_string() {
        let env = make_env(vec!["date", "-j", "-f", "%Y-%m-%d"]);
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("requires a date string"));
    }

    // ========================================================================
    // -v flag tests (adjustments)
    // ========================================================================

    #[test]
    fn adjust_add_day() {
        let env = make_env(vec!["date", "-r", "0", "-v", "+1d", "+%Y-%m-%d"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("1970-01-02\n", env.stdout.into_string());
    }

    #[test]
    fn adjust_subtract_day() {
        let env = make_env(vec!["date", "-r", "86400", "-v", "-1d", "+%Y-%m-%d"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("1970-01-01\n", env.stdout.into_string());
    }

    #[test]
    fn adjust_add_month() {
        let env = make_env(vec!["date", "-r", "0", "-v", "+1m", "+%Y-%m-%d"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("1970-02-01\n", env.stdout.into_string());
    }

    #[test]
    fn adjust_add_year() {
        let env = make_env(vec!["date", "-r", "0", "-v", "+1y", "+%Y-%m-%d"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("1971-01-01\n", env.stdout.into_string());
    }

    #[test]
    fn adjust_add_week() {
        let env = make_env(vec!["date", "-r", "0", "-v", "+1w", "+%Y-%m-%d"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("1970-01-08\n", env.stdout.into_string());
    }

    #[test]
    fn adjust_add_hour() {
        let env = make_env(vec!["date", "-r", "0", "-v", "+1H", "+%H"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("01\n", env.stdout.into_string());
    }

    #[test]
    fn adjust_add_minute() {
        let env = make_env(vec!["date", "-r", "0", "-v", "+30M", "+%M"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("30\n", env.stdout.into_string());
    }

    #[test]
    fn adjust_add_second() {
        let env = make_env(vec!["date", "-r", "0", "-v", "+45S", "+%S"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("45\n", env.stdout.into_string());
    }

    #[test]
    fn adjust_set_month() {
        let env = make_env(vec!["date", "-r", "0", "-v", "6m", "+%m"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("06\n", env.stdout.into_string());
    }

    #[test]
    fn adjust_set_day() {
        let env = make_env(vec!["date", "-r", "0", "-v", "15d", "+%d"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("15\n", env.stdout.into_string());
    }

    #[test]
    fn adjust_multiple() {
        let env = make_env(vec![
            "date",
            "-r",
            "0",
            "-v",
            "+1y",
            "-v",
            "+1m",
            "-v",
            "+1d",
            "+%Y-%m-%d",
        ]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("1971-02-02\n", env.stdout.into_string());
    }

    #[test]
    fn adjust_invalid() {
        let env = make_env(vec!["date", "-v", "xyz"]);
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("Cannot apply"));
    }

    // ========================================================================
    // Edge cases
    // ========================================================================

    #[test]
    fn leap_year_feb_29() {
        // 2020 is a leap year, Feb 29 exists
        let env = make_env(vec![
            "date",
            "-j",
            "-f",
            "%Y-%m-%d",
            "2020-02-29",
            "+%Y-%m-%d",
        ]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("2020-02-29\n", env.stdout.into_string());
    }

    #[test]
    fn month_adjustment_day_clamping() {
        // Jan 31 + 1 month should give Feb 28 or 29
        // Using a non-leap year timestamp for Jan 31
        // Jan 31, 2019 00:00:00 UTC = 1548892800
        let env = make_env(vec!["date", "-r", "1548892800", "-v", "+1m", "+%Y-%m-%d"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert_eq!("2019-02-28\n", stdout);
    }

    #[test]
    fn year_2000() {
        let env = make_env(vec!["date", "-r", "946684800", "+%Y-%m-%d"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("2000-01-01\n", env.stdout.into_string());
    }

    #[test]
    fn large_timestamp() {
        // Year 2100
        let env = make_env(vec!["date", "-r", "4102444800", "+%Y"]);
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("2100\n", env.stdout.into_string());
    }

    // ========================================================================
    // expand_nanoseconds unit tests
    // ========================================================================

    #[test]
    fn expand_nanoseconds_full() {
        let result = expand_nanoseconds("%N", 123456789);
        assert_eq!("123456789", result);
    }

    #[test]
    fn expand_nanoseconds_partial() {
        let result = expand_nanoseconds("%3N", 123456789);
        assert_eq!("123", result);
    }

    #[test]
    fn expand_nanoseconds_with_text() {
        let result = expand_nanoseconds("time: %H:%M:%S.%3N", 500000000);
        assert_eq!("time: %H:%M:%S.500", result);
    }

    #[test]
    fn expand_nanoseconds_percent_literal() {
        // %% is preserved as %% for chrono to handle (it will output a literal %)
        let result = expand_nanoseconds("%%N", 123456789);
        assert_eq!("%%N", result);
    }

    // ========================================================================
    // days_in_month unit tests
    // ========================================================================

    #[test]
    fn days_in_month_january() {
        assert_eq!(31, days_in_month(2020, 1));
    }

    #[test]
    fn days_in_month_february_leap() {
        assert_eq!(29, days_in_month(2020, 2));
    }

    #[test]
    fn days_in_month_february_non_leap() {
        assert_eq!(28, days_in_month(2019, 2));
    }

    #[test]
    fn days_in_month_february_century_non_leap() {
        assert_eq!(28, days_in_month(1900, 2));
    }

    #[test]
    fn days_in_month_february_400_year_leap() {
        assert_eq!(29, days_in_month(2000, 2));
    }

    #[test]
    fn days_in_month_april() {
        assert_eq!(30, days_in_month(2020, 4));
    }
}

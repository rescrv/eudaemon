//! The grep builtin: file pattern searcher.

use std::collections::VecDeque;

use getopts::Options;
use regex::Regex;
use regex::RegexBuilder;

use crate::{
    Environment, Error, ExitCode, FileType, Filesystem, FsError, StdioIn, StdioOut,
    fs_error_message, resolve_path,
};

/// The type of pattern matching to perform.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum GrepBehavior {
    /// Basic regular expressions (default).
    #[default]
    Basic,
    /// Extended regular expressions (-E).
    Extended,
    /// Fixed strings (-F).
    Fixed,
}

/// Options for the grep command.
#[derive(Clone, Debug, Default)]
struct GrepOptions {
    /// Pattern matching behavior.
    behavior: GrepBehavior,
    /// -i: Case insensitive matching.
    ignore_case: bool,
    /// -v: Invert match (select non-matching lines).
    invert_match: bool,
    /// -c: Only print a count of matching lines.
    count_only: bool,
    /// -l: Only print names of files with matches.
    files_with_matches: bool,
    /// -L: Only print names of files without matches.
    files_without_match: bool,
    /// -n: Prefix each line with line number.
    line_number: bool,
    /// -H: Always print filename headers.
    with_filename: bool,
    /// -h: Never print filename headers.
    no_filename: bool,
    /// -o: Print only the matching part of lines.
    only_matching: bool,
    /// -q: Quiet mode, suppress normal output.
    quiet: bool,
    /// -s: Suppress error messages about nonexistent or unreadable files.
    no_messages: bool,
    /// -w: Match whole words only.
    word_regexp: bool,
    /// -x: Match whole lines only.
    line_regexp: bool,
    /// -r/-R: Recursively search directories.
    recursive: bool,
    /// -A num: Print num lines of trailing context.
    after_context: usize,
    /// -B num: Print num lines of leading context.
    before_context: usize,
    /// -m num: Stop after num matches.
    max_count: Option<usize>,
    /// -a: Treat binary files as text.
    text: bool,
    /// Multiple patterns from -e options.
    patterns: Vec<String>,
}

fn build_options() -> Options {
    let mut opts = Options::new();
    // Pattern type options
    opts.optflag(
        "E",
        "extended-regexp",
        "Interpret pattern as extended regex",
    );
    opts.optflag("F", "fixed-strings", "Interpret pattern as fixed strings");
    opts.optflag(
        "G",
        "basic-regexp",
        "Interpret pattern as basic regex (default)",
    );

    // Matching control
    opts.optflag("i", "ignore-case", "Ignore case distinctions");
    opts.optflag("v", "invert-match", "Select non-matching lines");
    opts.optflag("w", "word-regexp", "Match whole words only");
    opts.optflag("x", "line-regexp", "Match whole lines only");

    // General output control
    opts.optflag("c", "count", "Print only a count of matching lines");
    opts.optflag(
        "l",
        "files-with-matches",
        "Print only names of files with matches",
    );
    opts.optflag(
        "L",
        "files-without-match",
        "Print only names of files without matches",
    );
    opts.optopt("m", "max-count", "Stop after NUM matches", "NUM");
    opts.optflag("o", "only-matching", "Print only the matched parts");
    opts.optflag("q", "quiet", "Suppress all normal output");
    opts.optflag("s", "no-messages", "Suppress error messages");

    // Output line prefix control
    opts.optflag("H", "with-filename", "Print filename for each match");
    opts.optflag("h", "no-filename", "Suppress filename prefix");
    opts.optflag("n", "line-number", "Print line number with output");

    // Context control
    opts.optopt(
        "A",
        "after-context",
        "Print NUM lines of trailing context",
        "NUM",
    );
    opts.optopt(
        "B",
        "before-context",
        "Print NUM lines of leading context",
        "NUM",
    );
    opts.optopt("C", "context", "Print NUM lines of context", "NUM");

    // File and directory selection
    opts.optflag("r", "recursive", "Recursively search directories");
    opts.optflag("R", "", "Same as -r");
    opts.optflag("a", "text", "Treat binary files as text");

    // Pattern specification
    opts.optmulti("e", "regexp", "Use PATTERN for matching", "PATTERN");
    opts.optopt("f", "file", "Obtain patterns from FILE", "FILE");

    opts
}

/// The grep builtin: search files for patterns.
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
            env.stderr.write_line(&format!("grep: {}", e))?;
            return Ok(ExitCode::from(2));
        }
    };

    let mut grep_opts = GrepOptions::default();

    // Check program name first for egrep/fgrep behavior (flags can override)
    let prog_name = env.args.first().map(|s| s.as_str()).unwrap_or("grep");
    if prog_name.ends_with("egrep") {
        grep_opts.behavior = GrepBehavior::Extended;
    } else if prog_name.ends_with("fgrep") {
        grep_opts.behavior = GrepBehavior::Fixed;
    }

    // Pattern type flags (last one wins, can override program name)
    if matches.opt_present("G") {
        grep_opts.behavior = GrepBehavior::Basic;
    }
    if matches.opt_present("E") {
        grep_opts.behavior = GrepBehavior::Extended;
    }
    if matches.opt_present("F") {
        grep_opts.behavior = GrepBehavior::Fixed;
    }

    grep_opts.ignore_case = matches.opt_present("i");
    grep_opts.invert_match = matches.opt_present("v");
    grep_opts.count_only = matches.opt_present("c");
    grep_opts.files_with_matches = matches.opt_present("l");
    grep_opts.files_without_match = matches.opt_present("L");
    grep_opts.line_number = matches.opt_present("n");
    grep_opts.with_filename = matches.opt_present("H");
    grep_opts.no_filename = matches.opt_present("h");
    grep_opts.only_matching = matches.opt_present("o");
    grep_opts.quiet = matches.opt_present("q");
    grep_opts.no_messages = matches.opt_present("s");
    grep_opts.word_regexp = matches.opt_present("w");
    grep_opts.line_regexp = matches.opt_present("x");
    grep_opts.recursive = matches.opt_present("r") || matches.opt_present("R");
    grep_opts.text = matches.opt_present("a");

    // Context options
    if let Some(num) = matches.opt_str("A") {
        match num.parse() {
            Ok(n) => grep_opts.after_context = n,
            Err(_) => {
                env.stderr
                    .write_line(&format!("grep: invalid context length argument: {}", num))?;
                return Ok(ExitCode::from(2));
            }
        }
    }
    if let Some(num) = matches.opt_str("B") {
        match num.parse() {
            Ok(n) => grep_opts.before_context = n,
            Err(_) => {
                env.stderr
                    .write_line(&format!("grep: invalid context length argument: {}", num))?;
                return Ok(ExitCode::from(2));
            }
        }
    }
    if let Some(num) = matches.opt_str("C") {
        match num.parse() {
            Ok(n) => {
                grep_opts.after_context = n;
                grep_opts.before_context = n;
            }
            Err(_) => {
                env.stderr
                    .write_line(&format!("grep: invalid context length argument: {}", num))?;
                return Ok(ExitCode::from(2));
            }
        }
    }

    if let Some(num) = matches.opt_str("m") {
        match num.parse() {
            Ok(n) => grep_opts.max_count = Some(n),
            Err(_) => {
                env.stderr
                    .write_line(&format!("grep: invalid max count: {}", num))?;
                return Ok(ExitCode::from(2));
            }
        }
    }

    // Collect patterns from -e options
    grep_opts.patterns = matches.opt_strs("e");

    // Read patterns from file if -f specified
    if let Some(pattern_file) = matches.opt_str("f") {
        let resolved = resolve_path(env.cwd.as_str(), &pattern_file);
        match env.fs.read_to_string(&resolved) {
            Ok(contents) => {
                for line in contents.lines() {
                    if !line.is_empty() {
                        grep_opts.patterns.push(line.to_string());
                    }
                }
            }
            Err(e) => {
                env.stderr.write_line(&format!(
                    "grep: {}: {}",
                    pattern_file,
                    fs_error_message(&e)
                ))?;
                return Ok(ExitCode::from(2));
            }
        }
    }

    // Get free arguments
    let mut free_args = matches.free.clone();

    // If no patterns from -e or -f, first free argument is the pattern
    if grep_opts.patterns.is_empty() {
        if free_args.is_empty() {
            env.stderr.write_line("grep: no pattern specified")?;
            return Ok(ExitCode::from(2));
        }
        grep_opts.patterns.push(free_args.remove(0));
    }

    // Remaining arguments are files
    let files = free_args;

    // Build the matcher
    let matcher = match build_matcher(&grep_opts) {
        Ok(m) => m,
        Err(e) => {
            env.stderr.write_line(&format!("grep: {}", e))?;
            return Ok(ExitCode::from(2));
        }
    };

    // Determine if we should print filenames
    let print_filename = if grep_opts.no_filename {
        false
    } else if grep_opts.with_filename || grep_opts.recursive {
        true
    } else {
        files.len() > 1
    };

    let mut any_match = false;
    let mut any_error = false;

    if files.is_empty() {
        // Read from stdin
        match grep_stdin(env, &grep_opts, &matcher, print_filename) {
            Ok(matched) => any_match = matched,
            Err(_) => any_error = true,
        }
    } else {
        for file in &files {
            let resolved = resolve_path(env.cwd.as_str(), file);
            match grep_path(env, &resolved, file, &grep_opts, &matcher, print_filename) {
                Ok(matched) => {
                    if matched {
                        any_match = true;
                    }
                }
                Err(_) => any_error = true,
            }
            // Early exit if quiet and found a match
            if grep_opts.quiet && any_match {
                break;
            }
        }
    }

    // Exit codes: 0 = match found, 1 = no match, 2 = error
    // Note: -s suppresses error messages but doesn't change exit codes
    if any_error {
        Ok(ExitCode::from(2))
    } else if any_match {
        Ok(ExitCode::from(0))
    } else {
        Ok(ExitCode::from(1))
    }
}

/// The compiled pattern matcher.
enum Matcher {
    /// Regex-based matching.
    Regex(Vec<Regex>),
    /// Fixed string matching.
    Fixed {
        patterns: Vec<String>,
        ignore_case: bool,
    },
}

impl Matcher {
    /// Check if a line matches any pattern.
    fn is_match(&self, line: &str) -> bool {
        match self {
            Matcher::Regex(regexes) => regexes.iter().any(|r| r.is_match(line)),
            Matcher::Fixed {
                patterns,
                ignore_case,
            } => {
                if *ignore_case {
                    let line_lower = line.to_lowercase();
                    patterns
                        .iter()
                        .any(|p| line_lower.contains(&p.to_lowercase()))
                } else {
                    patterns.iter().any(|p| line.contains(p))
                }
            }
        }
    }

    /// Find all matches in a line and return their (start, end) byte offsets.
    fn find_matches(&self, line: &str) -> Vec<(usize, usize)> {
        let mut matches = Vec::new();
        match self {
            Matcher::Regex(regexes) => {
                for regex in regexes {
                    for m in regex.find_iter(line) {
                        matches.push((m.start(), m.end()));
                    }
                }
            }
            Matcher::Fixed {
                patterns,
                ignore_case,
            } => {
                for pattern in patterns {
                    if *ignore_case {
                        let line_lower = line.to_lowercase();
                        let pat_lower = pattern.to_lowercase();
                        let mut start = 0;
                        while let Some(pos) = line_lower[start..].find(&pat_lower) {
                            let abs_pos = start + pos;
                            matches.push((abs_pos, abs_pos + pattern.len()));
                            start = abs_pos + 1;
                        }
                    } else {
                        let mut start = 0;
                        while let Some(pos) = line[start..].find(pattern) {
                            let abs_pos = start + pos;
                            matches.push((abs_pos, abs_pos + pattern.len()));
                            start = abs_pos + 1;
                        }
                    }
                }
            }
        }
        // Sort by start position and deduplicate overlapping matches
        matches.sort_by_key(|m| m.0);
        matches
    }
}

/// Build a matcher from the grep options.
fn build_matcher(opts: &GrepOptions) -> Result<Matcher, String> {
    if opts.behavior == GrepBehavior::Fixed {
        return Ok(Matcher::Fixed {
            patterns: opts.patterns.clone(),
            ignore_case: opts.ignore_case,
        });
    }

    let mut regexes = Vec::new();
    for pattern in &opts.patterns {
        let mut adjusted_pattern = pattern.clone();

        // Handle word boundary matching
        if opts.word_regexp {
            adjusted_pattern = format!(r"\b{}\b", adjusted_pattern);
        }

        // Handle line matching
        if opts.line_regexp {
            adjusted_pattern = format!("^{}$", adjusted_pattern);
        }

        let regex = RegexBuilder::new(&adjusted_pattern)
            .case_insensitive(opts.ignore_case)
            .multi_line(true)
            .build()
            .map_err(|e| format!("invalid regex '{}': {}", pattern, e))?;
        regexes.push(regex);
    }

    Ok(Matcher::Regex(regexes))
}

/// Search stdin for matches.
fn grep_stdin<SI, SO, SE, FS>(
    env: &Environment<SI, SO, SE, FS>,
    opts: &GrepOptions,
    matcher: &Matcher,
    print_filename: bool,
) -> Result<bool, Error>
where
    SI: StdioIn,
    SO: StdioOut,
    SE: StdioOut,
    FS: Filesystem,
{
    let mut lines = Vec::new();
    while let Some(line) = env.stdin.read_line()? {
        lines.push(line);
    }

    let filename = if print_filename {
        Some("(standard input)")
    } else {
        None
    };
    grep_lines(env, &lines, filename, opts, matcher)
}

/// Search a path (file or directory) for matches.
fn grep_path<SI, SO, SE, FS>(
    env: &Environment<SI, SO, SE, FS>,
    resolved_path: &str,
    display_path: &str,
    opts: &GrepOptions,
    matcher: &Matcher,
    print_filename: bool,
) -> Result<bool, Error>
where
    SI: StdioIn,
    SO: StdioOut,
    SE: StdioOut,
    FS: Filesystem,
{
    // Check what type of file this is
    let stat = match env.fs.stat(resolved_path) {
        Ok(s) => s,
        Err(e) => {
            if !opts.no_messages {
                env.stderr.write_line(&format!(
                    "grep: {}: {}",
                    display_path,
                    fs_error_message(&e)
                ))?;
            }
            return Err(Error::from(e));
        }
    };

    if stat.file_type == FileType::Directory {
        if opts.recursive {
            grep_directory(
                env,
                resolved_path,
                display_path,
                opts,
                matcher,
                print_filename,
            )
        } else {
            if !opts.no_messages {
                env.stderr
                    .write_line(&format!("grep: {}: Is a directory", display_path))?;
            }
            Err(Error::Io(std::io::Error::new(
                std::io::ErrorKind::IsADirectory,
                "Is a directory",
            )))
        }
    } else {
        grep_file(
            env,
            resolved_path,
            display_path,
            opts,
            matcher,
            print_filename,
        )
    }
}

/// Recursively search a directory for matches.
fn grep_directory<SI, SO, SE, FS>(
    env: &Environment<SI, SO, SE, FS>,
    resolved_path: &str,
    display_path: &str,
    opts: &GrepOptions,
    matcher: &Matcher,
    print_filename: bool,
) -> Result<bool, Error>
where
    SI: StdioIn,
    SO: StdioOut,
    SE: StdioOut,
    FS: Filesystem,
{
    let entries = match env.fs.read_dir(resolved_path) {
        Ok(e) => e,
        Err(e) => {
            if !opts.no_messages {
                env.stderr.write_line(&format!(
                    "grep: {}: {}",
                    display_path,
                    fs_error_message(&e)
                ))?;
            }
            return Err(Error::from(e));
        }
    };

    let mut any_match = false;
    let mut sorted_entries: Vec<_> = entries.into_iter().collect();
    sorted_entries.sort_by(|a, b| a.0.cmp(&b.0));

    for (name, entry) in sorted_entries {
        // Skip hidden files in recursive mode
        if name.starts_with('.') {
            continue;
        }

        let child_resolved = format!("{}/{}", resolved_path.trim_end_matches('/'), name);
        let child_display = format!("{}/{}", display_path.trim_end_matches('/'), name);

        if entry.file_type == FileType::Directory {
            match grep_directory(
                env,
                &child_resolved,
                &child_display,
                opts,
                matcher,
                print_filename,
            ) {
                Ok(matched) if matched => any_match = true,
                _ => {}
            }
        } else if entry.file_type == FileType::RegularFile {
            match grep_file(
                env,
                &child_resolved,
                &child_display,
                opts,
                matcher,
                print_filename,
            ) {
                Ok(matched) if matched => any_match = true,
                _ => {}
            }
        }

        if opts.quiet && any_match {
            break;
        }
    }

    Ok(any_match)
}

/// Search a single file for matches.
fn grep_file<SI, SO, SE, FS>(
    env: &Environment<SI, SO, SE, FS>,
    resolved_path: &str,
    display_path: &str,
    opts: &GrepOptions,
    matcher: &Matcher,
    print_filename: bool,
) -> Result<bool, Error>
where
    SI: StdioIn,
    SO: StdioOut,
    SE: StdioOut,
    FS: Filesystem,
{
    let contents = match env.fs.read_to_string(resolved_path) {
        Ok(c) => c,
        Err(FsError::Io(e)) => {
            if !opts.no_messages {
                env.stderr
                    .write_line(&format!("grep: {}: {}", display_path, e))?;
            }
            return Err(Error::Io(e));
        }
    };

    // Check for binary content unless -a is specified
    if !opts.text && contents.bytes().any(|b| b == 0) {
        // Binary file - check if it matches but don't print lines
        let lines: Vec<&str> = contents.lines().collect();
        let has_match = lines.iter().any(|line| {
            let matches = matcher.is_match(line);
            if opts.invert_match { !matches } else { matches }
        });
        if has_match && !opts.quiet {
            env.stdout
                .write_line(&format!("Binary file {} matches", display_path))?;
        }
        return Ok(has_match);
    }

    let lines: Vec<String> = contents.lines().map(|s| s.to_string()).collect();
    let filename = if print_filename {
        Some(display_path)
    } else {
        None
    };
    grep_lines(env, &lines, filename, opts, matcher)
}

/// Context for printing with before/after context.
struct ContextPrinter {
    before_queue: VecDeque<(usize, String)>,
    after_remaining: usize,
    last_printed_line: Option<usize>,
    printed_separator: bool,
}

impl ContextPrinter {
    fn new(before_context: usize) -> Self {
        Self {
            before_queue: VecDeque::with_capacity(before_context),
            after_remaining: 0,
            last_printed_line: None,
            printed_separator: false,
        }
    }
}

/// Search lines for matches and output results.
fn grep_lines<SI, SO, SE, FS>(
    env: &Environment<SI, SO, SE, FS>,
    lines: &[String],
    filename: Option<&str>,
    opts: &GrepOptions,
    matcher: &Matcher,
) -> Result<bool, Error>
where
    SI: StdioIn,
    SO: StdioOut,
    SE: StdioOut,
    FS: Filesystem,
{
    let mut match_count = 0usize;
    let mut ctx = ContextPrinter::new(opts.before_context);
    let has_context = opts.before_context > 0 || opts.after_context > 0;

    for (idx, line) in lines.iter().enumerate() {
        let line_num = idx + 1;
        let raw_match = matcher.is_match(line);
        let is_match = if opts.invert_match {
            !raw_match
        } else {
            raw_match
        };

        if is_match {
            match_count += 1;

            if !opts.quiet
                && !opts.count_only
                && !opts.files_with_matches
                && !opts.files_without_match
            {
                // Print before context
                if has_context && !ctx.before_queue.is_empty() {
                    // Print separator if there's a gap
                    if let Some(last) = ctx.last_printed_line
                        && let Some((first_ctx_line, _)) = ctx.before_queue.front()
                        && *first_ctx_line > last + 1
                        && ctx.printed_separator
                    {
                        env.stdout.write_line("--")?;
                    }
                    for (ctx_line_num, ctx_line) in ctx.before_queue.drain(..) {
                        print_line(env, filename, Some(ctx_line_num), &ctx_line, '-')?;
                        ctx.last_printed_line = Some(ctx_line_num);
                    }
                }

                // Print separator if needed
                if has_context
                    && let Some(last) = ctx.last_printed_line
                    && line_num > last + 1
                    && ctx.printed_separator
                {
                    env.stdout.write_line("--")?;
                }

                // Print the matching line
                if opts.only_matching {
                    let matches = matcher.find_matches(line);
                    for (start, end) in matches {
                        let matched_text = &line[start..end];
                        print_line(
                            env,
                            filename,
                            if opts.line_number {
                                Some(line_num)
                            } else {
                                None
                            },
                            matched_text,
                            ':',
                        )?;
                    }
                } else {
                    print_line(
                        env,
                        filename,
                        if opts.line_number {
                            Some(line_num)
                        } else {
                            None
                        },
                        line,
                        ':',
                    )?;
                }
                ctx.last_printed_line = Some(line_num);
                ctx.printed_separator = true;
                ctx.after_remaining = opts.after_context;
            }

            // Check max count
            if let Some(max) = opts.max_count
                && match_count >= max
            {
                break;
            }
        } else {
            // Not a match - handle context
            if ctx.after_remaining > 0
                && !opts.quiet
                && !opts.count_only
                && !opts.files_with_matches
                && !opts.files_without_match
            {
                print_line(
                    env,
                    filename,
                    if opts.line_number {
                        Some(line_num)
                    } else {
                        None
                    },
                    line,
                    '-',
                )?;
                ctx.last_printed_line = Some(line_num);
                ctx.after_remaining -= 1;
            } else if opts.before_context > 0 {
                // Add to before context queue
                if ctx.before_queue.len() >= opts.before_context {
                    ctx.before_queue.pop_front();
                }
                ctx.before_queue.push_back((line_num, line.clone()));
            }
        }
    }

    // Handle output modes
    if opts.quiet {
        return Ok(match_count > 0);
    }

    if opts.count_only {
        if let Some(fname) = filename {
            env.stdout
                .write_line(&format!("{}:{}", fname, match_count))?;
        } else {
            env.stdout.write_line(&match_count.to_string())?;
        }
        return Ok(match_count > 0);
    }

    if opts.files_with_matches && match_count > 0 {
        if let Some(fname) = filename {
            env.stdout.write_line(fname)?;
        } else {
            env.stdout.write_line("(standard input)")?;
        }
    }

    if opts.files_without_match && match_count == 0 {
        if let Some(fname) = filename {
            env.stdout.write_line(fname)?;
        } else {
            env.stdout.write_line("(standard input)")?;
        }
    }

    Ok(match_count > 0)
}

/// Print a single output line with optional filename and line number prefix.
fn print_line<SI, SO, SE, FS>(
    env: &Environment<SI, SO, SE, FS>,
    filename: Option<&str>,
    line_num: Option<usize>,
    line: &str,
    separator: char,
) -> Result<(), Error>
where
    SI: StdioIn,
    SO: StdioOut,
    SE: StdioOut,
    FS: Filesystem,
{
    let mut output = String::new();

    if let Some(fname) = filename {
        output.push_str(fname);
        output.push(separator);
    }

    if let Some(num) = line_num {
        output.push_str(&num.to_string());
        output.push(separator);
    }

    output.push_str(line);

    env.stdout.write_line(&output)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TestFilesystemExt;
    use crate::test_utils::make_test_env_with_stdin;

    // ========================================================================
    // Basic matching tests
    // ========================================================================

    #[test]
    fn simple_match() {
        let env = make_test_env_with_stdin(vec!["grep", "hello"], "hello world\ngoodbye world");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("hello world\n", env.stdout.into_string());
    }

    #[test]
    fn no_match() {
        let env = make_test_env_with_stdin(vec!["grep", "notfound"], "hello world\ngoodbye world");
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        assert_eq!("", env.stdout.into_string());
    }

    #[test]
    fn multiple_matches() {
        let env =
            make_test_env_with_stdin(vec!["grep", "world"], "hello world\ngoodbye world\nfoo");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("hello world\ngoodbye world\n", env.stdout.into_string());
    }

    // ========================================================================
    // Case insensitive matching (-i)
    // ========================================================================

    #[test]
    fn ignore_case() {
        let env = make_test_env_with_stdin(vec!["grep", "-i", "hello"], "HELLO world\nhello world");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("HELLO world\nhello world\n", env.stdout.into_string());
    }

    #[test]
    fn ignore_case_no_match_without_flag() {
        let env = make_test_env_with_stdin(vec!["grep", "hello"], "HELLO world");
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
    }

    // ========================================================================
    // Inverted matching (-v)
    // ========================================================================

    #[test]
    fn invert_match() {
        let env = make_test_env_with_stdin(
            vec!["grep", "-v", "hello"],
            "hello world\ngoodbye world\nfoo",
        );
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("goodbye world\nfoo\n", env.stdout.into_string());
    }

    #[test]
    fn invert_match_all() {
        let env = make_test_env_with_stdin(vec!["grep", "-v", "x"], "hello\nworld");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("hello\nworld\n", env.stdout.into_string());
    }

    // ========================================================================
    // Count only (-c)
    // ========================================================================

    #[test]
    fn count_only() {
        let env = make_test_env_with_stdin(
            vec!["grep", "-c", "world"],
            "hello world\ngoodbye world\nfoo",
        );
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("2\n", env.stdout.into_string());
    }

    #[test]
    fn count_zero() {
        let env = make_test_env_with_stdin(vec!["grep", "-c", "notfound"], "hello world");
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        assert_eq!("0\n", env.stdout.into_string());
    }

    // ========================================================================
    // Line numbers (-n)
    // ========================================================================

    #[test]
    fn line_numbers() {
        let env = make_test_env_with_stdin(
            vec!["grep", "-n", "world"],
            "hello world\nfoo\ngoodbye world",
        );
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("1:hello world\n3:goodbye world\n", env.stdout.into_string());
    }

    // ========================================================================
    // Only matching (-o)
    // ========================================================================

    #[test]
    fn only_matching() {
        let env =
            make_test_env_with_stdin(vec!["grep", "-o", "wor.d"], "hello world goodbye world");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert!(stdout.contains("world"));
    }

    // ========================================================================
    // Quiet mode (-q)
    // ========================================================================

    #[test]
    fn quiet_match() {
        let env = make_test_env_with_stdin(vec!["grep", "-q", "hello"], "hello world");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("", env.stdout.into_string());
    }

    #[test]
    fn quiet_no_match() {
        let env = make_test_env_with_stdin(vec!["grep", "-q", "notfound"], "hello world");
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
        assert_eq!("", env.stdout.into_string());
    }

    // ========================================================================
    // Files with matches (-l)
    // ========================================================================

    #[test]
    fn files_with_matches() {
        let env = make_test_env_with_stdin(vec!["grep", "-l", "hello", "a.txt", "b.txt"], "");
        env.fs.add_file("/a.txt", "hello world\n");
        env.fs.add_file("/b.txt", "goodbye world\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("a.txt\n", env.stdout.into_string());
    }

    // ========================================================================
    // Files without match (-L)
    // ========================================================================

    #[test]
    fn files_without_match() {
        let env = make_test_env_with_stdin(vec!["grep", "-L", "hello", "a.txt", "b.txt"], "");
        env.fs.add_file("/a.txt", "hello world\n");
        env.fs.add_file("/b.txt", "goodbye world\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("b.txt\n", env.stdout.into_string());
    }

    // ========================================================================
    // Word matching (-w)
    // ========================================================================

    #[test]
    fn word_match() {
        let env = make_test_env_with_stdin(
            vec!["grep", "-w", "hello"],
            "hello world\nhelloworld\nthe hello end",
        );
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert!(stdout.contains("hello world"));
        assert!(stdout.contains("the hello end"));
        assert!(!stdout.contains("helloworld"));
    }

    // ========================================================================
    // Line matching (-x)
    // ========================================================================

    #[test]
    fn line_match() {
        let env =
            make_test_env_with_stdin(vec!["grep", "-x", "hello"], "hello\nhello world\nthe hello");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("hello\n", env.stdout.into_string());
    }

    // ========================================================================
    // Extended regex (-E)
    // ========================================================================

    #[test]
    fn extended_regex() {
        let env = make_test_env_with_stdin(vec!["grep", "-E", "hel+o"], "hello\nhelo\nhelllo");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert!(stdout.contains("hello"));
        assert!(stdout.contains("helo"));
        assert!(stdout.contains("helllo"));
    }

    #[test]
    fn extended_regex_alternation() {
        let env = make_test_env_with_stdin(vec!["grep", "-E", "cat|dog"], "cat\ndog\nbird");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("cat\ndog\n", env.stdout.into_string());
    }

    // ========================================================================
    // Fixed strings (-F)
    // ========================================================================

    #[test]
    fn fixed_strings() {
        let env = make_test_env_with_stdin(vec!["grep", "-F", "a.b"], "a.b\naxb\na*b");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("a.b\n", env.stdout.into_string());
    }

    #[test]
    fn fixed_strings_case_insensitive() {
        let env = make_test_env_with_stdin(vec!["grep", "-Fi", "hello"], "HELLO\nhello\nHeLLo");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert!(stdout.contains("HELLO"));
        assert!(stdout.contains("hello"));
        assert!(stdout.contains("HeLLo"));
    }

    // ========================================================================
    // Multiple patterns (-e)
    // ========================================================================

    #[test]
    fn multiple_patterns() {
        let env =
            make_test_env_with_stdin(vec!["grep", "-e", "cat", "-e", "dog"], "cat\ndog\nbird");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("cat\ndog\n", env.stdout.into_string());
    }

    // ========================================================================
    // Context (-A, -B, -C)
    // ========================================================================

    #[test]
    fn after_context() {
        let env = make_test_env_with_stdin(
            vec!["grep", "-A", "1", "match"],
            "before\nmatch\nafter\nend",
        );
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert!(stdout.contains("match"));
        assert!(stdout.contains("after"));
    }

    #[test]
    fn before_context() {
        let env = make_test_env_with_stdin(
            vec!["grep", "-B", "1", "match"],
            "start\nbefore\nmatch\nafter",
        );
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert!(stdout.contains("before"));
        assert!(stdout.contains("match"));
    }

    #[test]
    fn context_both() {
        let env = make_test_env_with_stdin(
            vec!["grep", "-C", "1", "match"],
            "start\nbefore\nmatch\nafter\nend",
        );
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert!(stdout.contains("before"));
        assert!(stdout.contains("match"));
        assert!(stdout.contains("after"));
    }

    // ========================================================================
    // Max count (-m)
    // ========================================================================

    #[test]
    fn max_count() {
        let env = make_test_env_with_stdin(vec!["grep", "-m", "2", "a"], "a1\na2\na3\na4");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert_eq!("a1\na2\n", stdout);
    }

    // ========================================================================
    // File operations
    // ========================================================================

    #[test]
    fn single_file() {
        let env = make_test_env_with_stdin(vec!["grep", "hello", "test.txt"], "");
        env.fs.add_file("/test.txt", "hello world\ngoodbye world\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("hello world\n", env.stdout.into_string());
    }

    #[test]
    fn multiple_files_with_names() {
        let env = make_test_env_with_stdin(vec!["grep", "hello", "a.txt", "b.txt"], "");
        env.fs.add_file("/a.txt", "hello from a\n");
        env.fs.add_file("/b.txt", "hello from b\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert!(stdout.contains("a.txt:hello from a"));
        assert!(stdout.contains("b.txt:hello from b"));
    }

    #[test]
    fn file_not_found() {
        let env = make_test_env_with_stdin(vec!["grep", "hello", "nonexistent.txt"], "");
        let result = bin(&env).unwrap();
        assert_eq!(2, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("nonexistent.txt"));
    }

    #[test]
    fn suppress_errors() {
        let env = make_test_env_with_stdin(vec!["grep", "-s", "hello", "nonexistent.txt"], "");
        let result = bin(&env).unwrap();
        // -s suppresses error messages but exit code is still 2
        assert_eq!(2, result.code());
        assert_eq!("", env.stderr.into_string());
    }

    // ========================================================================
    // Filename control (-h, -H)
    // ========================================================================

    #[test]
    fn no_filename_flag() {
        let env = make_test_env_with_stdin(vec!["grep", "-h", "hello", "a.txt", "b.txt"], "");
        env.fs.add_file("/a.txt", "hello from a\n");
        env.fs.add_file("/b.txt", "hello from b\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert!(!stdout.contains("a.txt:"));
        assert!(!stdout.contains("b.txt:"));
    }

    #[test]
    fn with_filename_single_file() {
        let env = make_test_env_with_stdin(vec!["grep", "-H", "hello", "test.txt"], "");
        env.fs.add_file("/test.txt", "hello world\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert!(stdout.contains("test.txt:"));
    }

    // ========================================================================
    // Recursive search (-r)
    // ========================================================================

    #[test]
    fn recursive_search() {
        let env = make_test_env_with_stdin(vec!["grep", "-r", "hello", "dir"], "");
        env.fs.mkdir_all("/dir/subdir").unwrap();
        env.fs.add_file("/dir/a.txt", "hello from a\n");
        env.fs.add_file("/dir/subdir/b.txt", "hello from b\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert!(stdout.contains("hello from a"));
        assert!(stdout.contains("hello from b"));
    }

    // ========================================================================
    // Pattern from file (-f)
    // ========================================================================

    #[test]
    fn patterns_from_file() {
        let env = make_test_env_with_stdin(vec!["grep", "-f", "patterns.txt"], "cat\ndog\nbird");
        env.fs.add_file("/patterns.txt", "cat\ndog\n");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("cat\ndog\n", env.stdout.into_string());
    }

    // ========================================================================
    // Edge cases
    // ========================================================================

    #[test]
    fn empty_pattern_matches_all() {
        let env = make_test_env_with_stdin(vec!["grep", ""], "hello\nworld");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert_eq!("hello\nworld\n", env.stdout.into_string());
    }

    #[test]
    fn no_pattern_error() {
        let env = make_test_env_with_stdin(vec!["grep"], "hello");
        let result = bin(&env).unwrap();
        assert_eq!(2, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("no pattern"));
    }

    #[test]
    fn invalid_regex_error() {
        let env = make_test_env_with_stdin(vec!["grep", "[invalid"], "hello");
        let result = bin(&env).unwrap();
        assert_eq!(2, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("invalid regex"));
    }

    #[test]
    fn invalid_after_context() {
        let env = make_test_env_with_stdin(vec!["grep", "-A", "foo", "pattern"], "hello");
        let result = bin(&env).unwrap();
        assert_eq!(2, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("invalid context length"));
    }

    #[test]
    fn invalid_before_context() {
        let env = make_test_env_with_stdin(vec!["grep", "-B", "bar", "pattern"], "hello");
        let result = bin(&env).unwrap();
        assert_eq!(2, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("invalid context length"));
    }

    #[test]
    fn invalid_max_count() {
        let env = make_test_env_with_stdin(vec!["grep", "-m", "baz", "pattern"], "hello");
        let result = bin(&env).unwrap();
        assert_eq!(2, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("invalid max count"));
    }

    #[test]
    fn fgrep_override_with_e_flag() {
        // fgrep should default to fixed, but -E should override to extended
        let env = make_test_env_with_stdin(vec!["fgrep", "-E", "hel+o"], "hello\nhelo");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        // With -E, hel+o is a regex matching hello and helo
        assert!(stdout.contains("hello"));
        assert!(stdout.contains("helo"));
    }

    #[test]
    fn combined_flags() {
        let env =
            make_test_env_with_stdin(vec!["grep", "-inv", "hello"], "HELLO world\ngoodbye\nfoo");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        // -i: case insensitive, -n: line numbers, -v: invert
        // Should match lines NOT containing hello (case insensitive)
        assert!(stdout.contains("2:goodbye"));
        assert!(stdout.contains("3:foo"));
    }
}

use crate::{Environment, Error, ExitCode, Filesystem, StdioIn, StdioOut};

mod base64;
mod basename;
mod cat;
mod comm;
mod cp;
mod cut;
mod date;
mod du;
mod echo;
mod env;
pub mod expand;
mod fold;
mod grep;
mod head;
mod ln;
mod ls;
mod markdownsp;
mod mkdir;
mod mktemp;
mod mv;
mod nl;
mod paste;
mod printf;
mod pwd;
mod readlink;
mod realpath;
mod rm;
mod rmdir;
mod seq;
pub mod sh;
mod shuf;
mod sort;
mod split;
mod stat;
mod tail;
mod tee;
mod test;
mod touch;
mod tr;
mod truncate;
mod uname;
mod unexpand;
mod uniq;
mod wc;
mod which;
mod yes;

const BIN_BASE64: &str = "base64";
const PATH_BASE64: &str = "/usr/bin/base64";
const CONTENTS_BASE64: &str =
    r#"//! The base64 command: encode or decode data using Base64 encoding (RFC 4648)."#;
const BIN_BASENAME: &str = "basename";
const PATH_BASENAME: &str = "/usr/bin/basename";
const CONTENTS_BASENAME: &str =
    r#"//! The basename command: strip directory and suffix from filenames."#;
const BIN_CAT: &str = "cat";
const PATH_CAT: &str = "/bin/cat";
const CONTENTS_CAT: &str = r#"//! The cat command: concatenate and print files."#;
const BIN_COMM: &str = "comm";
const PATH_COMM: &str = "/usr/bin/comm";
const CONTENTS_COMM: &str = r#"//! The comm command: select or reject lines common to two files."#;
const BIN_CP: &str = "cp";
const PATH_CP: &str = "/bin/cp";
const CONTENTS_CP: &str = r#"//! The cp utility: copy files."#;
const BIN_CUT: &str = "cut";
const PATH_CUT: &str = "/usr/bin/cut";
const CONTENTS_CUT: &str =
    r#"//! The cut utility: cut out selected portions of each line of a file."#;
const BIN_DATE: &str = "date";
const PATH_DATE: &str = "/bin/date";
const CONTENTS_DATE: &str = r#"//! The date command: display or set date and time."#;
const BIN_DU: &str = "du";
const PATH_DU: &str = "/usr/bin/du";
const CONTENTS_DU: &str = r#"//! The du command: display disk usage statistics."#;
const BIN_ECHO: &str = "echo";
const PATH_ECHO: &str = "/bin/echo";
const CONTENTS_ECHO: &str = r#"//! The echo command: write arguments to standard output."#;
const BIN_ENV: &str = "env";
const PATH_ENV: &str = "/usr/bin/env";
const CONTENTS_ENV: &str =
    r#"//! The env command: set environment and execute command, or print environment."#;
const BIN_EXIT: &str = "exit";
const PATH_EXIT: &str = "exit";
const CONTENTS_EXIT: &str = "";
const BIN_EXPAND: &str = "expand";
const PATH_EXPAND: &str = "/usr/bin/expand";
const CONTENTS_EXPAND: &str = r#"//! The expand utility converts tabs to spaces."#;
const BIN_FALSE: &str = "false";
const PATH_FALSE: &str = "/bin/false";
const CONTENTS_FALSE: &str = "";
const BIN_FOLD: &str = "fold";
const PATH_FOLD: &str = "/usr/bin/fold";
const CONTENTS_FOLD: &str = r#"//! The fold utility: wrap lines to a specified width."#;
const BIN_GREP: &str = "grep";
const PATH_GREP: &str = "/usr/bin/grep";
const CONTENTS_GREP: &str = r#"//! The grep command: file pattern searcher."#;
const BIN_EGREP: &str = "egrep";
const PATH_EGREP: &str = "/usr/bin/egrep";
const CONTENTS_EGREP: &str = "";
const BIN_FGREP: &str = "fgrep";
const PATH_FGREP: &str = "/usr/bin/fgrep";
const CONTENTS_FGREP: &str = "";
const BIN_HEAD: &str = "head";
const PATH_HEAD: &str = "/usr/bin/head";
const CONTENTS_HEAD: &str = r#"//! The head command: display first lines of a file."#;
const BIN_LN: &str = "ln";
const PATH_LN: &str = "/bin/ln";
const CONTENTS_LN: &str = r#"//! The ln utility: create links between files."#;
const BIN_LS: &str = "ls";
const PATH_LS: &str = "/bin/ls";
const CONTENTS_LS: &str = r#"//! The ls command: list directory contents."#;
const BIN_MKDIR: &str = "mkdir";
const PATH_MKDIR: &str = "/bin/mkdir";
const CONTENTS_MKDIR: &str = r#"//! The mkdir command: create directories."#;
const BIN_MKTEMP: &str = "mktemp";
const PATH_MKTEMP: &str = "/usr/bin/mktemp";
const CONTENTS_MKTEMP: &str = r#"//! The mktemp command: create temporary files or directories."#;
const BIN_MV: &str = "mv";
const PATH_MV: &str = "/bin/mv";
const CONTENTS_MV: &str = r#"//! The mv utility: move files."#;
const BIN_NL: &str = "nl";
const PATH_NL: &str = "/usr/bin/nl";
const CONTENTS_NL: &str = r#"//! The nl command: line numbering filter."#;
const BIN_PASTE: &str = "paste";
const PATH_PASTE: &str = "/usr/bin/paste";
const CONTENTS_PASTE: &str =
    r#"//! The paste utility: merge corresponding or subsequent lines of files."#;
const BIN_PRINTF: &str = "printf";
const PATH_PRINTF: &str = "/usr/bin/printf";
const CONTENTS_PRINTF: &str = r#"//! The printf command: format and print data."#;
const BIN_PWD: &str = "pwd";
const PATH_PWD: &str = "/bin/pwd";
const CONTENTS_PWD: &str = r#"//! The pwd command: print working directory name."#;
const BIN_READLINK: &str = "readlink";
const PATH_READLINK: &str = "/usr/bin/readlink";
const CONTENTS_READLINK: &str = r#"//! The readlink command: print the target of a symbolic link."#;
const BIN_REALPATH: &str = "realpath";
const PATH_REALPATH: &str = "/bin/realpath";
const CONTENTS_REALPATH: &str = r#"//! The realpath command: return resolved physical path."#;
const BIN_RM: &str = "rm";
const PATH_RM: &str = "/bin/rm";
const CONTENTS_RM: &str = r#"//! The rm command: remove directory entries."#;
const BIN_RMDIR: &str = "rmdir";
const PATH_RMDIR: &str = "/bin/rmdir";
const CONTENTS_RMDIR: &str = r#"//! The rmdir command: remove empty directories."#;
const BIN_SEQ: &str = "seq";
const PATH_SEQ: &str = "/usr/bin/seq";
const CONTENTS_SEQ: &str = r#"//! The seq utility: print sequences of numbers."#;
const BIN_SH: &str = "sh";
const PATH_SH: &str = "/bin/sh";
const CONTENTS_SH: &str = "";
const BIN_SHUF: &str = "shuf";
const PATH_SHUF: &str = "/usr/bin/shuf";
const CONTENTS_SHUF: &str = r#"//! The shuf command: generate random permutations."#;
const BIN_SORT: &str = "sort";
const PATH_SORT: &str = "/usr/bin/sort";
const CONTENTS_SORT: &str = r#"//! The sort command: sort lines of text files."#;
const BIN_SPLIT: &str = "split";
const PATH_SPLIT: &str = "/usr/bin/split";
const CONTENTS_SPLIT: &str = r#"//! The split command: split a file into pieces."#;
const BIN_STAT: &str = "stat";
const PATH_STAT: &str = "/usr/bin/stat";
const CONTENTS_STAT: &str = r#"//! The stat command: display file status."#;
const BIN_TAIL: &str = "tail";
const PATH_TAIL: &str = "/usr/bin/tail";
const CONTENTS_TAIL: &str = r#"//! The tail command: display the last part of a file."#;
const BIN_TEE: &str = "tee";
const PATH_TEE: &str = "/usr/bin/tee";
const CONTENTS_TEE: &str = r#"//! The tee command: duplicate standard input."#;
const BIN_TEST: &str = "test";
const PATH_TEST: &str = "/usr/bin/test";
const CONTENTS_TEST: &str = r#"//! The test command: condition evaluation utility.
//!
//! Evaluates conditional expressions and returns 0 (true) or 1 (false)."#;
const BIN_BRACKET: &str = "[";
const PATH_BRACKET: &str = "/usr/bin/[";
const CONTENTS_BRACKET: &str = r#"//! The [ command: condition evaluation utility (alias for test).
//!
//! Evaluates conditional expressions and returns 0 (true) or 1 (false).
//! Requires a closing ] as the last argument."#;
const BIN_TOUCH: &str = "touch";
const PATH_TOUCH: &str = "/usr/bin/touch";
const CONTENTS_TOUCH: &str = r#"//! The touch command: change file access and modification times."#;
const BIN_TR: &str = "tr";
const PATH_TR: &str = "/usr/bin/tr";
const CONTENTS_TR: &str = r#"//! The tr utility: translate, squeeze, and/or delete characters."#;
const BIN_TRUE: &str = "true";
const PATH_TRUE: &str = "/bin/true";
const CONTENTS_TRUE: &str = "";
const BIN_TRUNCATE: &str = "truncate";
const PATH_TRUNCATE: &str = "/usr/bin/truncate";
const CONTENTS_TRUNCATE: &str =
    r#"//! The truncate command: truncate or extend a file to a specified size."#;
const BIN_UNAME: &str = "uname";
const PATH_UNAME: &str = "/usr/bin/uname";
const CONTENTS_UNAME: &str = r#"//! The uname command: print eudaemonsh system identification."#;
const BIN_UNEXPAND: &str = "unexpand";
const PATH_UNEXPAND: &str = "/usr/bin/unexpand";
const CONTENTS_UNEXPAND: &str = r#"//! The unexpand utility converts spaces to tabs."#;
const BIN_UNIQ: &str = "uniq";
const PATH_UNIQ: &str = "/usr/bin/uniq";
const CONTENTS_UNIQ: &str =
    r#"//! The uniq command: report or filter out repeated lines in a file."#;
const BIN_UNLINK: &str = "unlink";
const PATH_UNLINK: &str = "/bin/unlink";
const CONTENTS_UNLINK: &str = "";
const BIN_WC: &str = "wc";
const PATH_WC: &str = "/usr/bin/wc";
const CONTENTS_WC: &str = r#"//! The wc command: word, line, character, and byte count."#;
const BIN_WHICH: &str = "which";
const PATH_WHICH: &str = "/usr/bin/which";
const CONTENTS_WHICH: &str = r#"//! The which command: locate a command."#;
const BIN_YES: &str = "yes";
const PATH_YES: &str = "/usr/bin/yes";
const CONTENTS_YES: &str = r#"//! The yes utility: be repetitively affirmative."#;
const BIN_MARKDOWNSP: &str = "markdownsp";
const PATH_MARKDOWNSP: &str = "/usr/bin/markdownsp";
const CONTENTS_MARKDOWNSP: &str = r#"//! The markdownsp command: a lisp REPL for markdown documents.
//!
//! This command launches an interactive S-expression REPL that allows
//! parsing, querying, and manipulating markdown documents in the current directory."#;

/// Canonical command names, their paths, and their documentation contents.
pub const BUILTIN_COMMANDS: &[(&str, &str, &str)] = &[
    (BIN_BASE64, PATH_BASE64, CONTENTS_BASE64),
    (BIN_BASENAME, PATH_BASENAME, CONTENTS_BASENAME),
    (BIN_CAT, PATH_CAT, CONTENTS_CAT),
    (BIN_COMM, PATH_COMM, CONTENTS_COMM),
    (BIN_CP, PATH_CP, CONTENTS_CP),
    (BIN_CUT, PATH_CUT, CONTENTS_CUT),
    (BIN_DATE, PATH_DATE, CONTENTS_DATE),
    (BIN_DU, PATH_DU, CONTENTS_DU),
    (BIN_ECHO, PATH_ECHO, CONTENTS_ECHO),
    (BIN_ENV, PATH_ENV, CONTENTS_ENV),
    (BIN_EXIT, PATH_EXIT, CONTENTS_EXIT),
    (BIN_EXPAND, PATH_EXPAND, CONTENTS_EXPAND),
    (BIN_FALSE, PATH_FALSE, CONTENTS_FALSE),
    (BIN_FOLD, PATH_FOLD, CONTENTS_FOLD),
    (BIN_GREP, PATH_GREP, CONTENTS_GREP),
    (BIN_EGREP, PATH_EGREP, CONTENTS_EGREP),
    (BIN_FGREP, PATH_FGREP, CONTENTS_FGREP),
    (BIN_HEAD, PATH_HEAD, CONTENTS_HEAD),
    (BIN_LN, PATH_LN, CONTENTS_LN),
    (BIN_LS, PATH_LS, CONTENTS_LS),
    (BIN_MKDIR, PATH_MKDIR, CONTENTS_MKDIR),
    (BIN_MKTEMP, PATH_MKTEMP, CONTENTS_MKTEMP),
    (BIN_MV, PATH_MV, CONTENTS_MV),
    (BIN_NL, PATH_NL, CONTENTS_NL),
    (BIN_PASTE, PATH_PASTE, CONTENTS_PASTE),
    (BIN_PRINTF, PATH_PRINTF, CONTENTS_PRINTF),
    (BIN_PWD, PATH_PWD, CONTENTS_PWD),
    (BIN_READLINK, PATH_READLINK, CONTENTS_READLINK),
    (BIN_REALPATH, PATH_REALPATH, CONTENTS_REALPATH),
    (BIN_RM, PATH_RM, CONTENTS_RM),
    (BIN_RMDIR, PATH_RMDIR, CONTENTS_RMDIR),
    (BIN_SEQ, PATH_SEQ, CONTENTS_SEQ),
    (BIN_SH, PATH_SH, CONTENTS_SH),
    (BIN_SHUF, PATH_SHUF, CONTENTS_SHUF),
    (BIN_SORT, PATH_SORT, CONTENTS_SORT),
    (BIN_SPLIT, PATH_SPLIT, CONTENTS_SPLIT),
    (BIN_STAT, PATH_STAT, CONTENTS_STAT),
    (BIN_TAIL, PATH_TAIL, CONTENTS_TAIL),
    (BIN_TEE, PATH_TEE, CONTENTS_TEE),
    (BIN_TEST, PATH_TEST, CONTENTS_TEST),
    (BIN_BRACKET, PATH_BRACKET, CONTENTS_BRACKET),
    (BIN_TOUCH, PATH_TOUCH, CONTENTS_TOUCH),
    (BIN_TR, PATH_TR, CONTENTS_TR),
    (BIN_TRUE, PATH_TRUE, CONTENTS_TRUE),
    (BIN_TRUNCATE, PATH_TRUNCATE, CONTENTS_TRUNCATE),
    (BIN_UNAME, PATH_UNAME, CONTENTS_UNAME),
    (BIN_UNEXPAND, PATH_UNEXPAND, CONTENTS_UNEXPAND),
    (BIN_UNIQ, PATH_UNIQ, CONTENTS_UNIQ),
    (BIN_UNLINK, PATH_UNLINK, CONTENTS_UNLINK),
    (BIN_WC, PATH_WC, CONTENTS_WC),
    (BIN_WHICH, PATH_WHICH, CONTENTS_WHICH),
    (BIN_YES, PATH_YES, CONTENTS_YES),
    (BIN_MARKDOWNSP, PATH_MARKDOWNSP, CONTENTS_MARKDOWNSP),
];

/// Look up a builtin binary by name.
#[allow(clippy::type_complexity)]
pub fn lookup_bin<SI, SO, SE, FS>(
    bin: &str,
) -> Result<fn(&Environment<SI, SO, SE, FS>) -> Result<ExitCode, Error>, Error>
where
    SI: StdioIn,
    SO: StdioOut,
    SE: StdioOut,
    FS: Filesystem + 'static,
{
    match bin {
        "base64" | "/usr/bin/base64" => Ok(base64::bin),
        "basename" | "/usr/bin/basename" => Ok(basename::bin),
        "cat" | "/bin/cat" => Ok(cat::bin),
        "comm" | "/usr/bin/comm" => Ok(comm::bin),
        "cp" | "/bin/cp" => Ok(cp::bin),
        "cut" | "/usr/bin/cut" => Ok(cut::bin),
        "date" | "/bin/date" => Ok(date::bin),
        "du" | "/usr/bin/du" => Ok(du::bin),
        "echo" | "/bin/echo" => Ok(echo::bin),
        "env" | "/usr/bin/env" => Ok(env::bin),
        "exit" => Ok(exit_bin),
        "expand" | "/usr/bin/expand" => Ok(expand::bin),
        "fold" | "/usr/bin/fold" => Ok(fold::bin),
        "grep" | "/usr/bin/grep" | "egrep" | "/usr/bin/egrep" | "fgrep" | "/usr/bin/fgrep" => {
            Ok(grep::bin)
        }
        "head" | "/usr/bin/head" => Ok(head::bin),
        "ln" | "/bin/ln" => Ok(ln::bin),
        "ls" | "/bin/ls" => Ok(ls::bin),
        "mkdir" | "/bin/mkdir" => Ok(mkdir::bin),
        "mktemp" | "/usr/bin/mktemp" => Ok(mktemp::bin),
        "mv" | "/bin/mv" => Ok(mv::bin),
        "nl" | "/usr/bin/nl" => Ok(nl::bin),
        "paste" | "/usr/bin/paste" => Ok(paste::bin),
        "printf" | "/usr/bin/printf" => Ok(printf::bin),
        "pwd" | "/bin/pwd" => Ok(pwd::bin),
        "readlink" | "/usr/bin/readlink" => Ok(readlink::bin),
        "realpath" | "/bin/realpath" => Ok(realpath::bin),
        "rm" | "/bin/rm" => Ok(rm::bin),
        "rmdir" | "/bin/rmdir" => Ok(rmdir::bin),
        "seq" | "/usr/bin/seq" => Ok(seq::bin),
        "shuf" | "/usr/bin/shuf" => Ok(shuf::bin),
        "sort" | "/usr/bin/sort" => Ok(sort::bin),
        "split" | "/usr/bin/split" => Ok(split::bin),
        "stat" | "/usr/bin/stat" => Ok(stat::bin),
        "tail" | "/usr/bin/tail" => Ok(tail::bin),
        "tee" | "/usr/bin/tee" => Ok(tee::bin),
        "test" | "/usr/bin/test" | "[" | "/usr/bin/[" => Ok(test::bin),
        "touch" | "/usr/bin/touch" => Ok(touch::bin),
        "tr" | "/usr/bin/tr" => Ok(tr::bin),
        "false" | "/bin/false" => Ok(false_bin),
        "sh" | "/bin/sh" => Ok(sh::bin),
        "true" | "/bin/true" => Ok(true_bin),
        "truncate" | "/usr/bin/truncate" => Ok(truncate::bin),
        "uname" | "/usr/bin/uname" => Ok(uname::bin),
        "unexpand" | "/usr/bin/unexpand" => Ok(unexpand::bin),
        "uniq" | "/usr/bin/uniq" => Ok(uniq::bin),
        "unlink" | "/bin/unlink" => Ok(rm::bin),
        "wc" | "/usr/bin/wc" => Ok(wc::bin),
        "which" | "/usr/bin/which" => Ok(which::bin),
        "yes" | "/usr/bin/yes" => Ok(yes::bin),
        "markdownsp" | "/usr/bin/markdownsp" => Ok(markdownsp::bin),
        _ => Err(Error::UnknownBinary(bin.to_string())),
    }
}

/// The true builtin: do nothing, successfully.
///
/// Returns exit code 0.
fn true_bin<SI, SO, SE, FS>(_env: &Environment<SI, SO, SE, FS>) -> Result<ExitCode, Error>
where
    SI: StdioIn,
    SO: StdioOut,
    SE: StdioOut,
    FS: Filesystem,
{
    Ok(ExitCode::from(0))
}

/// The false builtin: do nothing, unsuccessfully.
///
/// Returns exit code 1.
fn false_bin<SI, SO, SE, FS>(_env: &Environment<SI, SO, SE, FS>) -> Result<ExitCode, Error>
where
    SI: StdioIn,
    SO: StdioOut,
    SE: StdioOut,
    FS: Filesystem,
{
    Ok(ExitCode::from(1))
}

/// The exit builtin: exit the shell with an optional exit code.
///
/// Usage:
///   exit [n]
///
/// If n is omitted, the exit code is 0.
fn exit_bin<SI, SO, SE, FS>(env: &Environment<SI, SO, SE, FS>) -> Result<ExitCode, Error>
where
    SI: StdioIn,
    SO: StdioOut,
    SE: StdioOut,
    FS: Filesystem,
{
    let code = if env.args.len() > 1 {
        match env.args[1].parse::<i8>() {
            Ok(n) => n,
            Err(_) => {
                env.stderr
                    .write_line(&format!("exit: {}: numeric argument required", env.args[1]))?;
                return Ok(ExitCode::from(2));
            }
        }
    } else {
        0
    };
    env.signal_exit();
    Ok(ExitCode::from(code))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::{TestFilesystem, make_test_env};
    use crate::{StringStdioIn, StringStdioOut};

    // ========================================================================
    // exit builtin tests
    // ========================================================================

    #[test]
    fn exit_no_args_returns_zero() {
        let env = make_test_env(vec!["exit"]);
        let result = exit_bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert!(env.is_exit_signaled());
    }

    #[test]
    fn exit_zero_returns_zero() {
        let env = make_test_env(vec!["exit", "0"]);
        let result = exit_bin(&env).unwrap();
        assert_eq!(0, result.code());
        assert!(env.is_exit_signaled());
    }

    #[test]
    fn exit_one_returns_one() {
        let env = make_test_env(vec!["exit", "1"]);
        let result = exit_bin(&env).unwrap();
        assert_eq!(1, result.code());
        assert!(env.is_exit_signaled());
    }

    #[test]
    fn exit_42_returns_42() {
        let env = make_test_env(vec!["exit", "42"]);
        let result = exit_bin(&env).unwrap();
        assert_eq!(42, result.code());
        assert!(env.is_exit_signaled());
    }

    #[test]
    fn exit_negative_returns_negative() {
        let env = make_test_env(vec!["exit", "-1"]);
        let result = exit_bin(&env).unwrap();
        assert_eq!(-1, result.code());
        assert!(env.is_exit_signaled());
    }

    #[test]
    fn exit_127_returns_127() {
        let env = make_test_env(vec!["exit", "127"]);
        let result = exit_bin(&env).unwrap();
        assert_eq!(127, result.code());
        assert!(env.is_exit_signaled());
    }

    #[test]
    fn exit_non_numeric_returns_error() {
        let env = make_test_env(vec!["exit", "abc"]);
        let result = exit_bin(&env).unwrap();
        assert_eq!(2, result.code());
        assert!(!env.is_exit_signaled());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("numeric argument required"));
        assert!(stderr.contains("abc"));
    }

    #[test]
    fn exit_empty_string_returns_error() {
        let env = make_test_env(vec!["exit", ""]);
        let result = exit_bin(&env).unwrap();
        assert_eq!(2, result.code());
        assert!(!env.is_exit_signaled());
    }

    #[test]
    fn exit_overflow_returns_error() {
        let env = make_test_env(vec!["exit", "999"]);
        let result = exit_bin(&env).unwrap();
        assert_eq!(2, result.code());
        assert!(!env.is_exit_signaled());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("numeric argument required"));
    }

    #[test]
    fn exit_extra_args_ignored() {
        let env = make_test_env(vec!["exit", "5", "extra", "args"]);
        let result = exit_bin(&env).unwrap();
        assert_eq!(5, result.code());
        assert!(env.is_exit_signaled());
    }

    // ========================================================================
    // true builtin tests
    // ========================================================================

    #[test]
    fn true_returns_zero() {
        let env = make_test_env(vec!["true"]);
        let bin =
            lookup_bin::<StringStdioIn, StringStdioOut, StringStdioOut, TestFilesystem>("true")
                .unwrap();
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
    }

    #[test]
    fn true_with_args_returns_zero() {
        let env = make_test_env(vec!["true", "ignored", "arguments"]);
        let bin =
            lookup_bin::<StringStdioIn, StringStdioOut, StringStdioOut, TestFilesystem>("true")
                .unwrap();
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
    }

    #[test]
    fn true_via_path_returns_zero() {
        let env = make_test_env(vec!["/bin/true"]);
        let bin = lookup_bin::<StringStdioIn, StringStdioOut, StringStdioOut, TestFilesystem>(
            "/bin/true",
        )
        .unwrap();
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
    }

    #[test]
    fn true_produces_no_output() {
        let env = make_test_env(vec!["true"]);
        let bin =
            lookup_bin::<StringStdioIn, StringStdioOut, StringStdioOut, TestFilesystem>("true")
                .unwrap();
        let _ = bin(&env).unwrap();
        assert_eq!("", env.stdout.into_string());
        assert_eq!("", env.stderr.into_string());
    }

    // ========================================================================
    // false builtin tests
    // ========================================================================

    #[test]
    fn false_returns_one() {
        let env = make_test_env(vec!["false"]);
        let bin =
            lookup_bin::<StringStdioIn, StringStdioOut, StringStdioOut, TestFilesystem>("false")
                .unwrap();
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
    }

    #[test]
    fn false_with_args_returns_one() {
        let env = make_test_env(vec!["false", "ignored", "arguments"]);
        let bin =
            lookup_bin::<StringStdioIn, StringStdioOut, StringStdioOut, TestFilesystem>("false")
                .unwrap();
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
    }

    #[test]
    fn false_via_path_returns_one() {
        let env = make_test_env(vec!["/bin/false"]);
        let bin = lookup_bin::<StringStdioIn, StringStdioOut, StringStdioOut, TestFilesystem>(
            "/bin/false",
        )
        .unwrap();
        let result = bin(&env).unwrap();
        assert_eq!(1, result.code());
    }

    #[test]
    fn false_produces_no_output() {
        let env = make_test_env(vec!["false"]);
        let bin =
            lookup_bin::<StringStdioIn, StringStdioOut, StringStdioOut, TestFilesystem>("false")
                .unwrap();
        let _ = bin(&env).unwrap();
        assert_eq!("", env.stdout.into_string());
        assert_eq!("", env.stderr.into_string());
    }

    // ========================================================================
    // lookup_bin tests
    // ========================================================================

    #[test]
    fn lookup_exit() {
        let result =
            lookup_bin::<StringStdioIn, StringStdioOut, StringStdioOut, TestFilesystem>("exit");
        assert!(result.is_ok());
    }

    #[test]
    fn lookup_true() {
        let result =
            lookup_bin::<StringStdioIn, StringStdioOut, StringStdioOut, TestFilesystem>("true");
        assert!(result.is_ok());
    }

    #[test]
    fn lookup_false() {
        let result =
            lookup_bin::<StringStdioIn, StringStdioOut, StringStdioOut, TestFilesystem>("false");
        assert!(result.is_ok());
    }

    #[test]
    fn lookup_unknown_returns_error() {
        let result = lookup_bin::<StringStdioIn, StringStdioOut, StringStdioOut, TestFilesystem>(
            "nonexistent",
        );
        assert!(matches!(result, Err(Error::UnknownBinary(_))));
    }
}

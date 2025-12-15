//! Debug REPL for interacting with LFS filesystem images.
//!
//! This tool provides an interactive Lisp environment for exploring and manipulating
//! log-structured filesystem images. It supports both creating new filesystems and
//! opening existing ones.
//!
//! # Usage
//!
//! Create a new filesystem image:
//! ```text
//! eudaemon-debug-img --new --size 1M image.lfs
//! ```
//!
//! Open an existing filesystem image:
//! ```text
//! eudaemon-debug-img image.lfs
//! ```
//!
//! # Available Commands
//!
//! - `:help` - Show available commands
//! - `:quit` - Exit the REPL
//! - `:tree` - Show filesystem tree
//! - `:ls [path]` - List directory contents
//! - `:cat path` - Show file contents
//! - `:stat path` - Show file metadata
//! - `:usage` - Show filesystem usage statistics
//! - `:sync` - Sync filesystem to disk
//!
//! # Lisp Functions
//!
//! All `lfs-*` functions from the lisp module are available:
//! - `(lfs-read path)` - Read file contents
//! - `(lfs-write path content)` - Write to a file
//! - `(lfs-mkdir path)` - Create a directory
//! - `(lfs-remove path)` - Remove a file
//! - `(lfs-tree)` - Get filesystem tree as S-expression
//! - And many more...

use std::borrow::Cow;
use std::cell::RefCell;
use std::io::Write;
use std::rc::Rc;
use std::time::{SystemTime, UNIX_EPOCH};

use arrrg::CommandLine;
use arrrg_derive::CommandLine;
use lispdown::{Parser, SExpr, Vm};
use rustyline::completion::{Completer, Pair};
use rustyline::config::Configurer;
use rustyline::error::ReadlineError;
use rustyline::highlight::{CmdKind, Highlighter};
use rustyline::hint::Hinter;
use rustyline::validate::{ValidationContext, ValidationResult, Validator};
use rustyline::{Context, EditMode, Editor, Helper};

use eudaemonfs::lisp::LfsLisp;
use eudaemonfs::{DeviceId, FileBlockDevice, Lfs};

/// Command line arguments for eudaemon-debug-img.
#[derive(CommandLine, Debug, Default, Eq, PartialEq)]
struct Args {
    /// Create a new filesystem image instead of opening an existing one.
    #[arrrg(flag, "Create a new filesystem image")]
    new: bool,

    /// Size of the new filesystem (e.g., 1M, 64K, 1G). Only used with --new.
    #[arrrg(optional, "Size of new filesystem (e.g., 1M, 64K)")]
    size: Option<String>,

    /// Use an in-memory filesystem instead of a file.
    #[arrrg(flag, "Use in-memory filesystem (no persistence)")]
    memory: bool,

    /// Read-only mode (don't sync changes to disk).
    #[arrrg(flag, "Read-only mode")]
    read_only: bool,
}

/// Parses a size string like "1M", "64K", "1G" into bytes.
fn parse_size(s: &str) -> Result<usize, String> {
    let s = s.trim();
    if s.is_empty() {
        return Err("Empty size string".to_string());
    }

    let (num_str, suffix) = if s.ends_with(|c: char| c.is_ascii_alphabetic()) {
        let idx = s.len() - 1;
        (&s[..idx], &s[idx..])
    } else {
        (s, "")
    };

    let num: usize = num_str
        .parse()
        .map_err(|_| format!("Invalid number: {}", num_str))?;

    let multiplier = match suffix.to_uppercase().as_str() {
        "" | "B" => 1,
        "K" | "KB" => 1024,
        "M" | "MB" => 1024 * 1024,
        "G" | "GB" => 1024 * 1024 * 1024,
        _ => return Err(format!("Unknown size suffix: {}", suffix)),
    };

    Ok(num * multiplier)
}

/// Returns current time in milliseconds since UNIX epoch.
fn current_time_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Helper struct for rustyline that provides multi-line input and autocomplete.
#[derive(Default)]
struct LfsHelper {
    function_names: Vec<String>,
}

impl LfsHelper {
    fn new() -> Self {
        let names = vec![
            // LFS builtins
            "lfs-stat",
            "lfs-lstat",
            "lfs-read",
            "lfs-write",
            "lfs-mkdir",
            "lfs-mkdir-all",
            "lfs-remove",
            "lfs-rmdir",
            "lfs-readdir",
            "lfs-exists?",
            "lfs-is-dir?",
            "lfs-symlink",
            "lfs-readlink",
            "lfs-link",
            "lfs-rename",
            "lfs-append",
            "lfs-tree",
            "lfs-free-blocks",
            "lfs-total-blocks",
            "lfs-usage-percent",
            "lfs-clean",
            // Core builtins
            "null?",
            "list?",
            "atom?",
            "empty?",
            "eq?",
            "first",
            "rest",
            "cons",
            "append",
            "length",
            "nth",
            "list",
            "help",
            "quote",
            "if",
            "let",
            "begin",
            "->",
            "->>",
            "map",
            "filter",
            "reduce",
            // JSON builtins
            "obj",
            "arr",
            "get",
            "keys",
            "values",
            "assoc",
            "dissoc",
            "merge",
        ];
        LfsHelper {
            function_names: names.iter().map(|s| s.to_string()).collect(),
        }
    }
}

impl Validator for LfsHelper {
    fn validate(&self, ctx: &mut ValidationContext) -> Result<ValidationResult, ReadlineError> {
        let input = ctx.input();
        if is_balanced(input) {
            Ok(ValidationResult::Valid(None))
        } else {
            Ok(ValidationResult::Incomplete)
        }
    }
}

impl Completer for LfsHelper {
    type Candidate = Pair;

    fn complete(
        &self,
        line: &str,
        pos: usize,
        _ctx: &Context<'_>,
    ) -> Result<(usize, Vec<Pair>), ReadlineError> {
        let start = line[..pos]
            .rfind(|c: char| c.is_whitespace() || c == '(' || c == ')')
            .map(|i| i + 1)
            .unwrap_or(0);
        let prefix = &line[start..pos];

        if prefix.is_empty() {
            return Ok((pos, Vec::new()));
        }

        let matches: Vec<Pair> = self
            .function_names
            .iter()
            .filter(|name| name.starts_with(prefix))
            .map(|name| Pair {
                display: name.clone(),
                replacement: name.clone(),
            })
            .collect();

        Ok((start, matches))
    }
}

impl Hinter for LfsHelper {
    type Hint = String;

    fn hint(&self, _line: &str, _pos: usize, _ctx: &Context<'_>) -> Option<Self::Hint> {
        None
    }
}

impl Highlighter for LfsHelper {
    fn highlight<'l>(&self, line: &'l str, _pos: usize) -> Cow<'l, str> {
        Cow::Borrowed(line)
    }

    fn highlight_char(&self, _line: &str, _pos: usize, _kind: CmdKind) -> bool {
        false
    }
}

impl Helper for LfsHelper {}

/// Checks if parentheses are balanced in the input string.
fn is_balanced(input: &str) -> bool {
    let mut depth = 0i32;
    let mut in_string = false;
    let mut escape_next = false;

    for c in input.chars() {
        if escape_next {
            escape_next = false;
            continue;
        }

        match c {
            '\\' if in_string => {
                escape_next = true;
            }
            '"' => {
                in_string = !in_string;
            }
            '(' if !in_string => {
                depth += 1;
            }
            ')' if !in_string => {
                depth -= 1;
            }
            _ => {}
        }
    }

    depth <= 0 && !in_string
}

/// Trait object for type-erased LfsLisp operations.
trait LfsDebugOps {
    fn tree(&self) -> String;
    fn usage(&self) -> String;
    fn ls(&self, path: &str) -> String;
    fn cat(&self, path: &str) -> String;
    fn stat(&self, path: &str) -> String;
}

impl<D: eudaemonfs::BlockDevice, T: Fn() -> i64> LfsDebugOps for LfsLisp<D, T> {
    fn tree(&self) -> String {
        match LfsLisp::tree(self) {
            Ok(expr) => expr.to_string(),
            Err(e) => format!("Error: {}", e),
        }
    }

    fn usage(&self) -> String {
        let free = self.free_blocks();
        let total = self.total_log_blocks();
        let percent = self.usage_percent();
        format!(
            "Free blocks: {}\nTotal blocks: {}\nUsage: {}%",
            free, total, percent
        )
    }

    fn ls(&self, path: &str) -> String {
        match LfsLisp::readdir(self, path) {
            Ok(SExpr::List(entries)) => {
                let mut output = String::new();
                for entry in entries {
                    if let SExpr::List(items) = entry
                        && items.len() >= 2
                    {
                        // Extract name and type from stat
                        let name = &items[0];
                        let stat = &items[1];
                        let file_type = extract_type_from_stat(stat);
                        output.push_str(&format!("{} {}\n", file_type, name));
                    }
                }
                if output.is_empty() {
                    "(empty directory)".to_string()
                } else {
                    output.trim_end().to_string()
                }
            }
            Ok(other) => other.to_string(),
            Err(e) => format!("Error: {}", e),
        }
    }

    fn cat(&self, path: &str) -> String {
        match LfsLisp::read(self, path) {
            Ok(content) => content,
            Err(e) => format!("Error: {}", e),
        }
    }

    fn stat(&self, path: &str) -> String {
        match LfsLisp::stat(self, path) {
            Ok(expr) => format_stat(&expr),
            Err(e) => format!("Error: {}", e),
        }
    }
}

/// Extracts file type from a stat S-expression.
fn extract_type_from_stat(stat: &SExpr) -> &str {
    if let SExpr::List(items) = stat {
        for item in items {
            if let SExpr::List(pair) = item
                && pair.len() == 2
                && let SExpr::Atom(key) = &pair[0]
                && key == "type"
                && let SExpr::Atom(val) = &pair[1]
            {
                return match val.as_str() {
                    "dir" => "d",
                    "file" => "-",
                    "symlink" => "l",
                    _ => "?",
                };
            }
        }
    }
    "?"
}

/// Formats a stat S-expression for display.
fn format_stat(stat: &SExpr) -> String {
    if let SExpr::List(items) = stat {
        let mut output = String::new();
        for item in items {
            if let SExpr::List(pair) = item
                && pair.len() == 2
                && let (SExpr::Atom(key), SExpr::Atom(val)) = (&pair[0], &pair[1])
            {
                output.push_str(&format!("{}: {}\n", key, val));
            }
        }
        if output.is_empty() {
            stat.to_string()
        } else {
            output.trim_end().to_string()
        }
    } else {
        stat.to_string()
    }
}

/// Prints help message.
fn print_help() {
    println!(
        r#"Commands:
  :help, :h, :?      Show this help
  :quit, :q, :exit   Exit the REPL
  :tree              Show filesystem tree
  :ls [path]         List directory (default: /)
  :cat path          Show file contents
  :stat path         Show file metadata
  :usage             Show filesystem usage
  :sync              Sync changes to disk
  :fns               List available Lisp functions

Lisp evaluation:
  Type any S-expression to evaluate it.
  
Examples:
  (lfs-write "/hello.txt" "Hello, World!")
  (lfs-read "/hello.txt")
  (lfs-mkdir "/mydir")
  (lfs-tree)
  (lfs-exists? "/hello.txt")
  (-> "/hello.txt" lfs-stat)
"#
    );
}

/// Prints available functions.
fn print_functions() {
    println!(
        r#"LFS Functions:
  (lfs-stat path)           Get file metadata
  (lfs-lstat path)          Get metadata (no symlink follow)
  (lfs-read path)           Read file contents
  (lfs-write path content)  Write to file
  (lfs-append path content) Append to file
  (lfs-mkdir path)          Create directory
  (lfs-mkdir-all path)      Create directory with parents
  (lfs-remove path)         Remove file
  (lfs-rmdir path)          Remove empty directory
  (lfs-readdir path)        List directory
  (lfs-exists? path)        Check if path exists
  (lfs-is-dir? path)        Check if path is directory
  (lfs-symlink target link) Create symlink
  (lfs-readlink path)       Read symlink target
  (lfs-link src dst)        Create hard link
  (lfs-rename src dst)      Rename file/directory
  (lfs-tree)                Get filesystem tree
  (lfs-free-blocks)         Get free block count
  (lfs-total-blocks)        Get total block count
  (lfs-usage-percent)       Get usage percentage
  (lfs-clean)               Run garbage collection

Core Functions:
  first, rest, cons, append, length, nth, list
  null?, list?, atom?, empty?, eq?
  map, filter, reduce
  quote, if, let, begin, ->, ->>
"#
    );
}

/// Runs the REPL with the given LfsLisp instance.
fn run_repl<D: eudaemonfs::BlockDevice + 'static, T: Fn() -> i64 + 'static>(
    lfs: LfsLisp<D, T>,
    sync_fn: Option<Box<dyn Fn()>>,
    read_only: bool,
) {
    let lfs_rc = Rc::new(lfs);
    let lfs_ops: Rc<dyn LfsDebugOps> = Rc::clone(&lfs_rc) as Rc<dyn LfsDebugOps>;

    // Create LfsLispState for VM integration
    // We need to wrap this differently since we already have an Rc
    let state = LfsLispStateWrapper {
        lfs: Rc::clone(&lfs_rc),
    };

    let mut vm = Vm::new();
    vm.register_builtins();
    vm.register_json_builtins();
    state.register_builtins(&mut vm);

    let helper = LfsHelper::new();
    let mut rl: Editor<LfsHelper, rustyline::history::DefaultHistory> =
        Editor::new().expect("Failed to create readline editor");
    rl.set_helper(Some(helper));
    rl.set_edit_mode(EditMode::Vi);

    println!("eudaemon-debug-img - LFS filesystem debugger");
    if read_only {
        println!("Mode: read-only");
    }
    println!("Type :help for commands, :quit to exit\n");

    loop {
        match rl.readline("lfs> ") {
            Ok(line) => {
                let input = line.trim();
                if input.is_empty() {
                    continue;
                }

                let _ = rl.add_history_entry(&line);

                // Handle REPL commands
                if input.starts_with(':') {
                    let parts: Vec<&str> = input.split_whitespace().collect();
                    match parts[0] {
                        ":quit" | ":q" | ":exit" => break,
                        ":help" | ":h" | ":?" => print_help(),
                        ":fns" | ":functions" => print_functions(),
                        ":tree" => println!("{}", lfs_ops.tree()),
                        ":usage" => println!("{}", lfs_ops.usage()),
                        ":ls" => {
                            let path = if parts.len() > 1 { parts[1] } else { "/" };
                            println!("{}", lfs_ops.ls(path));
                        }
                        ":cat" => {
                            if parts.len() < 2 {
                                println!("Usage: :cat <path>");
                            } else {
                                println!("{}", lfs_ops.cat(parts[1]));
                            }
                        }
                        ":stat" => {
                            if parts.len() < 2 {
                                println!("Usage: :stat <path>");
                            } else {
                                println!("{}", lfs_ops.stat(parts[1]));
                            }
                        }
                        ":sync" => {
                            if read_only {
                                println!("Read-only mode - sync disabled");
                            } else if let Some(ref sync) = sync_fn {
                                sync();
                                println!("Synced to disk");
                            } else {
                                println!("No sync function available (in-memory filesystem)");
                            }
                        }
                        _ => println!("Unknown command: {}. Type :help for commands.", parts[0]),
                    }
                    continue;
                }

                // Evaluate S-expression
                let mut parser = Parser::new(input);
                match parser.parse() {
                    Ok(expr) => match vm.eval(&expr) {
                        Ok(result) => println!("{}", result),
                        Err(e) => println!("Error: {}", e),
                    },
                    Err(e) => println!("Parse error: {}", e),
                }
            }
            Err(ReadlineError::Interrupted) => {
                println!("^C");
                continue;
            }
            Err(ReadlineError::Eof) => {
                println!("^D");
                break;
            }
            Err(err) => {
                println!("Error: {:?}", err);
                break;
            }
        }
    }

    // Sync on exit if not read-only
    if !read_only && let Some(sync) = sync_fn {
        sync();
        println!("Changes synced to disk.");
    }
}

/// Wrapper to register builtins with an existing Rc<LfsLisp>.
struct LfsLispStateWrapper<D: eudaemonfs::BlockDevice, T: Fn() -> i64> {
    lfs: Rc<LfsLisp<D, T>>,
}

impl<D: eudaemonfs::BlockDevice + 'static, T: Fn() -> i64 + 'static> LfsLispStateWrapper<D, T> {
    fn register_builtins(&self, vm: &mut Vm) {
        set_thread_local_lfs_wrapper(Rc::clone(&self.lfs));

        // Now register the builtins - they'll use the thread-local
        vm.def_fn("lfs-stat", builtin_lfs_stat);
        vm.def_fn("lfs-lstat", builtin_lfs_lstat);
        vm.def_fn("lfs-read", builtin_lfs_read);
        vm.def_fn("lfs-write", builtin_lfs_write);
        vm.def_fn("lfs-mkdir", builtin_lfs_mkdir);
        vm.def_fn("lfs-mkdir-all", builtin_lfs_mkdir_all);
        vm.def_fn("lfs-remove", builtin_lfs_remove);
        vm.def_fn("lfs-rmdir", builtin_lfs_rmdir);
        vm.def_fn("lfs-readdir", builtin_lfs_readdir);
        vm.def_fn("lfs-exists?", builtin_lfs_exists);
        vm.def_fn("lfs-is-dir?", builtin_lfs_is_dir);
        vm.def_fn("lfs-symlink", builtin_lfs_symlink);
        vm.def_fn("lfs-readlink", builtin_lfs_readlink);
        vm.def_fn("lfs-link", builtin_lfs_link);
        vm.def_fn("lfs-rename", builtin_lfs_rename);
        vm.def_fn("lfs-append", builtin_lfs_append);
        vm.def_fn("lfs-tree", builtin_lfs_tree);
        vm.def_fn("lfs-free-blocks", builtin_lfs_free_blocks);
        vm.def_fn("lfs-total-blocks", builtin_lfs_total_blocks);
        vm.def_fn("lfs-usage-percent", builtin_lfs_usage_percent);
        vm.def_fn("lfs-clean", builtin_lfs_clean);
    }
}

// Thread-local storage for the LfsLisp wrapper
trait LfsDebugOpsInternal: LfsDebugOps {
    fn stat_internal(&self, path: &str) -> lispdown::SResult<SExpr>;
    fn lstat_internal(&self, path: &str) -> lispdown::SResult<SExpr>;
    fn read_internal(&self, path: &str) -> lispdown::SResult<String>;
    fn write_internal(&self, path: &str, content: &str) -> lispdown::SResult<()>;
    fn mkdir_internal(&self, path: &str) -> lispdown::SResult<()>;
    fn mkdir_all_internal(&self, path: &str) -> lispdown::SResult<()>;
    fn remove_internal(&self, path: &str) -> lispdown::SResult<()>;
    fn rmdir_internal(&self, path: &str) -> lispdown::SResult<()>;
    fn readdir_internal(&self, path: &str) -> lispdown::SResult<SExpr>;
    fn exists_internal(&self, path: &str) -> bool;
    fn is_dir_internal(&self, path: &str) -> bool;
    fn symlink_internal(&self, target: &str, linkpath: &str) -> lispdown::SResult<()>;
    fn readlink_internal(&self, path: &str) -> lispdown::SResult<String>;
    fn link_internal(&self, src: &str, dst: &str) -> lispdown::SResult<()>;
    fn rename_internal(&self, src: &str, dst: &str) -> lispdown::SResult<()>;
    fn append_internal(&self, path: &str, content: &str) -> lispdown::SResult<()>;
    fn tree_internal(&self) -> lispdown::SResult<SExpr>;
    fn free_blocks_internal(&self) -> u64;
    fn total_log_blocks_internal(&self) -> u64;
    fn usage_percent_internal(&self) -> u64;
    fn clean_internal(&self) -> lispdown::SResult<usize>;
}

impl<D: eudaemonfs::BlockDevice, T: Fn() -> i64> LfsDebugOpsInternal for LfsLisp<D, T> {
    fn stat_internal(&self, path: &str) -> lispdown::SResult<SExpr> {
        LfsLisp::stat(self, path)
    }
    fn lstat_internal(&self, path: &str) -> lispdown::SResult<SExpr> {
        LfsLisp::lstat(self, path)
    }
    fn read_internal(&self, path: &str) -> lispdown::SResult<String> {
        LfsLisp::read(self, path)
    }
    fn write_internal(&self, path: &str, content: &str) -> lispdown::SResult<()> {
        LfsLisp::write(self, path, content)
    }
    fn mkdir_internal(&self, path: &str) -> lispdown::SResult<()> {
        LfsLisp::mkdir(self, path)
    }
    fn mkdir_all_internal(&self, path: &str) -> lispdown::SResult<()> {
        LfsLisp::mkdir_all(self, path)
    }
    fn remove_internal(&self, path: &str) -> lispdown::SResult<()> {
        LfsLisp::remove(self, path)
    }
    fn rmdir_internal(&self, path: &str) -> lispdown::SResult<()> {
        LfsLisp::rmdir(self, path)
    }
    fn readdir_internal(&self, path: &str) -> lispdown::SResult<SExpr> {
        LfsLisp::readdir(self, path)
    }
    fn exists_internal(&self, path: &str) -> bool {
        LfsLisp::exists(self, path)
    }
    fn is_dir_internal(&self, path: &str) -> bool {
        LfsLisp::is_dir(self, path)
    }
    fn symlink_internal(&self, target: &str, linkpath: &str) -> lispdown::SResult<()> {
        LfsLisp::symlink(self, target, linkpath)
    }
    fn readlink_internal(&self, path: &str) -> lispdown::SResult<String> {
        LfsLisp::readlink(self, path)
    }
    fn link_internal(&self, src: &str, dst: &str) -> lispdown::SResult<()> {
        LfsLisp::link(self, src, dst)
    }
    fn rename_internal(&self, src: &str, dst: &str) -> lispdown::SResult<()> {
        LfsLisp::rename(self, src, dst)
    }
    fn append_internal(&self, path: &str, content: &str) -> lispdown::SResult<()> {
        LfsLisp::append(self, path, content)
    }
    fn tree_internal(&self) -> lispdown::SResult<SExpr> {
        LfsLisp::tree(self)
    }
    fn free_blocks_internal(&self) -> u64 {
        LfsLisp::free_blocks(self)
    }
    fn total_log_blocks_internal(&self) -> u64 {
        LfsLisp::total_log_blocks(self)
    }
    fn usage_percent_internal(&self) -> u64 {
        LfsLisp::usage_percent(self)
    }
    fn clean_internal(&self) -> lispdown::SResult<usize> {
        LfsLisp::clean(self)
    }
}

thread_local! {
    static THREAD_LFS_WRAPPER: RefCell<Option<Rc<dyn LfsDebugOpsInternal>>> = const { RefCell::new(None) };
}

fn set_thread_local_lfs_wrapper<D: eudaemonfs::BlockDevice + 'static, T: Fn() -> i64 + 'static>(
    lfs: Rc<LfsLisp<D, T>>,
) {
    THREAD_LFS_WRAPPER.with(|cell| {
        *cell.borrow_mut() = Some(lfs as Rc<dyn LfsDebugOpsInternal>);
    });
}

fn get_thread_local_lfs_wrapper() -> lispdown::SResult<Rc<dyn LfsDebugOpsInternal>> {
    THREAD_LFS_WRAPPER.with(|cell| {
        cell.borrow().clone().ok_or_else(|| {
            lispdown::SError::new("lfs")
                .with_code("no-filesystem")
                .with_message("No LFS filesystem is registered")
        })
    })
}

/// Extracts a string from an S-expression atom.
fn extract_string(expr: &SExpr) -> String {
    match expr {
        SExpr::Atom(s) => {
            if s.starts_with('"') && s.ends_with('"') && s.len() >= 2 {
                s[1..s.len() - 1].to_string()
            } else {
                s.clone()
            }
        }
        SExpr::List(_) => expr.to_string(),
    }
}

/// Creates a quoted string atom.
fn string_atom(s: &str) -> SExpr {
    SExpr::Atom(format!(
        "\"{}\"",
        s.replace('\\', "\\\\").replace('"', "\\\"")
    ))
}

// Builtin functions that use the thread-local wrapper

fn builtin_lfs_stat(_vm: &Vm, args: &[SExpr]) -> lispdown::SResult<SExpr> {
    if args.len() != 1 {
        return Err(lispdown::SError::new("lfs-stat")
            .with_code("wrong-argument-count")
            .with_message("lfs-stat requires exactly one argument: path"));
    }
    let path = extract_string(&args[0]);
    let lfs = get_thread_local_lfs_wrapper()?;
    lfs.stat_internal(&path)
}

fn builtin_lfs_lstat(_vm: &Vm, args: &[SExpr]) -> lispdown::SResult<SExpr> {
    if args.len() != 1 {
        return Err(lispdown::SError::new("lfs-lstat")
            .with_code("wrong-argument-count")
            .with_message("lfs-lstat requires exactly one argument: path"));
    }
    let path = extract_string(&args[0]);
    let lfs = get_thread_local_lfs_wrapper()?;
    lfs.lstat_internal(&path)
}

fn builtin_lfs_read(_vm: &Vm, args: &[SExpr]) -> lispdown::SResult<SExpr> {
    if args.len() != 1 {
        return Err(lispdown::SError::new("lfs-read")
            .with_code("wrong-argument-count")
            .with_message("lfs-read requires exactly one argument: path"));
    }
    let path = extract_string(&args[0]);
    let lfs = get_thread_local_lfs_wrapper()?;
    let content = lfs.read_internal(&path)?;
    Ok(string_atom(&content))
}

fn builtin_lfs_write(_vm: &Vm, args: &[SExpr]) -> lispdown::SResult<SExpr> {
    if args.len() != 2 {
        return Err(lispdown::SError::new("lfs-write")
            .with_code("wrong-argument-count")
            .with_message("lfs-write requires exactly two arguments: path and content"));
    }
    let path = extract_string(&args[0]);
    let content = extract_string(&args[1]);
    let lfs = get_thread_local_lfs_wrapper()?;
    lfs.write_internal(&path, &content)?;
    Ok(string_atom(&path))
}

fn builtin_lfs_mkdir(_vm: &Vm, args: &[SExpr]) -> lispdown::SResult<SExpr> {
    if args.len() != 1 {
        return Err(lispdown::SError::new("lfs-mkdir")
            .with_code("wrong-argument-count")
            .with_message("lfs-mkdir requires exactly one argument: path"));
    }
    let path = extract_string(&args[0]);
    let lfs = get_thread_local_lfs_wrapper()?;
    lfs.mkdir_internal(&path)?;
    Ok(string_atom(&path))
}

fn builtin_lfs_mkdir_all(_vm: &Vm, args: &[SExpr]) -> lispdown::SResult<SExpr> {
    if args.len() != 1 {
        return Err(lispdown::SError::new("lfs-mkdir-all")
            .with_code("wrong-argument-count")
            .with_message("lfs-mkdir-all requires exactly one argument: path"));
    }
    let path = extract_string(&args[0]);
    let lfs = get_thread_local_lfs_wrapper()?;
    lfs.mkdir_all_internal(&path)?;
    Ok(string_atom(&path))
}

fn builtin_lfs_remove(_vm: &Vm, args: &[SExpr]) -> lispdown::SResult<SExpr> {
    if args.len() != 1 {
        return Err(lispdown::SError::new("lfs-remove")
            .with_code("wrong-argument-count")
            .with_message("lfs-remove requires exactly one argument: path"));
    }
    let path = extract_string(&args[0]);
    let lfs = get_thread_local_lfs_wrapper()?;
    lfs.remove_internal(&path)?;
    Ok(SExpr::Atom("#t".to_string()))
}

fn builtin_lfs_rmdir(_vm: &Vm, args: &[SExpr]) -> lispdown::SResult<SExpr> {
    if args.len() != 1 {
        return Err(lispdown::SError::new("lfs-rmdir")
            .with_code("wrong-argument-count")
            .with_message("lfs-rmdir requires exactly one argument: path"));
    }
    let path = extract_string(&args[0]);
    let lfs = get_thread_local_lfs_wrapper()?;
    lfs.rmdir_internal(&path)?;
    Ok(SExpr::Atom("#t".to_string()))
}

fn builtin_lfs_readdir(_vm: &Vm, args: &[SExpr]) -> lispdown::SResult<SExpr> {
    if args.len() != 1 {
        return Err(lispdown::SError::new("lfs-readdir")
            .with_code("wrong-argument-count")
            .with_message("lfs-readdir requires exactly one argument: path"));
    }
    let path = extract_string(&args[0]);
    let lfs = get_thread_local_lfs_wrapper()?;
    lfs.readdir_internal(&path)
}

fn builtin_lfs_exists(_vm: &Vm, args: &[SExpr]) -> lispdown::SResult<SExpr> {
    if args.len() != 1 {
        return Err(lispdown::SError::new("lfs-exists?")
            .with_code("wrong-argument-count")
            .with_message("lfs-exists? requires exactly one argument: path"));
    }
    let path = extract_string(&args[0]);
    let lfs = get_thread_local_lfs_wrapper()?;
    let exists = lfs.exists_internal(&path);
    Ok(SExpr::Atom(if exists { "#t" } else { "#f" }.to_string()))
}

fn builtin_lfs_is_dir(_vm: &Vm, args: &[SExpr]) -> lispdown::SResult<SExpr> {
    if args.len() != 1 {
        return Err(lispdown::SError::new("lfs-is-dir?")
            .with_code("wrong-argument-count")
            .with_message("lfs-is-dir? requires exactly one argument: path"));
    }
    let path = extract_string(&args[0]);
    let lfs = get_thread_local_lfs_wrapper()?;
    let is_dir = lfs.is_dir_internal(&path);
    Ok(SExpr::Atom(if is_dir { "#t" } else { "#f" }.to_string()))
}

fn builtin_lfs_symlink(_vm: &Vm, args: &[SExpr]) -> lispdown::SResult<SExpr> {
    if args.len() != 2 {
        return Err(lispdown::SError::new("lfs-symlink")
            .with_code("wrong-argument-count")
            .with_message("lfs-symlink requires exactly two arguments: target and linkpath"));
    }
    let target = extract_string(&args[0]);
    let linkpath = extract_string(&args[1]);
    let lfs = get_thread_local_lfs_wrapper()?;
    lfs.symlink_internal(&target, &linkpath)?;
    Ok(string_atom(&linkpath))
}

fn builtin_lfs_readlink(_vm: &Vm, args: &[SExpr]) -> lispdown::SResult<SExpr> {
    if args.len() != 1 {
        return Err(lispdown::SError::new("lfs-readlink")
            .with_code("wrong-argument-count")
            .with_message("lfs-readlink requires exactly one argument: path"));
    }
    let path = extract_string(&args[0]);
    let lfs = get_thread_local_lfs_wrapper()?;
    let target = lfs.readlink_internal(&path)?;
    Ok(string_atom(&target))
}

fn builtin_lfs_link(_vm: &Vm, args: &[SExpr]) -> lispdown::SResult<SExpr> {
    if args.len() != 2 {
        return Err(lispdown::SError::new("lfs-link")
            .with_code("wrong-argument-count")
            .with_message("lfs-link requires exactly two arguments: src and dst"));
    }
    let src = extract_string(&args[0]);
    let dst = extract_string(&args[1]);
    let lfs = get_thread_local_lfs_wrapper()?;
    lfs.link_internal(&src, &dst)?;
    Ok(string_atom(&dst))
}

fn builtin_lfs_rename(_vm: &Vm, args: &[SExpr]) -> lispdown::SResult<SExpr> {
    if args.len() != 2 {
        return Err(lispdown::SError::new("lfs-rename")
            .with_code("wrong-argument-count")
            .with_message("lfs-rename requires exactly two arguments: src and dst"));
    }
    let src = extract_string(&args[0]);
    let dst = extract_string(&args[1]);
    let lfs = get_thread_local_lfs_wrapper()?;
    lfs.rename_internal(&src, &dst)?;
    Ok(string_atom(&dst))
}

fn builtin_lfs_append(_vm: &Vm, args: &[SExpr]) -> lispdown::SResult<SExpr> {
    if args.len() != 2 {
        return Err(lispdown::SError::new("lfs-append")
            .with_code("wrong-argument-count")
            .with_message("lfs-append requires exactly two arguments: path and content"));
    }
    let path = extract_string(&args[0]);
    let content = extract_string(&args[1]);
    let lfs = get_thread_local_lfs_wrapper()?;
    lfs.append_internal(&path, &content)?;
    Ok(string_atom(&path))
}

fn builtin_lfs_tree(_vm: &Vm, args: &[SExpr]) -> lispdown::SResult<SExpr> {
    if !args.is_empty() {
        return Err(lispdown::SError::new("lfs-tree")
            .with_code("wrong-argument-count")
            .with_message("lfs-tree takes no arguments"));
    }
    let lfs = get_thread_local_lfs_wrapper()?;
    lfs.tree_internal()
}

fn builtin_lfs_free_blocks(_vm: &Vm, args: &[SExpr]) -> lispdown::SResult<SExpr> {
    if !args.is_empty() {
        return Err(lispdown::SError::new("lfs-free-blocks")
            .with_code("wrong-argument-count")
            .with_message("lfs-free-blocks takes no arguments"));
    }
    let lfs = get_thread_local_lfs_wrapper()?;
    Ok(SExpr::Atom(lfs.free_blocks_internal().to_string()))
}

fn builtin_lfs_total_blocks(_vm: &Vm, args: &[SExpr]) -> lispdown::SResult<SExpr> {
    if !args.is_empty() {
        return Err(lispdown::SError::new("lfs-total-blocks")
            .with_code("wrong-argument-count")
            .with_message("lfs-total-blocks takes no arguments"));
    }
    let lfs = get_thread_local_lfs_wrapper()?;
    Ok(SExpr::Atom(lfs.total_log_blocks_internal().to_string()))
}

fn builtin_lfs_usage_percent(_vm: &Vm, args: &[SExpr]) -> lispdown::SResult<SExpr> {
    if !args.is_empty() {
        return Err(lispdown::SError::new("lfs-usage-percent")
            .with_code("wrong-argument-count")
            .with_message("lfs-usage-percent takes no arguments"));
    }
    let lfs = get_thread_local_lfs_wrapper()?;
    Ok(SExpr::Atom(lfs.usage_percent_internal().to_string()))
}

fn builtin_lfs_clean(_vm: &Vm, args: &[SExpr]) -> lispdown::SResult<SExpr> {
    if !args.is_empty() {
        return Err(lispdown::SError::new("lfs-clean")
            .with_code("wrong-argument-count")
            .with_message("lfs-clean takes no arguments"));
    }
    let lfs = get_thread_local_lfs_wrapper()?;
    let reclaimed = lfs.clean_internal()?;
    Ok(SExpr::Atom(reclaimed.to_string()))
}

fn main() {
    let (args, free) =
        Args::from_command_line_relaxed("eudaemon-debug-img: debug LFS filesystem images");

    // Check for rustyline dependency
    if free.is_empty() && !args.memory {
        eprintln!("Usage: eudaemon-debug-img [OPTIONS] <image-file>");
        eprintln!("       eudaemon-debug-img --memory [--size SIZE]");
        eprintln!();
        eprintln!("Options:");
        eprintln!("  --new           Create a new filesystem image");
        eprintln!("  --size SIZE     Size of new filesystem (e.g., 1M, 64K)");
        eprintln!("  --memory        Use in-memory filesystem (no persistence)");
        eprintln!("  --read-only     Read-only mode");
        std::process::exit(1);
    }

    if args.memory {
        // In-memory filesystem
        let size = args
            .size
            .as_deref()
            .map(|s| {
                parse_size(s).unwrap_or_else(|e| {
                    eprintln!("Error parsing size: {}", e);
                    std::process::exit(1);
                })
            })
            .unwrap_or(1024 * 1024); // Default 1MB

        let data = vec![0u8; size];
        let lfs = match LfsLisp::from_vec(data, DeviceId::new(1), current_time_ms) {
            Ok(lfs) => lfs,
            Err(e) => {
                eprintln!("Error creating filesystem: {:?}", e);
                std::process::exit(1);
            }
        };

        println!("Created in-memory filesystem ({} bytes)", size);
        run_repl(lfs, None, args.read_only);
    } else {
        // File-backed filesystem
        let path = &free[0];

        if args.new {
            // Create new filesystem
            let size = args
                .size
                .as_deref()
                .map(|s| {
                    parse_size(s).unwrap_or_else(|e| {
                        eprintln!("Error parsing size: {}", e);
                        std::process::exit(1);
                    })
                })
                .unwrap_or(1024 * 1024); // Default 1MB

            let total_blocks = (size / 4096) as u64;
            if total_blocks < 16 {
                eprintln!("Error: Filesystem too small (minimum 64KB)");
                std::process::exit(1);
            }

            let device = match FileBlockDevice::create(path, total_blocks) {
                Ok(dev) => dev,
                Err(e) => {
                    eprintln!("Error creating file {}: {}", path, e);
                    std::process::exit(1);
                }
            };

            let lfs = match Lfs::new(device, total_blocks, DeviceId::new(1), current_time_ms) {
                Ok(lfs) => lfs,
                Err(e) => {
                    eprintln!("Error creating filesystem: {:?}", e);
                    std::process::exit(1);
                }
            };

            let lfs_lisp = LfsLisp::new(lfs);
            println!("Created new filesystem: {} ({} blocks)", path, total_blocks);

            // Sync function for file-backed filesystem
            let sync_fn: Box<dyn Fn()> = Box::new(|| {
                // The filesystem is synced through the FileBlockDevice
                let _ = std::io::stdout().flush();
            });

            run_repl(lfs_lisp, Some(sync_fn), args.read_only);
        } else {
            // Open existing filesystem
            let device = match FileBlockDevice::open(path) {
                Ok(dev) => dev,
                Err(e) => {
                    eprintln!("Error opening file {}: {}", path, e);
                    std::process::exit(1);
                }
            };

            let total_blocks = device.total_blocks();
            let lfs = match Lfs::open(device, total_blocks, DeviceId::new(1), current_time_ms) {
                Ok(lfs) => lfs,
                Err(e) => {
                    eprintln!("Error opening filesystem: {:?}", e);
                    std::process::exit(1);
                }
            };

            let lfs_lisp = LfsLisp::new(lfs);
            println!("Opened filesystem: {} ({} blocks)", path, total_blocks);

            // Sync function
            let sync_fn: Box<dyn Fn()> = Box::new(|| {
                let _ = std::io::stdout().flush();
            });

            run_repl(lfs_lisp, Some(sync_fn), args.read_only);
        }
    }
}

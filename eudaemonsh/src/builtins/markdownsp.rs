//! The markdownsp builtin: a lisp REPL for markdown documents.
//!
//! This builtin launches an interactive S-expression REPL that allows
//! parsing, querying, and manipulating markdown documents in the current directory.

#![allow(dead_code)]

use std::path::Path;

use getopts::Options;

use lispdown::{Parser, SError, SExpr, SResult, Vm, register_markdown_builtins};

use crate::FileType;
use crate::Filesystem as EuFilesystem;
use crate::{Environment, Error, ExitCode, FsError, Stderr, Stdin, Stdout, resolve_path};

fn build_options() -> Options {
    let mut opts = Options::new();
    opts.optflag("h", "help", "Print help information");
    opts.optopt("e", "eval", "Evaluate a single expression and exit", "EXPR");
    opts.optopt("d", "dir", "Root directory for markdown files", "DIR");
    opts
}

fn print_usage<W: Stderr>(out: &W) -> Result<(), Error> {
    out.write_line("Usage: markdownsp [-d DIR] [-e EXPR]")?;
    out.write_line("")?;
    out.write_line("Options:")?;
    out.write_line("  -h, --help    Print help information")?;
    out.write_line("  -e, --eval    Evaluate a single expression and exit")?;
    out.write_line("  -d, --dir     Root directory for markdown files (default: cwd)")?;
    out.write_line("")?;
    out.write_line("REPL Commands:")?;
    out.write_line("  :help         Show help")?;
    out.write_line("  :quit         Exit the REPL")?;
    out.write_line("  :ls           List markdown files")?;
    out.write_line("  :show FILE    Show document as s-expression")?;
    out.write_line("  :md FILE      Show document as markdown")?;
    out.write_line("  :annotate FILE  Show document with path IDs")?;
    out.write_line("  :pwd          Print working directory")?;
    out.write_line("")?;
    out.write_line("Markdown Functions:")?;
    out.write_line("  (load \"file.md\")              Load and parse markdown file")?;
    out.write_line("  (save \"file.md\" doc)          Save document to file")?;
    out.write_line("  (markdown-to-sexpr str)        Parse markdown string")?;
    out.write_line("  (sexpr-to-markdown doc)        Convert to markdown")?;
    out.write_line("  (get-by-path doc \"1.2\")       Get node by path")?;
    out.write_line("  (prune doc \"1.2\")             Remove node at path")?;
    out.write_line("  (replace-at doc \"1\" node)     Replace node")?;
    out.write_line("  (generate-toc doc)             Generate table of contents")?;
    out.write_line("  (annotate doc)                 Add path IDs to all nodes")?;
    Ok(())
}

/// The markdownsp builtin: a lisp REPL for markdown documents.
pub fn bin<SI, SO, SE, FS>(env: &Environment<SI, SO, SE, FS>) -> Result<ExitCode, Error>
where
    SI: Stdin,
    SO: Stdout,
    SE: Stderr,
    FS: EuFilesystem + Clone + Send + Sync + 'static,
{
    let opts_def = build_options();

    let matches = match opts_def.parse(&env.args[1..]) {
        Ok(m) => m,
        Err(e) => {
            env.stderr.write_line(&format!("markdownsp: {}", e))?;
            return Ok(ExitCode::from(1));
        }
    };

    if matches.opt_present("h") {
        print_usage(&env.stderr)?;
        return Ok(ExitCode::from(0));
    }

    // Determine working directory
    let working_dir = if let Some(dir) = matches.opt_str("d") {
        resolve_path(env.cwd.as_str(), &dir)
    } else {
        env.cwd.as_str().to_string()
    };

    // Create the filesystem adapter
    let adapter = FilesystemAdapter::new(env.fs.dup(), working_dir.clone());

    // If -e is provided, evaluate and exit
    if let Some(expr_str) = matches.opt_str("e") {
        let mut vm = create_vm(&adapter);

        match evaluate_expr(&mut vm, &expr_str) {
            Ok(result) => {
                env.stdout.write_line(&result.to_string())?;
                return Ok(ExitCode::from(0));
            }
            Err(e) => {
                env.stderr.write_line(&format!("markdownsp: {}", e))?;
                return Ok(ExitCode::from(1));
            }
        }
    }

    // Interactive REPL
    env.stdout
        .write_line("markdownsp - Lisp REPL for Markdown")?;
    env.stdout
        .write_line(&format!("Working directory: {}", working_dir))?;
    env.stdout
        .write_line("Type :help for commands, :quit to exit")?;
    env.stdout.write_line("")?;

    loop {
        env.stdout.write_str("λ> ")?;

        let line = match env.stdin.read_line()? {
            Some(l) => l,
            None => break,
        };

        let input = line.trim();
        if input.is_empty() {
            continue;
        }

        // Handle REPL commands
        if input.starts_with(':') {
            match handle_command(env, &adapter, input) {
                Ok(true) => continue,
                Ok(false) => break,
                Err(e) => {
                    env.stderr.write_line(&format!("Error: {:?}", e))?;
                    continue;
                }
            }
        }

        // Create fresh VM for each evaluation
        let mut vm = create_vm(&adapter);

        match evaluate_expr(&mut vm, input) {
            Ok(result) => {
                env.stdout.write_line(&result.to_string())?;
            }
            Err(e) => {
                env.stderr.write_line(&format!("Error: {}", e))?;
            }
        }
    }

    Ok(ExitCode::from(0))
}

/// Create a VM with all markdown and filesystem builtins registered.
fn create_vm<FS: EuFilesystem + Send + Sync + 'static>(adapter: &FilesystemAdapter<FS>) -> Vm {
    let mut vm = Vm::new();
    vm.register_builtins();
    vm.register_json_builtins();
    register_markdown_builtins(&mut vm);

    // Set up thread-local storage for our builtins
    set_thread_local_adapter(Box::new(ThreadLocalAdapter {
        fs: Box::new(adapter.fs.dup()),
        cwd: adapter.cwd.clone(),
    }));

    // Register our filesystem-backed builtins
    vm.def_fn("load", builtin_load);
    vm.def_fn("save", builtin_save);
    vm.def_fn("list-files", builtin_list_files);
    vm.def_fn("read-file", builtin_read_file);
    vm.def_fn("write-file", builtin_write_file);
    vm.def_fn("file-exists?", builtin_file_exists);

    vm
}

/// Evaluate an S-expression string and return the result.
fn evaluate_expr(vm: &mut Vm, input: &str) -> Result<SExpr, String> {
    let mut parser = Parser::new(input);
    let expr = parser.parse().map_err(|e| e.to_string())?;
    vm.eval(&expr).map_err(|e| e.to_string())
}

/// Adapter from eudaemonsh Filesystem to lispdown Filesystem.
#[derive(Clone)]
struct FilesystemAdapter<FS: EuFilesystem> {
    fs: FS,
    cwd: String,
}

impl<FS: EuFilesystem> FilesystemAdapter<FS> {
    fn new(fs: FS, cwd: String) -> Self {
        Self { fs, cwd }
    }

    fn resolve(&self, path: &str) -> String {
        resolve_path(&self.cwd, path)
    }
}

impl<FS: EuFilesystem + Send + Sync> lispdown::Filesystem for FilesystemAdapter<FS> {
    fn root(&self) -> &Path {
        Path::new(&self.cwd)
    }

    fn list_markdown_files(&self) -> SResult<Vec<String>> {
        let mut files = Vec::new();
        find_markdown_recursive(&self.fs, &self.cwd, &self.cwd, &mut files)?;
        files.sort();
        Ok(files)
    }

    fn read(&self, path: &str) -> SResult<String> {
        let resolved = self.resolve(path);
        self.fs.read_to_string(&resolved).map_err(|e| {
            SError::new("read")
                .with_code("io-error")
                .with_message(&format!("{:?}", e))
        })
    }

    fn write(&self, path: &str, content: &str) -> SResult<()> {
        let resolved = self.resolve(path);
        self.fs.write_string(&resolved, content).map_err(|e| {
            SError::new("write")
                .with_code("io-error")
                .with_message(&format!("{:?}", e))
        })
    }

    fn exists(&self, path: &str) -> bool {
        let resolved = self.resolve(path);
        self.fs.exists(&resolved)
    }
}

fn find_markdown_recursive<FS: EuFilesystem>(
    fs: &FS,
    base: &str,
    path: &str,
    results: &mut Vec<String>,
) -> SResult<()> {
    let entries = fs.read_dir(path).map_err(|e| {
        SError::new("list")
            .with_code("io-error")
            .with_message(&format!("{:?}", e))
    })?;

    for (name, entry) in entries {
        if name == "." || name == ".." {
            continue;
        }
        let full_path = format!("{}/{}", path.trim_end_matches('/'), name);

        if entry.file_type == FileType::Directory {
            find_markdown_recursive(fs, base, &full_path, results)?;
        } else if name.ends_with(".md") || name.ends_with(".MD") {
            let rel_path = full_path
                .strip_prefix(base)
                .unwrap_or(&full_path)
                .trim_start_matches('/');
            results.push(rel_path.to_string());
        }
    }
    Ok(())
}

/// Handle a REPL command (lines starting with ':').
/// Returns Ok(true) to continue, Ok(false) to quit.
fn handle_command<SI, SO, SE, FS>(
    env: &Environment<SI, SO, SE, FS>,
    adapter: &FilesystemAdapter<FS>,
    input: &str,
) -> Result<bool, Error>
where
    SI: Stdin,
    SO: Stdout,
    SE: Stderr,
    FS: EuFilesystem,
{
    let parts: Vec<&str> = input.split_whitespace().collect();
    if parts.is_empty() {
        return Ok(true);
    }

    match parts[0] {
        ":quit" | ":q" | ":exit" => return Ok(false),

        ":help" | ":h" | ":?" => {
            print_usage(&env.stderr)?;
        }

        ":ls" | ":list" => {
            let mut files = Vec::new();
            find_markdown_recursive(&env.fs, &adapter.cwd, &adapter.cwd, &mut files)
                .map_err(|e| FsError::Io(std::io::Error::other(e.to_string())))?;
            if files.is_empty() {
                env.stdout.write_line("No markdown files in directory")?;
            } else {
                for f in files {
                    env.stdout.write_line(&format!("  {}", f))?;
                }
            }
        }

        ":show" => {
            if parts.len() < 2 {
                env.stderr.write_line("Usage: :show <filename>")?;
                return Ok(true);
            }
            let filename = parts[1];
            let path = adapter.resolve(filename);
            match env.fs.read_to_string(&path) {
                Ok(content) => match lispdown::markdown_to_sexpr(&content) {
                    Ok(doc) => {
                        env.stdout.write_line(&doc.to_string())?;
                    }
                    Err(e) => {
                        env.stderr.write_line(&format!("Parse error: {}", e))?;
                    }
                },
                Err(e) => {
                    env.stderr
                        .write_line(&format!("Failed to read {}: {:?}", filename, e))?;
                }
            }
        }

        ":md" => {
            if parts.len() < 2 {
                env.stderr.write_line("Usage: :md <filename>")?;
                return Ok(true);
            }
            let filename = parts[1];
            let path = adapter.resolve(filename);
            match env.fs.read_to_string(&path) {
                Ok(content) => {
                    env.stdout.write_line(&content)?;
                }
                Err(e) => {
                    env.stderr
                        .write_line(&format!("Failed to read {}: {:?}", filename, e))?;
                }
            }
        }

        ":annotate" | ":ann" => {
            if parts.len() < 2 {
                env.stderr.write_line("Usage: :annotate <filename>")?;
                return Ok(true);
            }
            let filename = parts[1];
            let path = adapter.resolve(filename);
            match env.fs.read_to_string(&path) {
                Ok(content) => match lispdown::markdown_to_sexpr(&content) {
                    Ok(doc) => {
                        let annotated = lispdown::to_annotated_sexpr(&doc);
                        env.stdout.write_line(&annotated.to_string())?;
                    }
                    Err(e) => {
                        env.stderr.write_line(&format!("Parse error: {}", e))?;
                    }
                },
                Err(e) => {
                    env.stderr
                        .write_line(&format!("Failed to read {}: {:?}", filename, e))?;
                }
            }
        }

        ":pwd" => {
            env.stdout.write_line(&adapter.cwd)?;
        }

        _ => {
            env.stderr.write_line(&format!(
                "Unknown command: {}. Type :help for commands.",
                parts[0]
            ))?;
        }
    }

    Ok(true)
}

// ============================================================================
// Thread-local storage for filesystem access from builtins
// ============================================================================

thread_local! {
    static THREAD_ADAPTER: std::cell::RefCell<Option<Box<ThreadLocalAdapter>>> =
        const { std::cell::RefCell::new(None) };
}

/// Type-erased filesystem adapter for thread-local storage.
struct ThreadLocalAdapter {
    fs: Box<dyn ThreadLocalFs>,
    cwd: String,
}

/// Trait for type-erased filesystem operations.
trait ThreadLocalFs: Send {
    fn read_to_string(&self, path: &str) -> Result<String, Error>;
    fn write_string(&self, path: &str, contents: &str) -> Result<(), Error>;
    fn exists(&self, path: &str) -> bool;
    fn read_dir(&self, path: &str) -> Result<Vec<(String, crate::DirEntry)>, Error>;
}

impl<FS: EuFilesystem + Send> ThreadLocalFs for FS {
    fn read_to_string(&self, path: &str) -> Result<String, Error> {
        Ok(EuFilesystem::read_to_string(self, path)?)
    }

    fn write_string(&self, path: &str, contents: &str) -> Result<(), Error> {
        Ok(EuFilesystem::write_string(self, path, contents)?)
    }

    fn exists(&self, path: &str) -> bool {
        EuFilesystem::exists(self, path)
    }

    fn read_dir(&self, path: &str) -> Result<Vec<(String, crate::DirEntry)>, Error> {
        Ok(EuFilesystem::read_dir(self, path)?)
    }
}

fn set_thread_local_adapter(adapter: Box<ThreadLocalAdapter>) {
    THREAD_ADAPTER.with(|cell| {
        *cell.borrow_mut() = Some(adapter);
    });
}

fn with_adapter<F, R>(f: F) -> SResult<R>
where
    F: FnOnce(&ThreadLocalAdapter) -> SResult<R>,
{
    THREAD_ADAPTER.with(|cell| {
        let borrow = cell.borrow();
        match &*borrow {
            Some(adapter) => f(adapter),
            None => Err(SError::new("markdownsp")
                .with_code("no-filesystem")
                .with_message("No filesystem adapter registered")),
        }
    })
}

/// Extract a string from an S-expression, handling both quoted strings and atoms.
fn extract_string(expr: &SExpr) -> String {
    match expr {
        SExpr::Atom(s) => {
            if s.starts_with('"') && s.ends_with('"') && s.len() >= 2 {
                s[1..s.len() - 1].to_string()
            } else {
                s.clone()
            }
        }
        SExpr::List(_) => String::new(),
    }
}

// ============================================================================
// Builtin functions
// ============================================================================

fn builtin_load(_vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
    if args.is_empty() {
        return Err(SError::new("load")
            .with_code("missing-argument")
            .with_message("load requires a filename argument"));
    }

    with_adapter(|adapter| {
        let filename = extract_string(&args[0]);
        let path = resolve_path(&adapter.cwd, &filename);

        let content = adapter.fs.read_to_string(&path).map_err(|e| {
            SError::new("load")
                .with_code("io-error")
                .with_message(&format!("Failed to read {}: {:?}", filename, e))
        })?;

        lispdown::markdown_to_sexpr(&content)
    })
}

fn builtin_save(_vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
    if args.len() < 2 {
        return Err(SError::new("save")
            .with_code("missing-argument")
            .with_message("save requires filename and document arguments"));
    }

    with_adapter(|adapter| {
        let filename = extract_string(&args[0]);
        let doc = &args[1];
        let path = resolve_path(&adapter.cwd, &filename);

        let markdown = lispdown::sexpr_to_markdown(doc)?;
        adapter.fs.write_string(&path, &markdown).map_err(|e| {
            SError::new("save")
                .with_code("io-error")
                .with_message(&format!("Failed to write {}: {:?}", filename, e))
        })?;

        Ok(SExpr::Atom("t".to_string()))
    })
}

fn builtin_list_files(_vm: &Vm, _args: &[SExpr]) -> SResult<SExpr> {
    with_adapter(|adapter| {
        let mut files = Vec::new();
        find_markdown_files_tl(&*adapter.fs, &adapter.cwd, &adapter.cwd, &mut files)?;
        let items: Vec<SExpr> = files
            .into_iter()
            .map(|f| SExpr::Atom(format!("\"{}\"", f)))
            .collect();
        Ok(SExpr::List(items))
    })
}

fn find_markdown_files_tl(
    fs: &dyn ThreadLocalFs,
    base: &str,
    path: &str,
    results: &mut Vec<String>,
) -> SResult<()> {
    let entries = fs.read_dir(path).map_err(|e| {
        SError::new("list-files")
            .with_code("io-error")
            .with_message(&format!("{:?}", e))
    })?;

    for (name, entry) in entries {
        if name == "." || name == ".." {
            continue;
        }
        let full_path = format!("{}/{}", path.trim_end_matches('/'), name);

        if entry.file_type == FileType::Directory {
            find_markdown_files_tl(fs, base, &full_path, results)?;
        } else if name.ends_with(".md") || name.ends_with(".MD") {
            let rel_path = full_path
                .strip_prefix(base)
                .unwrap_or(&full_path)
                .trim_start_matches('/');
            results.push(rel_path.to_string());
        }
    }
    Ok(())
}

fn builtin_read_file(_vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
    if args.is_empty() {
        return Err(SError::new("read-file")
            .with_code("missing-argument")
            .with_message("read-file requires a filename argument"));
    }

    with_adapter(|adapter| {
        let filename = extract_string(&args[0]);
        let path = resolve_path(&adapter.cwd, &filename);

        let content = adapter.fs.read_to_string(&path).map_err(|e| {
            SError::new("read-file")
                .with_code("io-error")
                .with_message(&format!("Failed to read {}: {:?}", filename, e))
        })?;

        let escaped = content
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('\n', "\\n")
            .replace('\r', "\\r")
            .replace('\t', "\\t");
        Ok(SExpr::Atom(format!("\"{}\"", escaped)))
    })
}

fn builtin_write_file(_vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
    if args.len() < 2 {
        return Err(SError::new("write-file")
            .with_code("missing-argument")
            .with_message("write-file requires filename and content arguments"));
    }

    with_adapter(|adapter| {
        let filename = extract_string(&args[0]);
        let content = extract_string(&args[1]);
        let path = resolve_path(&adapter.cwd, &filename);

        adapter.fs.write_string(&path, &content).map_err(|e| {
            SError::new("write-file")
                .with_code("io-error")
                .with_message(&format!("Failed to write {}: {:?}", filename, e))
        })?;

        Ok(SExpr::Atom("t".to_string()))
    })
}

fn builtin_file_exists(_vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
    if args.is_empty() {
        return Err(SError::new("file-exists?")
            .with_code("missing-argument")
            .with_message("file-exists? requires a filename argument"));
    }

    with_adapter(|adapter| {
        let filename = extract_string(&args[0]);
        let path = resolve_path(&adapter.cwd, &filename);

        Ok(SExpr::Atom(
            if adapter.fs.exists(&path) { "t" } else { "nil" }.to_string(),
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::make_test_env;

    #[test]
    fn help_flag() {
        let env = make_test_env(vec!["markdownsp", "-h"]);
        env.fs.add_directory("/");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("Usage:"));
        assert!(stderr.contains("load"));
    }

    #[test]
    fn eval_markdown_to_sexpr() {
        let env = make_test_env(vec!["markdownsp", "-e", "(markdown-to-sexpr \"# Hello\")"]);
        env.fs.add_directory("/");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert!(stdout.contains("doc"));
        assert!(stdout.contains("h1"));
    }

    #[test]
    fn eval_load_file() {
        let env = make_test_env(vec!["markdownsp", "-e", "(load \"test.md\")"]);
        env.fs.add_directory("/");
        env.fs.add_file("/test.md", "# Hello World\n\nSome text.");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert!(stdout.contains("doc"));
        assert!(stdout.contains("h1"));
        assert!(stdout.contains("Hello World"));
    }

    #[test]
    fn eval_list_files() {
        let env = make_test_env(vec!["markdownsp", "-e", "(list-files)"]);
        env.fs.add_directory("/");
        env.fs.add_file("/readme.md", "# Readme");
        env.fs.add_file("/other.txt", "not markdown");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert!(stdout.contains("readme.md"));
        assert!(!stdout.contains("other.txt"));
    }

    #[test]
    fn eval_generate_toc() {
        let env = make_test_env(vec![
            "markdownsp",
            "-e",
            "(generate-toc (markdown-to-sexpr \"# One\\n\\n## Two\\n\\n### Three\"))",
        ]);
        env.fs.add_directory("/");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert!(stdout.contains("ul") || stdout.contains("li"));
    }

    #[test]
    fn eval_file_exists() {
        let env = make_test_env(vec!["markdownsp", "-e", "(file-exists? \"test.md\")"]);
        env.fs.add_directory("/");
        env.fs.add_file("/test.md", "# Hello");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert!(stdout.contains("t"));
    }

    #[test]
    fn eval_file_not_exists() {
        let env = make_test_env(vec![
            "markdownsp",
            "-e",
            "(file-exists? \"nonexistent.md\")",
        ]);
        env.fs.add_directory("/");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert!(stdout.contains("nil"));
    }

    #[test]
    fn eval_save_file() {
        let env = make_test_env(vec![
            "markdownsp",
            "-e",
            "(save \"output.md\" (markdown-to-sexpr \"# New Doc\"))",
        ]);
        env.fs.add_directory("/");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert!(stdout.contains("t"));

        // Verify file was written
        let content = EuFilesystem::read_to_string(&env.fs, "/output.md").unwrap();
        println!("content: {:?}", content);
        assert!(content.contains("# New Doc"));
    }

    #[test]
    fn eval_prune() {
        let env = make_test_env(vec![
            "markdownsp",
            "-e",
            "(prune (markdown-to-sexpr \"# Title\\n\\nParagraph\") \"2\")",
        ]);
        env.fs.add_directory("/");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        // After pruning paragraph at path "2", should only have h1
        assert!(stdout.contains("h1"));
    }

    #[test]
    fn custom_directory() {
        let env = make_test_env(vec!["markdownsp", "-d", "/docs", "-e", "(list-files)"]);
        env.fs.add_directory("/");
        env.fs.add_directory("/docs");
        env.fs.add_file("/docs/guide.md", "# Guide");
        env.fs.add_file("/readme.md", "# Readme");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stdout = env.stdout.into_string();
        println!("stdout: {:?}", stdout);
        assert!(stdout.contains("guide.md"));
        assert!(!stdout.contains("readme.md"));
    }
}

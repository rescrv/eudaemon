//! The markdownsp builtin: a lisp REPL for markdown documents.
//!
//! This builtin launches an interactive S-expression REPL that allows
//! parsing, querying, and manipulating markdown documents in the current directory.

use getopts::Options;
use utf8path::Path;

use lispdown::{Parser, Vm, register_markdown_builtins};

use crate::FileType;
use crate::Filesystem as EuFilesystem;

use crate::{DirEntry, Environment, Error, ExitCode, FsError, Stderr, Stdin, Stdout, resolve_path};

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
    out.write_line("  (save doc \"file.md\")          Save document to file")?;
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
    FS: EuFilesystem + 'static,
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

    // Create the filesystem adapter with path resolution
    let adapter = FilesystemAdapter::new(env.fs.dup(), working_dir.clone());

    // If -e is provided, evaluate and exit
    if let Some(expr_str) = matches.opt_str("e") {
        let mut vm = create_vm(adapter);

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

    // For interactive mode, we need a fresh adapter for each command
    // since handle_command needs to borrow the adapter
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
            match handle_command(env, &working_dir, input) {
                Ok(true) => continue,
                Ok(false) => break,
                Err(e) => {
                    env.stderr.write_line(&format!("Error: {:?}", e))?;
                    continue;
                }
            }
        }

        // Create fresh VM and adapter for each evaluation
        let adapter = FilesystemAdapter::new(env.fs.dup(), working_dir.clone());
        let mut vm = create_vm(adapter);

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
fn create_vm<FS: EuFilesystem + 'static>(adapter: FilesystemAdapter<FS>) -> Vm {
    let mut vm = Vm::new();
    vm.register_builtins();
    vm.register_json_builtins();
    register_markdown_builtins(&mut vm);

    // Set the filesystem on the VM - lispdown's filesystem builtins will use this
    vm.set_filesystem(Box::new(adapter));
    vm.register_filesystem_builtins();

    vm
}

/// Evaluate an S-expression string and return the result.
fn evaluate_expr(vm: &mut Vm, input: &str) -> Result<lispdown::SExpr, String> {
    let mut parser = Parser::new(input);
    let expr = parser.parse().map_err(|e| e.to_string())?;
    vm.eval(&expr).map_err(|e| e.to_string())
}

/// Adapter that wraps an EuFilesystem and resolves paths relative to a working directory.
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

impl<FS: EuFilesystem> lispdown::Filesystem for FilesystemAdapter<FS> {
    fn dup(&self) -> Self {
        Self {
            fs: self.fs.dup(),
            cwd: self.cwd.clone(),
        }
    }

    fn root(&self) -> Path<'_> {
        Path::new(&self.cwd)
    }

    fn list_markdown_files(&self) -> Result<Vec<String>, eudaemonty::Error> {
        let mut files = Vec::new();
        find_markdown_recursive(&self.fs, &self.cwd, &self.cwd, &mut files)?;
        files.sort();
        Ok(files)
    }

    fn read_to_string(&self, path: &str) -> Result<String, eudaemonty::Error> {
        let resolved = self.resolve(path);
        self.fs.read_to_string(&resolved)
    }

    fn exists(&self, path: &str) -> bool {
        let resolved = self.resolve(path);
        self.fs.exists(&resolved)
    }

    fn metadata(&self, path: &str) -> Result<eudaemonty::FileMetadata, eudaemonty::Error> {
        let resolved = self.resolve(path);
        self.fs.metadata(&resolved)
    }

    fn truncate(&self, path: &str, size: u64) -> Result<(), eudaemonty::Error> {
        let resolved = self.resolve(path);
        self.fs.truncate(&resolved, size)
    }

    fn truncate_existing(&self, path: &str, size: u64) -> Result<bool, eudaemonty::Error> {
        let resolved = self.resolve(path);
        self.fs.truncate_existing(&resolved, size)
    }

    fn punch_hole(&self, path: &str, offset: u64, length: u64) -> Result<(), eudaemonty::Error> {
        let resolved = self.resolve(path);
        self.fs.punch_hole(&resolved, offset, length)
    }

    fn write_string(&self, path: &str, contents: &str) -> Result<(), eudaemonty::Error> {
        let resolved = self.resolve(path);
        self.fs.write_string(&resolved, contents)
    }

    fn append_string(&self, path: &str, contents: &str) -> Result<(), eudaemonty::Error> {
        let resolved = self.resolve(path);
        self.fs.append_string(&resolved, contents)
    }

    fn mkdir(&self, path: &str) -> Result<(), eudaemonty::Error> {
        let resolved = self.resolve(path);
        self.fs.mkdir(&resolved)
    }

    fn mkdir_all(&self, path: &str) -> Result<(), eudaemonty::Error> {
        let resolved = self.resolve(path);
        self.fs.mkdir_all(&resolved)
    }

    fn is_dir(&self, path: &str) -> bool {
        let resolved = self.resolve(path);
        self.fs.is_dir(&resolved)
    }

    fn read_dir(&self, path: &str) -> Result<Vec<(String, DirEntry)>, eudaemonty::Error> {
        let resolved = self.resolve(path);
        self.fs.read_dir(&resolved)
    }

    fn stat(&self, path: &str) -> Result<DirEntry, eudaemonty::Error> {
        let resolved = self.resolve(path);
        self.fs.stat(&resolved)
    }

    fn lstat(&self, path: &str) -> Result<DirEntry, eudaemonty::Error> {
        let resolved = self.resolve(path);
        self.fs.lstat(&resolved)
    }

    fn symlink(&self, target: &str, linkpath: &str) -> Result<(), eudaemonty::Error> {
        let resolved_linkpath = self.resolve(linkpath);
        self.fs.symlink(target, &resolved_linkpath)
    }

    fn link(&self, src: &str, dst: &str) -> Result<(), eudaemonty::Error> {
        let resolved_src = self.resolve(src);
        let resolved_dst = self.resolve(dst);
        self.fs.link(&resolved_src, &resolved_dst)
    }

    fn unlink(&self, path: &str) -> Result<(), eudaemonty::Error> {
        let resolved = self.resolve(path);
        self.fs.unlink(&resolved)
    }

    fn rmdir(&self, path: &str) -> Result<(), eudaemonty::Error> {
        let resolved = self.resolve(path);
        self.fs.rmdir(&resolved)
    }

    fn readlink(&self, path: &str) -> Result<String, eudaemonty::Error> {
        let resolved = self.resolve(path);
        self.fs.readlink(&resolved)
    }

    fn set_times(
        &self,
        path: &str,
        atime: eudaemonty::TimeSpec,
        mtime: eudaemonty::TimeSpec,
    ) -> Result<(), eudaemonty::Error> {
        let resolved = self.resolve(path);
        self.fs.set_times(&resolved, atime, mtime)
    }

    fn lset_times(
        &self,
        path: &str,
        atime: eudaemonty::TimeSpec,
        mtime: eudaemonty::TimeSpec,
    ) -> Result<(), eudaemonty::Error> {
        let resolved = self.resolve(path);
        self.fs.lset_times(&resolved, atime, mtime)
    }

    fn create_file(&self, path: &str) -> Result<bool, eudaemonty::Error> {
        let resolved = self.resolve(path);
        self.fs.create_file(&resolved)
    }

    fn rename(&self, src: &str, dst: &str) -> Result<(), eudaemonty::Error> {
        let resolved_src = self.resolve(src);
        let resolved_dst = self.resolve(dst);
        self.fs.rename(&resolved_src, &resolved_dst)
    }

    fn mkstemp(&self, template: &str) -> Result<String, eudaemonty::Error> {
        let resolved = self.resolve(template);
        self.fs.mkstemp(&resolved)
    }

    fn mkdtemp(&self, template: &str) -> Result<String, eudaemonty::Error> {
        let resolved = self.resolve(template);
        self.fs.mkdtemp(&resolved)
    }
}

fn find_markdown_recursive<FS: EuFilesystem>(
    fs: &FS,
    base: &str,
    path: &str,
    results: &mut Vec<String>,
) -> Result<(), eudaemonty::Error> {
    let entries = fs.read_dir(path)?;

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
    working_dir: &str,
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
            find_markdown_recursive(&env.fs, working_dir, working_dir, &mut files)
                .map_err(|e| FsError::Io(std::io::Error::other(format!("{:?}", e))))?;
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
            let path = resolve_path(working_dir, filename);
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
            let path = resolve_path(working_dir, filename);
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
            let path = resolve_path(working_dir, filename);
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
            env.stdout.write_line(working_dir)?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TestFilesystemExt;
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
        assert!(stdout.contains("#t"));
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
        assert!(stdout.contains("#f"));
    }

    #[test]
    fn eval_save_file() {
        let env = make_test_env(vec![
            "markdownsp",
            "-e",
            "(save (markdown-to-sexpr \"# New Doc\") \"output.md\")",
        ]);
        env.fs.add_directory("/");
        let result = bin(&env).unwrap();
        let stdout = env.stdout.into_string();
        let stderr = env.stderr.into_string();
        println!("stdout: {:?}", stdout);
        println!("stderr: {:?}", stderr);
        assert_eq!(0, result.code());
        // save returns the path as a string
        assert!(stdout.contains("output.md"));

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

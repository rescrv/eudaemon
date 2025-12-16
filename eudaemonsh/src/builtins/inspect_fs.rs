//! The inspect-fs builtin: inspect the shell's filesystem using S-expressions.
//!
//! This builtin launches an interactive S-expression REPL that allows
//! inspection and manipulation of the filesystem attached to the shell.

use lispdown::{Parser, Vm};

use crate::{Environment, Error, ExitCode, Filesystem, Stderr, Stdin, Stdout};

fn print_usage<W: Stderr>(out: &W) -> Result<(), Error> {
    out.write_line("Usage: inspect-fs [-h]")?;
    out.write_line("")?;
    out.write_line("Inspect and manipulate the shell's filesystem interactively.")?;
    out.write_line("")?;
    out.write_line("Options:")?;
    out.write_line("  -h, --help        Print help information")?;
    out.write_line("")?;
    out.write_line("REPL Commands:")?;
    out.write_line("  :help, :h, :?     Show help")?;
    out.write_line("  :quit, :q, :exit  Exit the REPL")?;
    out.write_line("  :tree             Show filesystem tree")?;
    out.write_line("  :usage            Show filesystem usage")?;
    out.write_line("  :fns              List available Lisp functions")?;
    out.write_line("")?;
    out.write_line("Lisp Functions:")?;
    out.write_line("  (fs-read path)           Read file contents")?;
    out.write_line("  (fs-write path content)  Write to file")?;
    out.write_line("  (fs-mkdir path)          Create directory")?;
    out.write_line("  (fs-unlink path)         Remove file")?;
    out.write_line("  (fs-tree)                Get filesystem tree")?;
    out.write_line("  (fs-exists? path)        Check if path exists")?;
    out.write_line("  ... and many more (type :fns in REPL)")?;
    Ok(())
}

fn print_repl_help<W: Stdout>(out: &W) -> Result<(), Error> {
    out.write_line("Commands:")?;
    out.write_line("  :help, :h, :?      Show this help")?;
    out.write_line("  :quit, :q, :exit   Exit the REPL")?;
    out.write_line("  :tree              Show filesystem tree")?;
    out.write_line("  :usage             Show filesystem usage")?;
    out.write_line("  :fns               List available Lisp functions")?;
    out.write_line("")?;
    out.write_line("Lisp evaluation:")?;
    out.write_line("  Type any S-expression to evaluate it.")?;
    out.write_line("")?;
    out.write_line("Examples:")?;
    out.write_line("  (fs-write \"/hello.txt\" \"Hello, World!\")")?;
    out.write_line("  (fs-read \"/hello.txt\")")?;
    out.write_line("  (fs-mkdir \"/mydir\")")?;
    out.write_line("  (fs-tree)")?;
    out.write_line("  (fs-exists? \"/hello.txt\")")?;
    Ok(())
}

fn print_functions<W: Stdout>(out: &W) -> Result<(), Error> {
    out.write_line("Filesystem Functions:")?;
    out.write_line("  (fs-stat path)           Get file metadata")?;
    out.write_line("  (fs-lstat path)          Get metadata (no symlink follow)")?;
    out.write_line("  (fs-read path)           Read file contents")?;
    out.write_line("  (fs-write path content)  Write to file")?;
    out.write_line("  (fs-append path content) Append to file")?;
    out.write_line("  (fs-mkdir path)          Create directory")?;
    out.write_line("  (fs-mkdir-all path)      Create directory with parents")?;
    out.write_line("  (fs-unlink path)         Remove file")?;
    out.write_line("  (fs-rmdir path)          Remove empty directory")?;
    out.write_line("  (fs-readdir path)        List directory")?;
    out.write_line("  (fs-exists? path)        Check if path exists")?;
    out.write_line("  (fs-is-dir? path)        Check if path is directory")?;
    out.write_line("  (fs-symlink target link) Create symlink")?;
    out.write_line("  (fs-readlink path)       Read symlink target")?;
    out.write_line("  (fs-link src dst)        Create hard link")?;
    out.write_line("  (fs-rename src dst)      Rename file/directory")?;
    out.write_line("  (fs-tree)                Get filesystem tree")?;
    out.write_line("  (fs-free-blocks)         Get free block count")?;
    out.write_line("  (fs-total-blocks)        Get total block count")?;
    out.write_line("  (fs-usage-percent)       Get usage percentage")?;
    out.write_line("  (fs-clean)               Run garbage collection")?;
    Ok(())
}

/// The inspect-fs builtin: inspect the shell's filesystem using S-expressions.
pub fn bin<SI, SO, SE, FS>(env: &Environment<SI, SO, SE, FS>) -> Result<ExitCode, Error>
where
    SI: Stdin,
    SO: Stdout,
    SE: Stderr,
    FS: Filesystem + 'static,
{
    // Check for help flag.
    for arg in &env.args[1..] {
        if arg == "-h" || arg == "--help" {
            print_usage(&env.stderr)?;
            return Ok(ExitCode::from(0));
        }
    }

    // Set up the VM with the environment's filesystem.
    let mut vm = Vm::new();
    vm.register_builtins();
    vm.register_json_builtins();
    vm.set_filesystem(Box::new(env.fs.dup()));
    vm.register_filesystem_builtins();

    env.stdout
        .write_line("Inspecting shell filesystem. Type :help for commands, :quit to exit.")?;
    env.stdout.write_line("")?;

    loop {
        env.stdout.write_str("> ")?;

        let line = match env.stdin.read_line()? {
            Some(line) => line,
            None => break, // EOF
        };

        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        // Handle REPL commands.
        if line.starts_with(':') {
            match line {
                ":help" | ":h" | ":?" => print_repl_help(&env.stdout)?,
                ":quit" | ":q" | ":exit" => break,
                ":tree" => {
                    let mut parser = Parser::new("(fs-tree)");
                    if let Ok(expr) = parser.parse() {
                        match vm.eval(&expr) {
                            Ok(result) => env.stdout.write_line(&result.to_string())?,
                            Err(e) => env.stderr.write_line(&format!("Error: {}", e))?,
                        }
                    }
                }
                ":usage" => {
                    let code = "(list (list \"free\" (fs-free-blocks)) (list \"total\" (fs-total-blocks)) (list \"usage%\" (fs-usage-percent)))";
                    let mut parser = Parser::new(code);
                    if let Ok(expr) = parser.parse() {
                        match vm.eval(&expr) {
                            Ok(result) => env.stdout.write_line(&result.to_string())?,
                            Err(e) => env.stderr.write_line(&format!("Error: {}", e))?,
                        }
                    }
                }
                ":fns" => print_functions(&env.stdout)?,
                cmd => env
                    .stderr
                    .write_line(&format!("Unknown command: {}", cmd))?,
            }
            continue;
        }

        // Evaluate S-expression.
        let mut parser = Parser::new(line);
        match parser.parse() {
            Ok(expr) => match vm.eval(&expr) {
                Ok(result) => env.stdout.write_line(&result.to_string())?,
                Err(e) => env.stderr.write_line(&format!("Error: {}", e))?,
            },
            Err(e) => env.stderr.write_line(&format!("Parse error: {}", e))?,
        }
    }

    Ok(ExitCode::from(0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TestFilesystemExt;
    use crate::test_utils::make_test_env;

    #[test]
    fn help_flag() {
        let env = make_test_env(vec!["inspect-fs", "-h"]);
        env.fs.add_directory("/");
        let result = bin(&env).unwrap();
        assert_eq!(0, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("Usage:"));
        assert!(stderr.contains("fs-read"));
    }
}

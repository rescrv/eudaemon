//! agentkb - REPL for markdown s-expression manipulation.
//!
//! Usage: agentkb [directory]
//!
//! If no directory is specified, uses the current working directory.

use std::env;
use std::process::ExitCode;

use agentkb::Repl;

fn main() -> ExitCode {
    let args: Vec<String> = env::args().collect();

    let dir = if args.len() > 1 {
        args[1].clone()
    } else {
        env::current_dir()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| ".".to_string())
    };

    let mut repl = match Repl::new(&dir) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Error: {}", e);
            return ExitCode::FAILURE;
        }
    };

    if let Err(e) = repl.run_interactive() {
        eprintln!("Error: {}", e);
        return ExitCode::FAILURE;
    }

    ExitCode::SUCCESS
}

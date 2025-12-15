//! Convert S-expression representation back to markdown.
//!
//! Reads an S-expression AST from stdin and outputs markdown to stdout.

use std::io::{self, Read, Write};

use lispdown::{Parser, SError, sexpr_to_markdown};

fn main() {
    let mut stdin = io::stdin();
    let mut stdout = io::stdout();
    let mut stderr = io::stderr();

    let mut input = String::new();
    if let Err(e) = stdin.read_to_string(&mut input) {
        let io_error = SError::new("sexpr2markdown")
            .with_code("io-error")
            .with_message("Error reading input")
            .with_string_field("io_error", &e.to_string());
        writeln!(stderr, "{}", io_error.detail()).unwrap();
        std::process::exit(1);
    }

    let input = input.trim();
    if input.is_empty() {
        return;
    }

    let mut parser = Parser::new(input);
    let sexpr = match parser.parse() {
        Ok(sexpr) => sexpr,
        Err(e) => {
            writeln!(stderr, "{}", e.detail()).unwrap();
            std::process::exit(1);
        }
    };

    match sexpr_to_markdown(&sexpr) {
        Ok(markdown) => {
            if let Err(e) = write!(stdout, "{}", markdown) {
                let io_error = SError::new("sexpr2markdown")
                    .with_code("io-error")
                    .with_message("Error writing output")
                    .with_string_field("io_error", &e.to_string());
                writeln!(stderr, "{}", io_error.detail()).unwrap();
                std::process::exit(1);
            }
        }
        Err(e) => {
            writeln!(stderr, "{}", e.detail()).unwrap();
            std::process::exit(1);
        }
    }
}

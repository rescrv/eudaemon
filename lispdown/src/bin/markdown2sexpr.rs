//! Convert markdown to S-expression representation.
//!
//! Reads markdown from stdin and outputs the S-expression AST to stdout.

use std::io::{self, Read, Write};

use lispdown::{SError, markdown_to_sexpr};

fn main() {
    let mut stdin = io::stdin();
    let mut stdout = io::stdout();
    let mut stderr = io::stderr();

    let mut input = String::new();
    if let Err(e) = stdin.read_to_string(&mut input) {
        let io_error = SError::new("markdown2sexpr")
            .with_code("io-error")
            .with_message("Error reading input")
            .with_string_field("io_error", &e.to_string());
        writeln!(stderr, "{}", io_error.detail()).unwrap();
        std::process::exit(1);
    }

    match markdown_to_sexpr(&input) {
        Ok(sexpr) => {
            if let Err(e) = writeln!(stdout, "{}", sexpr) {
                let io_error = SError::new("markdown2sexpr")
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

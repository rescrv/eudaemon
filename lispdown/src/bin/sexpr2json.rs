//! Convert S-expressions (one per line) to JSONL (one JSON value per line)

use std::io::{self, BufRead, Write};

use lispdown::{SError, sexpr_to_json};

fn main() {
    let stdin = io::stdin();
    let mut stdout = io::stdout();
    let mut stderr = io::stderr();

    let mut line_number = 0;
    for line in stdin.lock().lines() {
        line_number += 1;
        match line {
            Ok(sexpr_line) => {
                let sexpr_line = sexpr_line.trim();
                if sexpr_line.is_empty() {
                    continue;
                }

                match sexpr_to_json(sexpr_line) {
                    Ok(json) => {
                        if let Err(e) = writeln!(stdout, "{}", json) {
                            let io_error = SError::new("sexpr2json")
                                .with_code("io-error")
                                .with_message("Error writing output")
                                .with_string_field("io_error", &e.to_string())
                                .with_atom_field("line_number", line_number);
                            writeln!(stderr, "{}", io_error.detail()).unwrap();
                            std::process::exit(1);
                        }
                    }
                    Err(e) => {
                        let error = e.with_atom_field("line_number", line_number);
                        writeln!(stderr, "{}", error.detail()).unwrap();
                        std::process::exit(1);
                    }
                }
            }
            Err(e) => {
                let io_error = SError::new("sexpr2json")
                    .with_code("io-error")
                    .with_message("Error reading input line")
                    .with_string_field("io_error", &e.to_string())
                    .with_atom_field("line_number", line_number);
                writeln!(stderr, "{}", io_error.detail()).unwrap();
                std::process::exit(1);
            }
        }
    }
}

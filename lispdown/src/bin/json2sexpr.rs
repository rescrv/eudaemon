//! Convert JSONL (one JSON value per line) to S-expressions (one per line)

use std::io::{self, BufRead, Write};

use agentkb::{SError, json_to_sexpr};

fn main() {
    let stdin = io::stdin();
    let mut stdout = io::stdout();
    let mut stderr = io::stderr();

    let mut line_number = 0;
    for line in stdin.lock().lines() {
        line_number += 1;
        match line {
            Ok(json_line) => {
                let json_line = json_line.trim();
                if json_line.is_empty() {
                    continue;
                }

                match json_to_sexpr(json_line) {
                    Ok(sexpr) => {
                        if let Err(e) = writeln!(stdout, "{}", sexpr) {
                            let io_error = SError::new("json2sexpr")
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
                let io_error = SError::new("json2sexpr")
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

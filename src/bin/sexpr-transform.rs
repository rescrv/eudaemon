//! Transform S-expressions from stdin to stdout using all registered builtins.
//!
//! Usage: sexpr-transform '<transformation-expr>'
//!
//! The input S-expression is inserted as the first argument after the function name.
//! For example: `echo '(doc (h1 "Hello"))' | sexpr-transform '(prune "1")'`
//! becomes `(prune <input> "1")`.

use std::io::{self, BufRead, Write};

use agentkb::{Parser, SError, SExpr, Vm, register_markdown_builtins};

fn setup_vm() -> Vm {
    let mut vm = Vm::new();
    vm.register_builtins();
    vm.register_json_builtins();
    register_markdown_builtins(&mut vm);
    vm
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: {} '<transformation-expr>'", args[0]);
        eprintln!();
        eprintln!("Examples:");
        eprintln!("  # JSON object manipulation");
        eprintln!(
            "  echo '(obj (\"name\" \"Alice\"))' | {} '(get \"name\")'",
            args[0]
        );
        eprintln!();
        eprintln!("  # Markdown document manipulation");
        eprintln!(
            "  cat doc.md | markdown2sexpr | {} '(prune \"1\")' | sexpr2markdown",
            args[0]
        );
        eprintln!();
        eprintln!("The stdin S-expression is inserted as the first argument to the transform.");
        std::process::exit(1);
    }

    let transform_str = &args[1];
    let mut parser = Parser::new(transform_str);
    let transform_expr = match parser.parse() {
        Ok(expr) => expr,
        Err(e) => {
            eprintln!("{}", e.detail());
            std::process::exit(1);
        }
    };

    let mut vm = setup_vm();
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

                let mut parser = Parser::new(sexpr_line);
                match parser.parse() {
                    Ok(input_expr) => {
                        // Insert the input as the first argument to the transformation
                        // (prune "1") becomes (prune <input> "1")
                        let expr_with_input = match &transform_expr {
                            SExpr::List(items) if !items.is_empty() => {
                                let mut new_items = vec![items[0].clone()];
                                // Wrap input in quote to prevent evaluation
                                new_items.push(SExpr::List(vec![
                                    SExpr::Atom("quote".to_string()),
                                    input_expr.clone(),
                                ]));
                                new_items.extend_from_slice(&items[1..]);
                                SExpr::List(new_items)
                            }
                            SExpr::Atom(_) => {
                                // Atom evaluates to itself, independent of input
                                transform_expr.clone()
                            }
                            SExpr::List(_) => {
                                // Empty list
                                let error = SError::new("sexpr-transform")
                                    .with_code("empty-transformation")
                                    .with_message(
                                        "Transformation expression cannot be an empty list",
                                    )
                                    .with_atom_field("line_number", line_number);
                                writeln!(stderr, "{}", error.detail()).unwrap();
                                std::process::exit(1);
                            }
                        };

                        match vm.eval(&expr_with_input) {
                            Ok(result) => {
                                if let Err(e) = writeln!(stdout, "{}", result) {
                                    let io_error = SError::new("sexpr-transform")
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
                        let error = e.with_atom_field("line_number", line_number);
                        writeln!(stderr, "{}", error.detail()).unwrap();
                        std::process::exit(1);
                    }
                }
            }
            Err(e) => {
                let io_error = SError::new("sexpr-transform")
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

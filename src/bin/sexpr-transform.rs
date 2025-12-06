//! Transform S-expressions from stdin to stdout using dialect functions
//!
//! Usage: sexpr-transform '<transformation-expr>'
//! Example: echo '(obj ("name" "Alice"))' | sexpr-transform '(lambda (x) (get x "name"))'

use std::io::{self, BufRead, Write};

use agentkb::{Env, Parser, SError, SExpr, SResult, eval, unescape_string};
use agentkb::{assoc, dissoc, get, keys, merge, values};

/// Extract string content from a quoted atom and unescape
fn atom_to_string(atom: &str) -> String {
    if atom.starts_with('"') && atom.ends_with('"') && atom.len() >= 2 {
        unescape_string(&atom[1..atom.len() - 1])
    } else {
        atom.to_string()
    }
}

fn setup_env() -> Env {
    let mut env = Env::new();

    // obj constructor: returns itself as-is
    env.def_fn("obj", |args: &[SExpr]| -> SResult<SExpr> {
        Ok(SExpr::List(
            std::iter::once(SExpr::Atom("obj".to_string()))
                .chain(args.iter().cloned())
                .collect(),
        ))
    });

    // arr constructor: returns itself as-is
    env.def_fn("arr", |args: &[SExpr]| -> SResult<SExpr> {
        Ok(SExpr::List(
            std::iter::once(SExpr::Atom("arr".to_string()))
                .chain(args.iter().cloned())
                .collect(),
        ))
    });

    // get function: (get value key)
    env.def_fn("get", |args: &[SExpr]| -> SResult<SExpr> {
        if args.len() != 2 {
            return Err(SError::new("get")
                .with_code("wrong-argument-count")
                .with_message("get requires exactly two arguments")
                .with_atom_field("expected", 2)
                .with_atom_field("received", args.len()));
        }
        let key = match &args[1] {
            SExpr::Atom(s) => atom_to_string(s),
            _ => {
                return Err(SError::new("get")
                    .with_code("invalid-key-type")
                    .with_message("Key must be a string atom")
                    .with_field("key", args[1].clone()));
            }
        };
        Ok(get(&args[0], &key))
    });

    // keys function: (keys value)
    env.def_fn("keys", |args: &[SExpr]| -> SResult<SExpr> {
        if args.len() != 1 {
            return Err(SError::new("keys")
                .with_code("wrong-argument-count")
                .with_message("keys requires exactly one argument")
                .with_atom_field("expected", 1)
                .with_atom_field("received", args.len()));
        }
        Ok(keys(&args[0]))
    });

    // values function: (values value)
    env.def_fn("values", |args: &[SExpr]| -> SResult<SExpr> {
        if args.len() != 1 {
            return Err(SError::new("values")
                .with_code("wrong-argument-count")
                .with_message("values requires exactly one argument")
                .with_atom_field("expected", 1)
                .with_atom_field("received", args.len()));
        }
        Ok(values(&args[0]))
    });

    // assoc function: (assoc value "key" new-value)
    env.def_fn("assoc", |args: &[SExpr]| -> SResult<SExpr> {
        if args.len() != 3 {
            return Err(SError::new("assoc")
                .with_code("wrong-argument-count")
                .with_message("assoc requires exactly three arguments")
                .with_atom_field("expected", 3)
                .with_atom_field("received", args.len()));
        }
        let key = match &args[1] {
            SExpr::Atom(s) => atom_to_string(s),
            _ => {
                return Err(SError::new("assoc")
                    .with_code("invalid-key-type")
                    .with_message("Key must be a string atom")
                    .with_field("key", args[1].clone()));
            }
        };
        Ok(assoc(&args[0], &key, args[2].clone()))
    });

    // dissoc function: (dissoc value "key")
    env.def_fn("dissoc", |args: &[SExpr]| -> SResult<SExpr> {
        if args.len() != 2 {
            return Err(SError::new("dissoc")
                .with_code("wrong-argument-count")
                .with_message("dissoc requires exactly two arguments")
                .with_atom_field("expected", 2)
                .with_atom_field("received", args.len()));
        }
        let key = match &args[1] {
            SExpr::Atom(s) => atom_to_string(s),
            _ => {
                return Err(SError::new("dissoc")
                    .with_code("invalid-key-type")
                    .with_message("Key must be a string atom")
                    .with_field("key", args[1].clone()));
            }
        };
        Ok(dissoc(&args[0], &key))
    });

    // merge function: (merge obj1 obj2 ...)
    env.def_fn("merge", |args: &[SExpr]| -> SResult<SExpr> {
        Ok(merge(args))
    });

    env
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: {} '<transformation-expr>'", args[0]);
        eprintln!(
            "Example: echo '(obj (\"name\" \"Alice\"))' | {} '(get \"name\")'",
            args[0]
        );
        eprintln!("The stdin S-expression is appended as the last argument");
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

    let env = setup_env();
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
                        // (get "name") becomes (get <input> "name")
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

                        match eval(&expr_with_input, &env) {
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

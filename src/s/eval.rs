use crate::s::error::{SError, SResult};
use crate::s::expr::SExpr;
use std::collections::HashMap;

pub type SExprFn = fn(&[SExpr]) -> SResult<SExpr>;

pub struct Env {
    pub funcs: HashMap<String, SExprFn>,
}

impl Default for Env {
    fn default() -> Self {
        Self::new()
    }
}

impl Env {
    pub fn new() -> Self {
        Env {
            funcs: HashMap::new(),
        }
    }

    pub fn def_fn(&mut self, name: &str, f: SExprFn) {
        self.funcs.insert(name.to_string(), f);
    }
}

pub fn eval(expr: &SExpr, env: &Env) -> SResult<SExpr> {
    match expr {
        SExpr::Atom(s) => {
            // attempt to parse as a number, otherwise it's a string
            if let Ok(num) = s.parse::<i64>() {
                Ok(SExpr::Atom(num.to_string()))
            } else {
                Ok(SExpr::Atom(s.clone()))
            }
        }
        SExpr::List(list) => {
            if list.is_empty() {
                return Ok(SExpr::List(vec![])); // Empty list evaluates to itself
            }

            let func_name_expr = &list[0];
            let func_name = match func_name_expr {
                SExpr::Atom(s) => s,
                _ => {
                    return Err(SError::new("eval")
                        .with_code("invalid-function-name")
                        .with_message("First element of a list must be a function name (atom)")
                        .with_field("expression", func_name_expr.clone()));
                }
            };

            let func = env.funcs.get(func_name).ok_or_else(|| {
                SError::new("eval")
                    .with_code("function-not-found")
                    .with_message("Function not found in environment")
                    .with_string_field("function_name", func_name)
                    .with_atom_field("available_functions", env.funcs.len())
            })?;

            let args: SResult<Vec<SExpr>> = list.iter().skip(1).map(|arg| eval(arg, env)).collect();
            let evaluated_args = args?;

            func(&evaluated_args).map_err(|e| {
                e.with_string_field("during_call_to", func_name)
                    .with_atom_field("num_args", evaluated_args.len())
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::s::expr::Parser;

    fn add(args: &[SExpr]) -> SResult<SExpr> {
        let mut sum = 0;
        for (idx, arg) in args.iter().enumerate() {
            if let SExpr::Atom(s) = arg {
                sum += s.parse::<i64>().map_err(|e| {
                    SError::new("add")
                        .with_code("invalid-argument")
                        .with_message("Argument is not a valid integer")
                        .with_atom_field("argument_index", idx)
                        .with_string_field("argument_value", s)
                        .with_string_field("parse_error", &e.to_string())
                })?;
            } else {
                return Err(SError::new("add")
                    .with_code("non-atomic-argument")
                    .with_message("add requires atomic arguments")
                    .with_atom_field("argument_index", idx)
                    .with_field("argument_value", arg.clone()));
            }
        }
        Ok(SExpr::Atom(sum.to_string()))
    }

    fn subtract(args: &[SExpr]) -> SResult<SExpr> {
        if args.len() != 2 {
            return Err(SError::new("subtract")
                .with_code("wrong-argument-count")
                .with_message("subtract requires exactly two arguments")
                .with_atom_field("expected", 2)
                .with_atom_field("received", args.len()));
        }
        let a = match &args[0] {
            SExpr::Atom(s) => s.parse::<i64>().map_err(|e| {
                SError::new("subtract")
                    .with_code("invalid-argument")
                    .with_message("First argument is not a valid integer")
                    .with_string_field("argument_value", s)
                    .with_string_field("parse_error", &e.to_string())
            })?,
            _ => {
                return Err(SError::new("subtract")
                    .with_code("non-atomic-argument")
                    .with_message("subtract arguments must be atoms")
                    .with_atom_field("argument_index", 0)
                    .with_field("argument_value", args[0].clone()));
            }
        };
        let b = match &args[1] {
            SExpr::Atom(s) => s.parse::<i64>().map_err(|e| {
                SError::new("subtract")
                    .with_code("invalid-argument")
                    .with_message("Second argument is not a valid integer")
                    .with_string_field("argument_value", s)
                    .with_string_field("parse_error", &e.to_string())
            })?,
            _ => {
                return Err(SError::new("subtract")
                    .with_code("non-atomic-argument")
                    .with_message("subtract arguments must be atoms")
                    .with_atom_field("argument_index", 1)
                    .with_field("argument_value", args[1].clone()));
            }
        };
        Ok(SExpr::Atom((a - b).to_string()))
    }

    #[test]
    fn test_eval_atom() {
        let env = Env::new();
        let expr = SExpr::Atom("foo".to_string());
        assert_eq!(eval(&expr, &env), Ok(SExpr::Atom("foo".to_string())));
    }

    #[test]
    fn test_eval_numeric_atom() {
        let env = Env::new();
        let expr = SExpr::Atom("123".to_string());
        assert_eq!(eval(&expr, &env), Ok(SExpr::Atom("123".to_string())));
    }

    #[test]
    fn test_eval_simple_func_call() {
        let mut env = Env::new();
        env.def_fn("+", add);

        let mut parser = Parser::new("(+ 1 2)");
        let expr = parser.parse().unwrap();

        assert_eq!(eval(&expr, &env), Ok(SExpr::Atom("3".to_string())));
    }

    #[test]
    fn test_eval_nested_func_call() {
        let mut env = Env::new();
        env.def_fn("+", add);

        let mut parser = Parser::new("(+ 1 (+ 2 3))");
        let expr = parser.parse().unwrap();

        assert_eq!(eval(&expr, &env), Ok(SExpr::Atom("6".to_string())));
    }

    #[test]
    fn test_eval_subtract_func_call() {
        let mut env = Env::new();
        env.def_fn("-", subtract);

        let mut parser = Parser::new("(- 5 2)");
        let expr = parser.parse().unwrap();

        assert_eq!(eval(&expr, &env), Ok(SExpr::Atom("3".to_string())));
    }

    #[test]
    fn test_eval_complex_nested_func_call() {
        let mut env = Env::new();
        env.def_fn("+", add);
        env.def_fn("-", subtract);

        let mut parser = Parser::new("(+ 10 (- 5 (+ 1 1)))");
        let expr = parser.parse().unwrap();

        assert_eq!(eval(&expr, &env), Ok(SExpr::Atom("13".to_string())));
    }

    #[test]
    fn test_unknown_function() {
        let env = Env::new();
        let mut parser = Parser::new("(unknown 1 2)");
        let expr = parser.parse().unwrap();
        assert!(eval(&expr, &env).is_err());
    }
}

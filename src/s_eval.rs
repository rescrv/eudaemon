use crate::s_expr::SExpr;
use std::collections::HashMap;

pub type SExprFn = fn(&[SExpr]) -> Result<SExpr, String>;

pub struct Env {
    pub funcs: HashMap<String, SExprFn>,
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

pub fn eval(expr: &SExpr, env: &Env) -> Result<SExpr, String> {
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
                _ => return Err("First element of a list must be a function name".to_string()),
            };

            let func = env
                .funcs
                .get(func_name)
                .ok_or(format!("Function '{}' not found", func_name))?;

            let args: Result<Vec<SExpr>, String> =
                list.iter().skip(1).map(|arg| eval(arg, env)).collect();
            let evaluated_args = args?;

            func(&evaluated_args)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::s_expr::Parser;

    fn add(args: &[SExpr]) -> Result<SExpr, String> {
        let mut sum = 0;
        for arg in args {
            if let SExpr::Atom(s) = arg {
                sum += s.parse::<i64>().map_err(|e| e.to_string())?;
            } else {
                return Err("add requires atomic arguments".to_string());
            }
        }
        Ok(SExpr::Atom(sum.to_string()))
    }
    
    fn subtract(args: &[SExpr]) -> Result<SExpr, String> {
        if args.len() != 2 {
            return Err("subtract requires exactly two arguments".to_string());
        }
        let a = match &args[0] {
            SExpr::Atom(s) => s.parse::<i64>().map_err(|e| e.to_string())?,
            _ => return Err("subtract arguments must be atoms".to_string()),
        };
        let b = match &args[1] {
            SExpr::Atom(s) => s.parse::<i64>().map_err(|e| e.to_string())?,
            _ => return Err("subtract arguments must be atoms".to_string()),
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

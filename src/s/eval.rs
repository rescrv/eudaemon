//! S-expression evaluator with special forms and environment bindings.

use std::collections::HashMap;

use super::error::{SError, SResult};
use super::expr::SExpr;

/// Function signature for built-in functions.
pub type SExprFn = fn(&[SExpr]) -> SResult<SExpr>;

/// Evaluation environment containing function definitions and variable bindings.
#[derive(Clone)]
pub struct Env {
    /// Built-in functions.
    funcs: HashMap<String, SExprFn>,
    /// Variable bindings (from let expressions).
    bindings: HashMap<String, SExpr>,
    /// Parent environment for lexical scoping.
    parent: Option<Box<Env>>,
}

impl Default for Env {
    fn default() -> Self {
        Self::new()
    }
}

impl Env {
    /// Creates a new empty environment.
    pub fn new() -> Self {
        Env {
            funcs: HashMap::new(),
            bindings: HashMap::new(),
            parent: None,
        }
    }

    /// Creates a child environment with this environment as parent.
    fn child(&self) -> Self {
        Env {
            funcs: HashMap::new(),
            bindings: HashMap::new(),
            parent: Some(Box::new(self.clone())),
        }
    }

    /// Defines a built-in function.
    pub fn def_fn(&mut self, name: &str, f: SExprFn) {
        self.funcs.insert(name.to_string(), f);
    }

    /// Binds a variable in this environment.
    fn bind(&mut self, name: &str, value: SExpr) {
        self.bindings.insert(name.to_string(), value);
    }

    /// Looks up a variable, searching parent environments.
    fn lookup(&self, name: &str) -> Option<&SExpr> {
        self.bindings
            .get(name)
            .or_else(|| self.parent.as_ref().and_then(|p| p.lookup(name)))
    }

    /// Looks up a function, searching parent environments.
    fn lookup_fn(&self, name: &str) -> Option<&SExprFn> {
        self.funcs
            .get(name)
            .or_else(|| self.parent.as_ref().and_then(|p| p.lookup_fn(name)))
    }
}

/// Evaluates an S-expression in the given environment.
pub fn eval(expr: &SExpr, env: &Env) -> SResult<SExpr> {
    eval_impl(expr, &mut env.clone())
}

/// Internal evaluation with mutable environment for let bindings.
fn eval_impl(expr: &SExpr, env: &mut Env) -> SResult<SExpr> {
    match expr {
        SExpr::Atom(s) => {
            // Check for variable binding first
            if let Some(value) = env.lookup(s) {
                return Ok(value.clone());
            }
            // Attempt to parse as a number, otherwise it's a literal
            if let Ok(num) = s.parse::<i64>() {
                Ok(SExpr::Atom(num.to_string()))
            } else {
                Ok(SExpr::Atom(s.clone()))
            }
        }
        SExpr::List(list) => {
            if list.is_empty() {
                return Ok(SExpr::List(vec![]));
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

            // Handle special forms
            match func_name.as_str() {
                "quote" => eval_quote(list),
                "if" => eval_if(list, env),
                "let" => eval_let(list, env),
                "begin" => eval_begin(list, env),
                "->>" => eval_thread_last(list, env),
                "map" => eval_map(list, env),
                "filter" => eval_filter(list, env),
                "reduce" => eval_reduce(list, env),
                _ => eval_function_call(func_name, list, env),
            }
        }
    }
}

/// Evaluates (quote expr) - returns expr unevaluated.
fn eval_quote(list: &[SExpr]) -> SResult<SExpr> {
    if list.len() != 2 {
        return Err(SError::new("eval")
            .with_code("wrong-argument-count")
            .with_message("quote requires exactly one argument")
            .with_atom_field("expected", 1)
            .with_atom_field("received", list.len() - 1));
    }
    Ok(list[1].clone())
}

/// Evaluates (if condition then-expr else-expr).
fn eval_if(list: &[SExpr], env: &mut Env) -> SResult<SExpr> {
    if list.len() != 4 {
        return Err(SError::new("eval")
            .with_code("wrong-argument-count")
            .with_message("if requires exactly three arguments: condition, then-expr, else-expr")
            .with_atom_field("expected", 3)
            .with_atom_field("received", list.len() - 1));
    }

    let condition = eval_impl(&list[1], env)?;
    let is_truthy = is_truthy(&condition);

    if is_truthy {
        eval_impl(&list[2], env)
    } else {
        eval_impl(&list[3], env)
    }
}

/// Determines if a value is truthy.
/// False values: #f, null, empty list ()
/// Everything else is truthy.
fn is_truthy(expr: &SExpr) -> bool {
    match expr {
        SExpr::Atom(s) => s != "#f" && s != "null",
        SExpr::List(items) => !items.is_empty(),
    }
}

/// Evaluates (let ((var1 val1) (var2 val2) ...) body...).
fn eval_let(list: &[SExpr], env: &mut Env) -> SResult<SExpr> {
    if list.len() < 3 {
        return Err(SError::new("eval")
            .with_code("wrong-argument-count")
            .with_message("let requires bindings and at least one body expression")
            .with_atom_field("minimum_args", 2)
            .with_atom_field("received", list.len() - 1));
    }

    // Parse bindings
    let bindings_expr = &list[1];
    let bindings = match bindings_expr {
        SExpr::List(b) => b,
        _ => {
            return Err(SError::new("eval")
                .with_code("invalid-let-bindings")
                .with_message("let bindings must be a list of (var value) pairs")
                .with_field("bindings", bindings_expr.clone()));
        }
    };

    // Create child environment for let scope
    let mut child_env = env.child();

    // Process each binding
    for (idx, binding) in bindings.iter().enumerate() {
        match binding {
            SExpr::List(pair) if pair.len() == 2 => {
                let var_name = match &pair[0] {
                    SExpr::Atom(name) => name.clone(),
                    _ => {
                        return Err(SError::new("eval")
                            .with_code("invalid-binding-name")
                            .with_message("Binding name must be an atom")
                            .with_atom_field("binding_index", idx)
                            .with_field("name_expr", pair[0].clone()));
                    }
                };
                // Evaluate value in parent environment (not the child with partial bindings)
                let value = eval_impl(&pair[1], env)?;
                child_env.bind(&var_name, value);
            }
            _ => {
                return Err(SError::new("eval")
                    .with_code("invalid-binding")
                    .with_message("Each binding must be a (var value) pair")
                    .with_atom_field("binding_index", idx)
                    .with_field("binding", binding.clone()));
            }
        }
    }

    // Evaluate body expressions in child environment
    let mut result = SExpr::List(vec![]);
    for body_expr in list.iter().skip(2) {
        result = eval_impl(body_expr, &mut child_env)?;
    }
    Ok(result)
}

/// Evaluates (begin expr1 expr2 ...) - sequences expressions, returns last.
fn eval_begin(list: &[SExpr], env: &mut Env) -> SResult<SExpr> {
    if list.len() < 2 {
        return Err(SError::new("eval")
            .with_code("wrong-argument-count")
            .with_message("begin requires at least one expression")
            .with_atom_field("minimum_args", 1)
            .with_atom_field("received", list.len() - 1));
    }

    let mut result = SExpr::List(vec![]);
    for expr in list.iter().skip(1) {
        result = eval_impl(expr, env)?;
    }
    Ok(result)
}

/// Evaluates (map func-name list) - applies func to each element, returns new list.
fn eval_map(list: &[SExpr], env: &mut Env) -> SResult<SExpr> {
    if list.len() != 3 {
        return Err(SError::new("map")
            .with_code("wrong-argument-count")
            .with_message("map requires exactly two arguments: function-name and list")
            .with_atom_field("expected", 2)
            .with_atom_field("received", list.len() - 1));
    }

    let func_name = match &list[1] {
        SExpr::Atom(s) => s.clone(),
        _ => {
            return Err(SError::new("map")
                .with_code("invalid-function")
                .with_message("First argument must be a function name")
                .with_field("argument", list[1].clone()));
        }
    };

    let list_expr = eval_impl(&list[2], env)?;
    let items = match &list_expr {
        SExpr::List(items) => items,
        _ => {
            return Err(SError::new("map")
                .with_code("type-error")
                .with_message("Second argument must be a list")
                .with_field("argument", list_expr));
        }
    };

    let mut results = Vec::new();
    for item in items {
        let call = SExpr::List(vec![
            SExpr::Atom(func_name.clone()),
            SExpr::List(vec![SExpr::Atom("quote".to_string()), item.clone()]),
        ]);
        results.push(eval_impl(&call, env)?);
    }

    Ok(SExpr::List(results))
}

/// Evaluates (filter func-name list) - keeps elements where func returns truthy.
fn eval_filter(list: &[SExpr], env: &mut Env) -> SResult<SExpr> {
    if list.len() != 3 {
        return Err(SError::new("filter")
            .with_code("wrong-argument-count")
            .with_message("filter requires exactly two arguments: function-name and list")
            .with_atom_field("expected", 2)
            .with_atom_field("received", list.len() - 1));
    }

    let func_name = match &list[1] {
        SExpr::Atom(s) => s.clone(),
        _ => {
            return Err(SError::new("filter")
                .with_code("invalid-function")
                .with_message("First argument must be a function name")
                .with_field("argument", list[1].clone()));
        }
    };

    let list_expr = eval_impl(&list[2], env)?;
    let items = match &list_expr {
        SExpr::List(items) => items,
        _ => {
            return Err(SError::new("filter")
                .with_code("type-error")
                .with_message("Second argument must be a list")
                .with_field("argument", list_expr));
        }
    };

    let mut results = Vec::new();
    for item in items {
        let call = SExpr::List(vec![
            SExpr::Atom(func_name.clone()),
            SExpr::List(vec![SExpr::Atom("quote".to_string()), item.clone()]),
        ]);
        let result = eval_impl(&call, env)?;
        if is_truthy(&result) {
            results.push(item.clone());
        }
    }

    Ok(SExpr::List(results))
}

/// Evaluates (reduce func-name initial list) - folds list with func(acc, elem).
fn eval_reduce(list: &[SExpr], env: &mut Env) -> SResult<SExpr> {
    if list.len() != 4 {
        return Err(SError::new("reduce")
            .with_code("wrong-argument-count")
            .with_message(
                "reduce requires exactly three arguments: function-name, initial, and list",
            )
            .with_atom_field("expected", 3)
            .with_atom_field("received", list.len() - 1));
    }

    let func_name = match &list[1] {
        SExpr::Atom(s) => s.clone(),
        _ => {
            return Err(SError::new("reduce")
                .with_code("invalid-function")
                .with_message("First argument must be a function name")
                .with_field("argument", list[1].clone()));
        }
    };

    let mut acc = eval_impl(&list[2], env)?;

    let list_expr = eval_impl(&list[3], env)?;
    let items = match &list_expr {
        SExpr::List(items) => items,
        _ => {
            return Err(SError::new("reduce")
                .with_code("type-error")
                .with_message("Third argument must be a list")
                .with_field("argument", list_expr));
        }
    };

    for item in items {
        let call = SExpr::List(vec![
            SExpr::Atom(func_name.clone()),
            SExpr::List(vec![SExpr::Atom("quote".to_string()), acc.clone()]),
            SExpr::List(vec![SExpr::Atom("quote".to_string()), item.clone()]),
        ]);
        acc = eval_impl(&call, env)?;
    }

    Ok(acc)
}

/// Evaluates (->> initial (f1 args...) (f2 args...) ...) - thread-last macro.
/// Each form has the result of the previous expression inserted as its last argument.
fn eval_thread_last(list: &[SExpr], env: &mut Env) -> SResult<SExpr> {
    if list.len() < 2 {
        return Err(SError::new("eval")
            .with_code("wrong-argument-count")
            .with_message("->> requires at least an initial value")
            .with_atom_field("minimum_args", 1)
            .with_atom_field("received", list.len() - 1));
    }

    // Evaluate initial value
    let mut result = eval_impl(&list[1], env)?;

    // Thread through remaining forms
    for form in list.iter().skip(2) {
        match form {
            SExpr::List(items) if !items.is_empty() => {
                // Insert result as last argument
                let mut new_items = items.clone();
                new_items.push(SExpr::List(vec![
                    SExpr::Atom("quote".to_string()),
                    result.clone(),
                ]));
                result = eval_impl(&SExpr::List(new_items), env)?;
            }
            SExpr::Atom(func_name) => {
                // Single function name: (func result)
                let call = SExpr::List(vec![
                    SExpr::Atom(func_name.clone()),
                    SExpr::List(vec![SExpr::Atom("quote".to_string()), result.clone()]),
                ]);
                result = eval_impl(&call, env)?;
            }
            _ => {
                return Err(SError::new("eval")
                    .with_code("invalid-thread-form")
                    .with_message("Thread form must be a function call or function name")
                    .with_field("form", form.clone()));
            }
        }
    }

    Ok(result)
}

// ============================================================================
// Built-in predicates
// ============================================================================

/// Returns #t if the argument is null, #f otherwise.
pub fn builtin_null_p(args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 1 {
        return Err(SError::new("null?")
            .with_code("wrong-argument-count")
            .with_message("null? requires exactly one argument")
            .with_atom_field("expected", 1)
            .with_atom_field("received", args.len()));
    }
    let result = matches!(&args[0], SExpr::Atom(s) if s == "null");
    Ok(SExpr::Atom(if result { "#t" } else { "#f" }.to_string()))
}

/// Returns #t if the argument is a list, #f otherwise.
pub fn builtin_list_p(args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 1 {
        return Err(SError::new("list?")
            .with_code("wrong-argument-count")
            .with_message("list? requires exactly one argument")
            .with_atom_field("expected", 1)
            .with_atom_field("received", args.len()));
    }
    let result = matches!(&args[0], SExpr::List(_));
    Ok(SExpr::Atom(if result { "#t" } else { "#f" }.to_string()))
}

/// Returns #t if the argument is an atom, #f otherwise.
pub fn builtin_atom_p(args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 1 {
        return Err(SError::new("atom?")
            .with_code("wrong-argument-count")
            .with_message("atom? requires exactly one argument")
            .with_atom_field("expected", 1)
            .with_atom_field("received", args.len()));
    }
    let result = matches!(&args[0], SExpr::Atom(_));
    Ok(SExpr::Atom(if result { "#t" } else { "#f" }.to_string()))
}

/// Returns #t if the argument is an empty list, #f otherwise.
pub fn builtin_empty_p(args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 1 {
        return Err(SError::new("empty?")
            .with_code("wrong-argument-count")
            .with_message("empty? requires exactly one argument")
            .with_atom_field("expected", 1)
            .with_atom_field("received", args.len()));
    }
    let result = matches!(&args[0], SExpr::List(items) if items.is_empty());
    Ok(SExpr::Atom(if result { "#t" } else { "#f" }.to_string()))
}

/// Returns #t if the two arguments are structurally equal, #f otherwise.
pub fn builtin_eq_p(args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 2 {
        return Err(SError::new("eq?")
            .with_code("wrong-argument-count")
            .with_message("eq? requires exactly two arguments")
            .with_atom_field("expected", 2)
            .with_atom_field("received", args.len()));
    }
    let result = args[0] == args[1];
    Ok(SExpr::Atom(if result { "#t" } else { "#f" }.to_string()))
}

// ============================================================================
// Built-in list operations
// ============================================================================

/// Returns the first element of a list.
pub fn builtin_first(args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 1 {
        return Err(SError::new("first")
            .with_code("wrong-argument-count")
            .with_message("first requires exactly one argument")
            .with_atom_field("expected", 1)
            .with_atom_field("received", args.len()));
    }
    match &args[0] {
        SExpr::List(items) => {
            if items.is_empty() {
                Err(SError::new("first")
                    .with_code("empty-list")
                    .with_message("Cannot take first of empty list"))
            } else {
                Ok(items[0].clone())
            }
        }
        _ => Err(SError::new("first")
            .with_code("type-error")
            .with_message("first requires a list argument")
            .with_field("argument", args[0].clone())),
    }
}

/// Returns the list without its first element.
pub fn builtin_rest(args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 1 {
        return Err(SError::new("rest")
            .with_code("wrong-argument-count")
            .with_message("rest requires exactly one argument")
            .with_atom_field("expected", 1)
            .with_atom_field("received", args.len()));
    }
    match &args[0] {
        SExpr::List(items) => {
            if items.is_empty() {
                Err(SError::new("rest")
                    .with_code("empty-list")
                    .with_message("Cannot take rest of empty list"))
            } else {
                Ok(SExpr::List(items[1..].to_vec()))
            }
        }
        _ => Err(SError::new("rest")
            .with_code("type-error")
            .with_message("rest requires a list argument")
            .with_field("argument", args[0].clone())),
    }
}

/// Constructs a new list with the first argument prepended to the second (a list).
pub fn builtin_cons(args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 2 {
        return Err(SError::new("cons")
            .with_code("wrong-argument-count")
            .with_message("cons requires exactly two arguments")
            .with_atom_field("expected", 2)
            .with_atom_field("received", args.len()));
    }
    match &args[1] {
        SExpr::List(items) => {
            let mut new_items = vec![args[0].clone()];
            new_items.extend(items.iter().cloned());
            Ok(SExpr::List(new_items))
        }
        _ => Err(SError::new("cons")
            .with_code("type-error")
            .with_message("Second argument to cons must be a list")
            .with_field("argument", args[1].clone())),
    }
}

/// Appends all argument lists together.
pub fn builtin_append(args: &[SExpr]) -> SResult<SExpr> {
    let mut result = Vec::new();
    for (idx, arg) in args.iter().enumerate() {
        match arg {
            SExpr::List(items) => result.extend(items.iter().cloned()),
            _ => {
                return Err(SError::new("append")
                    .with_code("type-error")
                    .with_message("All arguments to append must be lists")
                    .with_atom_field("argument_index", idx)
                    .with_field("argument", arg.clone()));
            }
        }
    }
    Ok(SExpr::List(result))
}

/// Returns the length of a list.
pub fn builtin_length(args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 1 {
        return Err(SError::new("length")
            .with_code("wrong-argument-count")
            .with_message("length requires exactly one argument")
            .with_atom_field("expected", 1)
            .with_atom_field("received", args.len()));
    }
    match &args[0] {
        SExpr::List(items) => Ok(SExpr::Atom(items.len().to_string())),
        _ => Err(SError::new("length")
            .with_code("type-error")
            .with_message("length requires a list argument")
            .with_field("argument", args[0].clone())),
    }
}

/// Returns the nth element of a list (0-indexed).
pub fn builtin_nth(args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 2 {
        return Err(SError::new("nth")
            .with_code("wrong-argument-count")
            .with_message("nth requires exactly two arguments: index and list")
            .with_atom_field("expected", 2)
            .with_atom_field("received", args.len()));
    }
    let index = match &args[0] {
        SExpr::Atom(s) => s.parse::<usize>().map_err(|_| {
            SError::new("nth")
                .with_code("invalid-index")
                .with_message("First argument must be a non-negative integer")
                .with_string_field("argument", s)
        })?,
        _ => {
            return Err(SError::new("nth")
                .with_code("type-error")
                .with_message("First argument must be an integer index")
                .with_field("argument", args[0].clone()));
        }
    };
    match &args[1] {
        SExpr::List(items) => {
            if index >= items.len() {
                Err(SError::new("nth")
                    .with_code("index-out-of-bounds")
                    .with_message("Index out of bounds")
                    .with_atom_field("index", index)
                    .with_atom_field("list_length", items.len()))
            } else {
                Ok(items[index].clone())
            }
        }
        _ => Err(SError::new("nth")
            .with_code("type-error")
            .with_message("Second argument must be a list")
            .with_field("argument", args[1].clone())),
    }
}

/// Creates a list from arguments.
pub fn builtin_list(args: &[SExpr]) -> SResult<SExpr> {
    Ok(SExpr::List(args.to_vec()))
}

/// Registers all standard built-in functions in the environment.
pub fn register_builtins(env: &mut Env) {
    // Predicates
    env.def_fn("null?", builtin_null_p);
    env.def_fn("list?", builtin_list_p);
    env.def_fn("atom?", builtin_atom_p);
    env.def_fn("empty?", builtin_empty_p);
    env.def_fn("eq?", builtin_eq_p);
    // List operations
    env.def_fn("first", builtin_first);
    env.def_fn("rest", builtin_rest);
    env.def_fn("cons", builtin_cons);
    env.def_fn("append", builtin_append);
    env.def_fn("length", builtin_length);
    env.def_fn("nth", builtin_nth);
    env.def_fn("list", builtin_list);
}

/// Evaluates a regular function call.
fn eval_function_call(func_name: &str, list: &[SExpr], env: &mut Env) -> SResult<SExpr> {
    let func = env.lookup_fn(func_name).ok_or_else(|| {
        SError::new("eval")
            .with_code("function-not-found")
            .with_message("Function not found in environment")
            .with_string_field("function_name", func_name)
    })?;
    let func = *func; // Copy the function pointer

    let args: SResult<Vec<SExpr>> = list.iter().skip(1).map(|arg| eval_impl(arg, env)).collect();
    let evaluated_args = args?;

    func(&evaluated_args).map_err(|e| {
        e.with_string_field("during_call_to", func_name)
            .with_atom_field("num_args", evaluated_args.len())
    })
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

    fn multiply(args: &[SExpr]) -> SResult<SExpr> {
        let mut product = 1;
        for (idx, arg) in args.iter().enumerate() {
            if let SExpr::Atom(s) = arg {
                product *= s.parse::<i64>().map_err(|e| {
                    SError::new("multiply")
                        .with_code("invalid-argument")
                        .with_message("Argument is not a valid integer")
                        .with_atom_field("argument_index", idx)
                        .with_string_field("argument_value", s)
                        .with_string_field("parse_error", &e.to_string())
                })?;
            } else {
                return Err(SError::new("multiply")
                    .with_code("non-atomic-argument")
                    .with_message("multiply requires atomic arguments")
                    .with_atom_field("argument_index", idx)
                    .with_field("argument_value", arg.clone()));
            }
        }
        Ok(SExpr::Atom(product.to_string()))
    }

    #[test]
    fn eval_atom() {
        let env = Env::new();
        let expr = SExpr::Atom("foo".to_string());
        assert_eq!(eval(&expr, &env), Ok(SExpr::Atom("foo".to_string())));
    }

    #[test]
    fn eval_numeric_atom() {
        let env = Env::new();
        let expr = SExpr::Atom("123".to_string());
        assert_eq!(eval(&expr, &env), Ok(SExpr::Atom("123".to_string())));
    }

    #[test]
    fn eval_simple_func_call() {
        let mut env = Env::new();
        env.def_fn("+", add);

        let mut parser = Parser::new("(+ 1 2)");
        let expr = parser.parse().unwrap();

        assert_eq!(eval(&expr, &env), Ok(SExpr::Atom("3".to_string())));
    }

    #[test]
    fn eval_nested_func_call() {
        let mut env = Env::new();
        env.def_fn("+", add);

        let mut parser = Parser::new("(+ 1 (+ 2 3))");
        let expr = parser.parse().unwrap();

        assert_eq!(eval(&expr, &env), Ok(SExpr::Atom("6".to_string())));
    }

    #[test]
    fn eval_subtract_func_call() {
        let mut env = Env::new();
        env.def_fn("-", subtract);

        let mut parser = Parser::new("(- 5 2)");
        let expr = parser.parse().unwrap();

        assert_eq!(eval(&expr, &env), Ok(SExpr::Atom("3".to_string())));
    }

    #[test]
    fn eval_complex_nested_func_call() {
        let mut env = Env::new();
        env.def_fn("+", add);
        env.def_fn("-", subtract);

        let mut parser = Parser::new("(+ 10 (- 5 (+ 1 1)))");
        let expr = parser.parse().unwrap();

        assert_eq!(eval(&expr, &env), Ok(SExpr::Atom("13".to_string())));
    }

    #[test]
    fn unknown_function_error() {
        let env = Env::new();
        let mut parser = Parser::new("(unknown 1 2)");
        let expr = parser.parse().unwrap();
        let err = eval(&expr, &env).unwrap_err();
        assert!(err.to_string().contains("function-not-found"));
    }

    // Tests for special forms

    #[test]
    fn eval_if_true_branch() {
        let mut env = Env::new();
        env.def_fn("+", add);

        let mut parser = Parser::new("(if #t (+ 1 2) (+ 3 4))");
        let expr = parser.parse().unwrap();

        assert_eq!(eval(&expr, &env), Ok(SExpr::Atom("3".to_string())));
    }

    #[test]
    fn eval_if_false_branch() {
        let mut env = Env::new();
        env.def_fn("+", add);

        let mut parser = Parser::new("(if #f (+ 1 2) (+ 3 4))");
        let expr = parser.parse().unwrap();

        assert_eq!(eval(&expr, &env), Ok(SExpr::Atom("7".to_string())));
    }

    #[test]
    fn eval_if_null_is_falsy() {
        let mut env = Env::new();
        env.def_fn("+", add);

        let mut parser = Parser::new("(if null (+ 1 2) (+ 3 4))");
        let expr = parser.parse().unwrap();

        assert_eq!(eval(&expr, &env), Ok(SExpr::Atom("7".to_string())));
    }

    #[test]
    fn eval_if_number_is_truthy() {
        let mut env = Env::new();
        env.def_fn("+", add);

        let mut parser = Parser::new("(if 0 (+ 1 2) (+ 3 4))");
        let expr = parser.parse().unwrap();

        // 0 is truthy (only #f and null are falsy)
        assert_eq!(eval(&expr, &env), Ok(SExpr::Atom("3".to_string())));
    }

    #[test]
    fn eval_let_simple() {
        let mut env = Env::new();
        env.def_fn("+", add);

        let mut parser = Parser::new("(let ((x 5)) (+ x 3))");
        let expr = parser.parse().unwrap();

        assert_eq!(eval(&expr, &env), Ok(SExpr::Atom("8".to_string())));
    }

    #[test]
    fn eval_let_multiple_bindings() {
        let mut env = Env::new();
        env.def_fn("+", add);

        let mut parser = Parser::new("(let ((x 5) (y 10)) (+ x y))");
        let expr = parser.parse().unwrap();

        assert_eq!(eval(&expr, &env), Ok(SExpr::Atom("15".to_string())));
    }

    #[test]
    fn eval_let_nested() {
        let mut env = Env::new();
        env.def_fn("+", add);

        let mut parser = Parser::new("(let ((x 5)) (let ((y 10)) (+ x y)))");
        let expr = parser.parse().unwrap();

        assert_eq!(eval(&expr, &env), Ok(SExpr::Atom("15".to_string())));
    }

    #[test]
    fn eval_let_shadowing() {
        let mut env = Env::new();
        env.def_fn("+", add);

        let mut parser = Parser::new("(let ((x 5)) (let ((x 10)) x))");
        let expr = parser.parse().unwrap();

        assert_eq!(eval(&expr, &env), Ok(SExpr::Atom("10".to_string())));
    }

    #[test]
    fn eval_let_multiple_body_expressions() {
        let mut env = Env::new();
        env.def_fn("+", add);

        let mut parser = Parser::new("(let ((x 5)) (+ x 1) (+ x 2))");
        let expr = parser.parse().unwrap();

        // Returns the last expression
        assert_eq!(eval(&expr, &env), Ok(SExpr::Atom("7".to_string())));
    }

    #[test]
    fn eval_begin() {
        let mut env = Env::new();
        env.def_fn("+", add);

        let mut parser = Parser::new("(begin (+ 1 2) (+ 3 4) (+ 5 6))");
        let expr = parser.parse().unwrap();

        assert_eq!(eval(&expr, &env), Ok(SExpr::Atom("11".to_string())));
    }

    #[test]
    fn eval_begin_single() {
        let mut env = Env::new();
        env.def_fn("+", add);

        let mut parser = Parser::new("(begin (+ 1 2))");
        let expr = parser.parse().unwrap();

        assert_eq!(eval(&expr, &env), Ok(SExpr::Atom("3".to_string())));
    }

    #[test]
    fn eval_thread_last_simple() {
        let mut env = Env::new();
        env.def_fn("+", add);
        env.def_fn("*", multiply);

        // (->> 5 (+ 3)) == (+ 3 5) == 8
        let mut parser = Parser::new("(->> 5 (+ 3))");
        let expr = parser.parse().unwrap();

        assert_eq!(eval(&expr, &env), Ok(SExpr::Atom("8".to_string())));
    }

    #[test]
    fn eval_thread_last_chain() {
        let mut env = Env::new();
        env.def_fn("+", add);
        env.def_fn("*", multiply);

        // (->> 5 (+ 3) (* 2)) == (* 2 (+ 3 5)) == (* 2 8) == 16
        let mut parser = Parser::new("(->> 5 (+ 3) (* 2))");
        let expr = parser.parse().unwrap();

        assert_eq!(eval(&expr, &env), Ok(SExpr::Atom("16".to_string())));
    }

    #[test]
    fn eval_combined_special_forms() {
        let mut env = Env::new();
        env.def_fn("+", add);
        env.def_fn("-", subtract);

        // Complex expression combining let, if, and function calls
        let mut parser = Parser::new("(let ((x 10) (y 5)) (if #t (+ x y) (- x y)))");
        let expr = parser.parse().unwrap();

        assert_eq!(eval(&expr, &env), Ok(SExpr::Atom("15".to_string())));
    }

    // Tests for built-in predicates

    #[test]
    fn builtin_null_p_true() {
        let mut env = Env::new();
        register_builtins(&mut env);

        let mut parser = Parser::new("(null? null)");
        let expr = parser.parse().unwrap();
        assert_eq!(eval(&expr, &env), Ok(SExpr::Atom("#t".to_string())));
    }

    #[test]
    fn builtin_null_p_false() {
        let mut env = Env::new();
        register_builtins(&mut env);

        let mut parser = Parser::new("(null? foo)");
        let expr = parser.parse().unwrap();
        assert_eq!(eval(&expr, &env), Ok(SExpr::Atom("#f".to_string())));
    }

    #[test]
    fn builtin_list_p_true() {
        let mut env = Env::new();
        register_builtins(&mut env);

        let mut parser = Parser::new("(list? (quote (1 2 3)))");
        let expr = parser.parse().unwrap();
        assert_eq!(eval(&expr, &env), Ok(SExpr::Atom("#t".to_string())));
    }

    #[test]
    fn builtin_list_p_false() {
        let mut env = Env::new();
        register_builtins(&mut env);

        let mut parser = Parser::new("(list? foo)");
        let expr = parser.parse().unwrap();
        assert_eq!(eval(&expr, &env), Ok(SExpr::Atom("#f".to_string())));
    }

    #[test]
    fn builtin_atom_p_true() {
        let mut env = Env::new();
        register_builtins(&mut env);

        let mut parser = Parser::new("(atom? foo)");
        let expr = parser.parse().unwrap();
        assert_eq!(eval(&expr, &env), Ok(SExpr::Atom("#t".to_string())));
    }

    #[test]
    fn builtin_atom_p_false() {
        let mut env = Env::new();
        register_builtins(&mut env);

        let mut parser = Parser::new("(atom? (quote (1 2)))");
        let expr = parser.parse().unwrap();
        assert_eq!(eval(&expr, &env), Ok(SExpr::Atom("#f".to_string())));
    }

    #[test]
    fn builtin_empty_p_true() {
        let mut env = Env::new();
        register_builtins(&mut env);

        let mut parser = Parser::new("(empty? (quote ()))");
        let expr = parser.parse().unwrap();
        assert_eq!(eval(&expr, &env), Ok(SExpr::Atom("#t".to_string())));
    }

    #[test]
    fn builtin_empty_p_false() {
        let mut env = Env::new();
        register_builtins(&mut env);

        let mut parser = Parser::new("(empty? (quote (1)))");
        let expr = parser.parse().unwrap();
        assert_eq!(eval(&expr, &env), Ok(SExpr::Atom("#f".to_string())));
    }

    #[test]
    fn builtin_eq_p_atoms_equal() {
        let mut env = Env::new();
        register_builtins(&mut env);

        let mut parser = Parser::new("(eq? foo foo)");
        let expr = parser.parse().unwrap();
        assert_eq!(eval(&expr, &env), Ok(SExpr::Atom("#t".to_string())));
    }

    #[test]
    fn builtin_eq_p_atoms_not_equal() {
        let mut env = Env::new();
        register_builtins(&mut env);

        let mut parser = Parser::new("(eq? foo bar)");
        let expr = parser.parse().unwrap();
        assert_eq!(eval(&expr, &env), Ok(SExpr::Atom("#f".to_string())));
    }

    #[test]
    fn builtin_eq_p_lists_equal() {
        let mut env = Env::new();
        register_builtins(&mut env);

        let mut parser = Parser::new("(eq? (quote (1 2)) (quote (1 2)))");
        let expr = parser.parse().unwrap();
        assert_eq!(eval(&expr, &env), Ok(SExpr::Atom("#t".to_string())));
    }

    // Tests for built-in list operations

    #[test]
    fn builtin_first_returns_head() {
        let mut env = Env::new();
        register_builtins(&mut env);

        let mut parser = Parser::new("(first (quote (a b c)))");
        let expr = parser.parse().unwrap();
        assert_eq!(eval(&expr, &env), Ok(SExpr::Atom("a".to_string())));
    }

    #[test]
    fn builtin_first_empty_list_error() {
        let mut env = Env::new();
        register_builtins(&mut env);

        let mut parser = Parser::new("(first (quote ()))");
        let expr = parser.parse().unwrap();
        assert!(eval(&expr, &env).is_err());
    }

    #[test]
    fn builtin_rest_returns_tail() {
        let mut env = Env::new();
        register_builtins(&mut env);

        let mut parser = Parser::new("(rest (quote (a b c)))");
        let expr = parser.parse().unwrap();
        let expected = SExpr::List(vec![
            SExpr::Atom("b".to_string()),
            SExpr::Atom("c".to_string()),
        ]);
        assert_eq!(eval(&expr, &env), Ok(expected));
    }

    #[test]
    fn builtin_rest_single_element() {
        let mut env = Env::new();
        register_builtins(&mut env);

        let mut parser = Parser::new("(rest (quote (a)))");
        let expr = parser.parse().unwrap();
        assert_eq!(eval(&expr, &env), Ok(SExpr::List(vec![])));
    }

    #[test]
    fn builtin_cons_prepends() {
        let mut env = Env::new();
        register_builtins(&mut env);

        let mut parser = Parser::new("(cons a (quote (b c)))");
        let expr = parser.parse().unwrap();
        let expected = SExpr::List(vec![
            SExpr::Atom("a".to_string()),
            SExpr::Atom("b".to_string()),
            SExpr::Atom("c".to_string()),
        ]);
        assert_eq!(eval(&expr, &env), Ok(expected));
    }

    #[test]
    fn builtin_cons_to_empty() {
        let mut env = Env::new();
        register_builtins(&mut env);

        let mut parser = Parser::new("(cons a (quote ()))");
        let expr = parser.parse().unwrap();
        let expected = SExpr::List(vec![SExpr::Atom("a".to_string())]);
        assert_eq!(eval(&expr, &env), Ok(expected));
    }

    #[test]
    fn builtin_append_two_lists() {
        let mut env = Env::new();
        register_builtins(&mut env);

        let mut parser = Parser::new("(append (quote (a b)) (quote (c d)))");
        let expr = parser.parse().unwrap();
        let expected = SExpr::List(vec![
            SExpr::Atom("a".to_string()),
            SExpr::Atom("b".to_string()),
            SExpr::Atom("c".to_string()),
            SExpr::Atom("d".to_string()),
        ]);
        assert_eq!(eval(&expr, &env), Ok(expected));
    }

    #[test]
    fn builtin_append_multiple_lists() {
        let mut env = Env::new();
        register_builtins(&mut env);

        let mut parser = Parser::new("(append (quote (a)) (quote (b)) (quote (c)))");
        let expr = parser.parse().unwrap();
        let expected = SExpr::List(vec![
            SExpr::Atom("a".to_string()),
            SExpr::Atom("b".to_string()),
            SExpr::Atom("c".to_string()),
        ]);
        assert_eq!(eval(&expr, &env), Ok(expected));
    }

    #[test]
    fn builtin_length_of_list() {
        let mut env = Env::new();
        register_builtins(&mut env);

        let mut parser = Parser::new("(length (quote (a b c d)))");
        let expr = parser.parse().unwrap();
        assert_eq!(eval(&expr, &env), Ok(SExpr::Atom("4".to_string())));
    }

    #[test]
    fn builtin_length_empty() {
        let mut env = Env::new();
        register_builtins(&mut env);

        let mut parser = Parser::new("(length (quote ()))");
        let expr = parser.parse().unwrap();
        assert_eq!(eval(&expr, &env), Ok(SExpr::Atom("0".to_string())));
    }

    #[test]
    fn builtin_nth_valid_index() {
        let mut env = Env::new();
        register_builtins(&mut env);

        let mut parser = Parser::new("(nth 1 (quote (a b c)))");
        let expr = parser.parse().unwrap();
        assert_eq!(eval(&expr, &env), Ok(SExpr::Atom("b".to_string())));
    }

    #[test]
    fn builtin_nth_out_of_bounds() {
        let mut env = Env::new();
        register_builtins(&mut env);

        let mut parser = Parser::new("(nth 10 (quote (a b c)))");
        let expr = parser.parse().unwrap();
        assert!(eval(&expr, &env).is_err());
    }

    #[test]
    fn builtin_list_creates_list() {
        let mut env = Env::new();
        register_builtins(&mut env);

        let mut parser = Parser::new("(list a b c)");
        let expr = parser.parse().unwrap();
        let expected = SExpr::List(vec![
            SExpr::Atom("a".to_string()),
            SExpr::Atom("b".to_string()),
            SExpr::Atom("c".to_string()),
        ]);
        assert_eq!(eval(&expr, &env), Ok(expected));
    }

    #[test]
    fn builtin_list_empty() {
        let mut env = Env::new();
        register_builtins(&mut env);

        let mut parser = Parser::new("(list)");
        let expr = parser.parse().unwrap();
        assert_eq!(eval(&expr, &env), Ok(SExpr::List(vec![])));
    }

    // Tests for higher-order functions

    fn double(args: &[SExpr]) -> SResult<SExpr> {
        if args.len() != 1 {
            return Err(SError::new("double")
                .with_code("wrong-argument-count")
                .with_message("double requires exactly one argument"));
        }
        match &args[0] {
            SExpr::Atom(s) => {
                let n = s.parse::<i64>().map_err(|_| {
                    SError::new("double")
                        .with_code("invalid-argument")
                        .with_message("Argument must be an integer")
                })?;
                Ok(SExpr::Atom((n * 2).to_string()))
            }
            _ => Err(SError::new("double")
                .with_code("type-error")
                .with_message("Argument must be an atom")),
        }
    }

    fn is_even(args: &[SExpr]) -> SResult<SExpr> {
        if args.len() != 1 {
            return Err(SError::new("is-even")
                .with_code("wrong-argument-count")
                .with_message("is-even requires exactly one argument"));
        }
        match &args[0] {
            SExpr::Atom(s) => {
                let n = s.parse::<i64>().map_err(|_| {
                    SError::new("is-even")
                        .with_code("invalid-argument")
                        .with_message("Argument must be an integer")
                })?;
                Ok(SExpr::Atom(
                    if n % 2 == 0 { "#t" } else { "#f" }.to_string(),
                ))
            }
            _ => Err(SError::new("is-even")
                .with_code("type-error")
                .with_message("Argument must be an atom")),
        }
    }

    #[test]
    fn map_doubles_elements() {
        let mut env = Env::new();
        register_builtins(&mut env);
        env.def_fn("double", double);

        let mut parser = Parser::new("(map double (quote (1 2 3)))");
        let expr = parser.parse().unwrap();
        let expected = SExpr::List(vec![
            SExpr::Atom("2".to_string()),
            SExpr::Atom("4".to_string()),
            SExpr::Atom("6".to_string()),
        ]);
        assert_eq!(eval(&expr, &env), Ok(expected));
    }

    #[test]
    fn map_empty_list() {
        let mut env = Env::new();
        register_builtins(&mut env);
        env.def_fn("double", double);

        let mut parser = Parser::new("(map double (quote ()))");
        let expr = parser.parse().unwrap();
        assert_eq!(eval(&expr, &env), Ok(SExpr::List(vec![])));
    }

    #[test]
    fn filter_keeps_evens() {
        let mut env = Env::new();
        register_builtins(&mut env);
        env.def_fn("is-even", is_even);

        let mut parser = Parser::new("(filter is-even (quote (1 2 3 4 5 6)))");
        let expr = parser.parse().unwrap();
        let expected = SExpr::List(vec![
            SExpr::Atom("2".to_string()),
            SExpr::Atom("4".to_string()),
            SExpr::Atom("6".to_string()),
        ]);
        assert_eq!(eval(&expr, &env), Ok(expected));
    }

    #[test]
    fn filter_empty_list() {
        let mut env = Env::new();
        register_builtins(&mut env);
        env.def_fn("is-even", is_even);

        let mut parser = Parser::new("(filter is-even (quote ()))");
        let expr = parser.parse().unwrap();
        assert_eq!(eval(&expr, &env), Ok(SExpr::List(vec![])));
    }

    #[test]
    fn filter_keeps_none() {
        let mut env = Env::new();
        register_builtins(&mut env);
        env.def_fn("is-even", is_even);

        let mut parser = Parser::new("(filter is-even (quote (1 3 5)))");
        let expr = parser.parse().unwrap();
        assert_eq!(eval(&expr, &env), Ok(SExpr::List(vec![])));
    }

    #[test]
    fn reduce_sum() {
        let mut env = Env::new();
        register_builtins(&mut env);
        env.def_fn("+", add);

        let mut parser = Parser::new("(reduce + 0 (quote (1 2 3 4)))");
        let expr = parser.parse().unwrap();
        // 0 + 1 + 2 + 3 + 4 = 10
        assert_eq!(eval(&expr, &env), Ok(SExpr::Atom("10".to_string())));
    }

    #[test]
    fn reduce_empty_list() {
        let mut env = Env::new();
        register_builtins(&mut env);
        env.def_fn("+", add);

        let mut parser = Parser::new("(reduce + 42 (quote ()))");
        let expr = parser.parse().unwrap();
        // Empty list returns initial value
        assert_eq!(eval(&expr, &env), Ok(SExpr::Atom("42".to_string())));
    }

    #[test]
    fn reduce_with_append_builds_list() {
        let mut env = Env::new();
        register_builtins(&mut env);

        // (reduce append '() '((a) (b) (c))) flattens a list of lists
        let mut parser = Parser::new("(reduce append (quote ()) (quote ((a) (b) (c))))");
        let expr = parser.parse().unwrap();
        // append () (a) = (a)
        // append (a) (b) = (a b)
        // append (a b) (c) = (a b c)
        let expected = SExpr::List(vec![
            SExpr::Atom("a".to_string()),
            SExpr::Atom("b".to_string()),
            SExpr::Atom("c".to_string()),
        ]);
        assert_eq!(eval(&expr, &env), Ok(expected));
    }

    #[test]
    fn reduce_with_multiply() {
        let mut env = Env::new();
        register_builtins(&mut env);
        env.def_fn("*", multiply);

        // (reduce * 1 '(2 3 4)) = 1 * 2 * 3 * 4 = 24
        let mut parser = Parser::new("(reduce * 1 (quote (2 3 4)))");
        let expr = parser.parse().unwrap();
        assert_eq!(eval(&expr, &env), Ok(SExpr::Atom("24".to_string())));
    }

    #[test]
    fn map_filter_combined() {
        let mut env = Env::new();
        register_builtins(&mut env);
        env.def_fn("double", double);
        env.def_fn("is-even", is_even);

        // Double all numbers, then filter for even results
        // All results of double are even, so this keeps all
        let mut parser = Parser::new("(filter is-even (map double (quote (1 2 3))))");
        let expr = parser.parse().unwrap();
        let expected = SExpr::List(vec![
            SExpr::Atom("2".to_string()),
            SExpr::Atom("4".to_string()),
            SExpr::Atom("6".to_string()),
        ]);
        assert_eq!(eval(&expr, &env), Ok(expected));
    }

    #[test]
    fn eval_empty_list() {
        let env = Env::new();
        let expr = SExpr::List(vec![]);
        assert_eq!(eval(&expr, &env), Ok(SExpr::List(vec![])));
    }

    #[test]
    fn eval_quote_wrong_arg_count_error() {
        let env = Env::new();
        let mut parser = Parser::new("(quote a b)");
        let expr = parser.parse().unwrap();
        let err = eval(&expr, &env).unwrap_err();
        assert!(err.to_string().contains("wrong-argument-count"));
    }

    #[test]
    fn eval_if_wrong_arg_count_error() {
        let env = Env::new();
        let mut parser = Parser::new("(if #t then-only)");
        let expr = parser.parse().unwrap();
        let err = eval(&expr, &env).unwrap_err();
        assert!(err.to_string().contains("wrong-argument-count"));
    }

    #[test]
    fn eval_let_invalid_bindings_error() {
        let env = Env::new();
        let mut parser = Parser::new("(let not-a-list x)");
        let expr = parser.parse().unwrap();
        let err = eval(&expr, &env).unwrap_err();
        assert!(err.to_string().contains("invalid-let-bindings"));
    }

    #[test]
    fn eval_let_invalid_binding_pair_error() {
        let env = Env::new();
        let mut parser = Parser::new("(let ((x)) x)");
        let expr = parser.parse().unwrap();
        let err = eval(&expr, &env).unwrap_err();
        assert!(err.to_string().contains("invalid-binding"));
    }

    #[test]
    fn eval_let_non_atom_binding_name_error() {
        let env = Env::new();
        let mut parser = Parser::new("(let (((not-atom) 5)) x)");
        let expr = parser.parse().unwrap();
        let err = eval(&expr, &env).unwrap_err();
        assert!(err.to_string().contains("invalid-binding-name"));
    }

    #[test]
    fn eval_begin_empty_error() {
        let env = Env::new();
        let mut parser = Parser::new("(begin)");
        let expr = parser.parse().unwrap();
        let err = eval(&expr, &env).unwrap_err();
        assert!(err.to_string().contains("wrong-argument-count"));
    }

    #[test]
    fn eval_map_wrong_arg_count_error() {
        let mut env = Env::new();
        register_builtins(&mut env);
        let mut parser = Parser::new("(map double)");
        let expr = parser.parse().unwrap();
        let err = eval(&expr, &env).unwrap_err();
        assert!(err.to_string().contains("wrong-argument-count"));
    }

    #[test]
    fn eval_map_non_atom_function_error() {
        let mut env = Env::new();
        register_builtins(&mut env);
        let mut parser = Parser::new("(map (not-atom) (quote (1 2)))");
        let expr = parser.parse().unwrap();
        let err = eval(&expr, &env).unwrap_err();
        assert!(err.to_string().contains("invalid-function"));
    }

    #[test]
    fn eval_map_non_list_arg_error() {
        let mut env = Env::new();
        register_builtins(&mut env);
        env.def_fn("double", double);
        let mut parser = Parser::new("(map double not-a-list)");
        let expr = parser.parse().unwrap();
        let err = eval(&expr, &env).unwrap_err();
        assert!(err.to_string().contains("type-error"));
    }

    #[test]
    fn eval_filter_non_list_arg_error() {
        let mut env = Env::new();
        register_builtins(&mut env);
        env.def_fn("is-even", is_even);
        let mut parser = Parser::new("(filter is-even not-a-list)");
        let expr = parser.parse().unwrap();
        let err = eval(&expr, &env).unwrap_err();
        assert!(err.to_string().contains("type-error"));
    }

    #[test]
    fn eval_reduce_wrong_arg_count_error() {
        let mut env = Env::new();
        register_builtins(&mut env);
        let mut parser = Parser::new("(reduce + 0)");
        let expr = parser.parse().unwrap();
        let err = eval(&expr, &env).unwrap_err();
        assert!(err.to_string().contains("wrong-argument-count"));
    }

    #[test]
    fn eval_reduce_non_list_arg_error() {
        let mut env = Env::new();
        register_builtins(&mut env);
        env.def_fn("+", add);
        let mut parser = Parser::new("(reduce + 0 not-a-list)");
        let expr = parser.parse().unwrap();
        let err = eval(&expr, &env).unwrap_err();
        assert!(err.to_string().contains("type-error"));
    }

    #[test]
    fn eval_thread_last_empty_error() {
        let env = Env::new();
        let mut parser = Parser::new("(->>)");
        let expr = parser.parse().unwrap();
        let err = eval(&expr, &env).unwrap_err();
        assert!(err.to_string().contains("wrong-argument-count"));
    }

    #[test]
    fn eval_thread_last_invalid_form_error() {
        let env = Env::new();
        let mut parser = Parser::new("(->> 5 ())");
        let expr = parser.parse().unwrap();
        let err = eval(&expr, &env).unwrap_err();
        assert!(err.to_string().contains("invalid-thread-form"));
    }

    #[test]
    fn eval_thread_last_with_atom_func() {
        let mut env = Env::new();
        env.def_fn("double", double);
        let mut parser = Parser::new("(->> 5 double)");
        let expr = parser.parse().unwrap();
        assert_eq!(eval(&expr, &env), Ok(SExpr::Atom("10".to_string())));
    }

    #[test]
    fn eval_if_empty_list_is_falsy() {
        let mut env = Env::new();
        env.def_fn("+", add);
        let mut parser = Parser::new("(if (quote ()) (+ 1 2) (+ 3 4))");
        let expr = parser.parse().unwrap();
        assert_eq!(eval(&expr, &env), Ok(SExpr::Atom("7".to_string())));
    }

    #[test]
    fn eval_if_nonempty_list_is_truthy() {
        let mut env = Env::new();
        env.def_fn("+", add);
        let mut parser = Parser::new("(if (quote (x)) (+ 1 2) (+ 3 4))");
        let expr = parser.parse().unwrap();
        assert_eq!(eval(&expr, &env), Ok(SExpr::Atom("3".to_string())));
    }

    #[test]
    fn builtin_null_p_wrong_arg_count_error() {
        let mut env = Env::new();
        register_builtins(&mut env);
        let mut parser = Parser::new("(null? a b)");
        let expr = parser.parse().unwrap();
        let err = eval(&expr, &env).unwrap_err();
        assert!(err.to_string().contains("wrong-argument-count"));
    }

    #[test]
    fn builtin_first_wrong_arg_count_error() {
        let mut env = Env::new();
        register_builtins(&mut env);
        let mut parser = Parser::new("(first)");
        let expr = parser.parse().unwrap();
        let err = eval(&expr, &env).unwrap_err();
        assert!(err.to_string().contains("wrong-argument-count"));
    }

    #[test]
    fn builtin_first_non_list_error() {
        let mut env = Env::new();
        register_builtins(&mut env);
        let mut parser = Parser::new("(first not-a-list)");
        let expr = parser.parse().unwrap();
        let err = eval(&expr, &env).unwrap_err();
        assert!(err.to_string().contains("type-error"));
    }

    #[test]
    fn builtin_rest_empty_list_error() {
        let mut env = Env::new();
        register_builtins(&mut env);
        let mut parser = Parser::new("(rest (quote ()))");
        let expr = parser.parse().unwrap();
        let err = eval(&expr, &env).unwrap_err();
        assert!(err.to_string().contains("empty-list"));
    }

    #[test]
    fn builtin_cons_non_list_second_arg_error() {
        let mut env = Env::new();
        register_builtins(&mut env);
        let mut parser = Parser::new("(cons a not-a-list)");
        let expr = parser.parse().unwrap();
        let err = eval(&expr, &env).unwrap_err();
        assert!(err.to_string().contains("type-error"));
    }

    #[test]
    fn builtin_append_non_list_error() {
        let mut env = Env::new();
        register_builtins(&mut env);
        let mut parser = Parser::new("(append (quote (a)) not-a-list)");
        let expr = parser.parse().unwrap();
        let err = eval(&expr, &env).unwrap_err();
        assert!(err.to_string().contains("type-error"));
    }

    #[test]
    fn builtin_append_empty() {
        let mut env = Env::new();
        register_builtins(&mut env);
        let mut parser = Parser::new("(append)");
        let expr = parser.parse().unwrap();
        assert_eq!(eval(&expr, &env), Ok(SExpr::List(vec![])));
    }

    #[test]
    fn builtin_length_non_list_error() {
        let mut env = Env::new();
        register_builtins(&mut env);
        let mut parser = Parser::new("(length not-a-list)");
        let expr = parser.parse().unwrap();
        let err = eval(&expr, &env).unwrap_err();
        assert!(err.to_string().contains("type-error"));
    }

    #[test]
    fn builtin_nth_zero_index() {
        let mut env = Env::new();
        register_builtins(&mut env);
        let mut parser = Parser::new("(nth 0 (quote (a b c)))");
        let expr = parser.parse().unwrap();
        assert_eq!(eval(&expr, &env), Ok(SExpr::Atom("a".to_string())));
    }

    #[test]
    fn builtin_nth_invalid_index_error() {
        let mut env = Env::new();
        register_builtins(&mut env);
        let mut parser = Parser::new("(nth not-a-number (quote (a b)))");
        let expr = parser.parse().unwrap();
        let err = eval(&expr, &env).unwrap_err();
        assert!(err.to_string().contains("invalid-index"));
    }

    #[test]
    fn builtin_nth_non_atom_index_error() {
        let mut env = Env::new();
        register_builtins(&mut env);
        let mut parser = Parser::new("(nth (quote (bad)) (quote (a b)))");
        let expr = parser.parse().unwrap();
        let err = eval(&expr, &env).unwrap_err();
        assert!(err.to_string().contains("type-error"));
    }

    #[test]
    fn builtin_nth_non_list_second_arg_error() {
        let mut env = Env::new();
        register_builtins(&mut env);
        let mut parser = Parser::new("(nth 0 not-a-list)");
        let expr = parser.parse().unwrap();
        let err = eval(&expr, &env).unwrap_err();
        assert!(err.to_string().contains("type-error"));
    }

    #[test]
    fn eval_non_atom_function_name_error() {
        let env = Env::new();
        let mut parser = Parser::new("((not-an-atom) 1 2)");
        let expr = parser.parse().unwrap();
        let err = eval(&expr, &env).unwrap_err();
        assert!(err.to_string().contains("invalid-function-name"));
    }

    #[test]
    fn env_child_inherits_bindings() {
        let mut env = Env::new();
        env.def_fn("+", add);
        let mut parser = Parser::new("(let ((x 10)) (let ((y 20)) (+ x y)))");
        let expr = parser.parse().unwrap();
        assert_eq!(eval(&expr, &env), Ok(SExpr::Atom("30".to_string())));
    }

    #[test]
    fn env_child_inherits_functions() {
        let mut env = Env::new();
        env.def_fn("+", add);
        let mut parser = Parser::new("(let ((x 5)) (+ x 3))");
        let expr = parser.parse().unwrap();
        assert_eq!(eval(&expr, &env), Ok(SExpr::Atom("8".to_string())));
    }
}

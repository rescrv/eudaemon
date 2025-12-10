//! Stackless virtual machine for S-expression evaluation.
//!
//! This module implements a VM with an explicit call stack, enabling features like:
//! - Steppable execution for debugging
//! - Restarts for error recovery without unwinding
//! - Hot-swapping of function definitions
//!
//! The VM stores functions in an arena and uses `FunctionId` references, allowing
//! function redefinition to affect all existing references.

use std::collections::HashMap;
use std::sync::Arc;

use crate::docs::get_help;
use crate::error::{SError, SResult};
use crate::expr::SExpr;
use crate::object::{assoc, dissoc, get, keys, merge, values};
use crate::util::string_atom;

/// Unique identifier for a function in the arena.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FunctionId(usize);

/// A runtime value in the VM.
///
/// TODO(user): This type is scaffolding for first-class functions. Currently unused because
/// the VM only passes SExpr values. When implementing user-defined lambdas (defun), this
/// type will be needed to distinguish between S-expression data and function references.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum Value {
    /// An S-expression value (atoms and lists).
    SExpr(SExpr),
    /// A reference to a function in the arena.
    Function(FunctionId),
}

impl From<SExpr> for Value {
    fn from(expr: SExpr) -> Self {
        Value::SExpr(expr)
    }
}

#[allow(dead_code)]
impl Value {
    /// Converts the value to an SExpr, panicking if it's a function reference.
    pub fn into_sexpr(self) -> SExpr {
        match self {
            Value::SExpr(e) => e,
            Value::Function(id) => panic!("Cannot convert FunctionId {:?} to SExpr", id),
        }
    }

    /// Tries to convert the value to an SExpr.
    pub fn as_sexpr(&self) -> Option<&SExpr> {
        match self {
            Value::SExpr(e) => Some(e),
            Value::Function(_) => None,
        }
    }
}

/// Function signature for built-in functions.
pub type BuiltinFn = fn(&[SExpr]) -> SResult<SExpr>;

/// A function stored in the arena.
#[derive(Clone)]
pub enum FunctionObj {
    /// A built-in function implemented in Rust.
    Builtin {
        /// Name of the function for error reporting.
        name: String,
        /// The function implementation.
        func: BuiltinFn,
    },
    /// A user-defined lambda (future extension).
    ///
    /// TODO(user): Implement `(lambda (params) body)` or `(defun name (params) body)` special
    /// forms in the VM to construct this variant. The scaffolding for evaluation exists in
    /// `step_call` but no syntax currently produces lambdas.
    #[allow(dead_code)]
    Lambda {
        /// Parameter names.
        params: Vec<String>,
        /// Body expression.
        body: SExpr,
        /// Captured environment (lexical scope).
        env: Arc<Environment>,
    },
}

/// A condition that caused the VM to suspend.
///
/// Conditions represent recoverable errors that can be handled by restarts.
#[derive(Debug, Clone)]
pub enum Condition {
    /// A function was not found in the environment.
    FunctionNotFound {
        /// The name that was looked up.
        name: String,
    },
    /// A type error occurred.
    TypeError {
        /// Description of the expected type.
        expected: String,
        /// The actual value received.
        actual: SExpr,
    },
    /// Wrong number of arguments passed to a function.
    WrongArgumentCount {
        /// Name of the function.
        function: String,
        /// Expected argument count.
        expected: usize,
        /// Actual argument count.
        actual: usize,
    },
    /// A custom error condition.
    Custom {
        /// Error code.
        code: String,
        /// Error message.
        message: String,
    },
}

/// A restart option for recovering from a condition.
#[derive(Debug, Clone)]
pub enum Restart {
    /// Abort the current computation.
    Abort,
    /// Use a specific value instead of the erroring expression.
    UseValue(SExpr),
    /// Retry the operation (possibly after fixing state).
    Retry,
}

/// The state of the VM after a step.
#[derive(Debug, Clone)]
pub enum VmState {
    /// The VM is still running.
    Running,
    /// The VM has finished with a result.
    Finished(SExpr),
    /// The VM is suspended on a condition.
    Suspended(Condition),
}

/// The operation being performed by a frame.
#[derive(Debug, Clone)]
enum FrameOp {
    /// Evaluating an expression.
    Eval(SExpr),
    /// Evaluating a function call with evaluated arguments.
    Call {
        /// Function name.
        func_name: String,
        /// Arguments (some may be unevaluated).
        args: Vec<SExpr>,
        /// Number of arguments already evaluated.
        evaluated_count: usize,
    },
    /// Evaluating a special form.
    SpecialForm {
        /// The special form name.
        name: String,
        /// The original arguments.
        args: Vec<SExpr>,
        /// Current state within the special form.
        state: SpecialFormState,
    },
}

/// State within a special form's multi-step evaluation.
#[derive(Debug, Clone)]
enum SpecialFormState {
    /// Waiting for a sub-expression to be evaluated.
    WaitingForValue,
    /// Processing index in a sequence.
    Index(usize),
}

/// A single frame on the VM's call stack.
#[derive(Debug, Clone)]
struct Frame {
    /// The operation this frame is performing.
    op: FrameOp,
    /// Local variable bindings for this frame.
    locals: HashMap<String, SExpr>,
}

impl Frame {
    fn new(op: FrameOp) -> Self {
        Frame {
            op,
            locals: HashMap::new(),
        }
    }

    fn with_locals(op: FrameOp, locals: HashMap<String, SExpr>) -> Self {
        Frame { op, locals }
    }
}

/// Lexical environment for variable bindings.
#[derive(Debug, Clone, Default)]
pub struct Environment {
    /// Variable bindings in this scope.
    bindings: HashMap<String, SExpr>,
    /// Parent environment for lexical scoping.
    parent: Option<Arc<Environment>>,
}

impl Environment {
    /// Creates a new empty environment.
    pub fn new() -> Self {
        Environment {
            bindings: HashMap::new(),
            parent: None,
        }
    }

    /// Creates a child environment with this as parent.
    ///
    /// TODO(user): This method will be used when implementing user-defined lambdas that
    /// capture their lexical environment.
    #[allow(dead_code)]
    pub fn child(self: &Arc<Self>) -> Self {
        Environment {
            bindings: HashMap::new(),
            parent: Some(Arc::clone(self)),
        }
    }

    /// Binds a variable in this environment.
    pub fn bind(&mut self, name: &str, value: SExpr) {
        self.bindings.insert(name.to_string(), value);
    }

    /// Looks up a variable, searching parent environments.
    pub fn lookup(&self, name: &str) -> Option<SExpr> {
        self.bindings
            .get(name)
            .cloned()
            .or_else(|| self.parent.as_ref().and_then(|p| p.lookup(name)))
    }
}

/// The stackless virtual machine.
pub struct Vm {
    /// The explicit call stack.
    frames: Vec<Frame>,
    /// Function arena: maps FunctionId to FunctionObj.
    functions: HashMap<FunctionId, FunctionObj>,
    /// Name to FunctionId mapping for lookup.
    function_names: HashMap<String, FunctionId>,
    /// Next available function ID.
    next_func_id: usize,
    /// The current result (set when a frame completes).
    current_result: Option<SExpr>,
}

impl Default for Vm {
    fn default() -> Self {
        Self::new()
    }
}

impl Vm {
    /// Creates a new empty VM.
    pub fn new() -> Self {
        Vm {
            frames: Vec::new(),
            functions: HashMap::new(),
            function_names: HashMap::new(),
            next_func_id: 0,
            current_result: None,
        }
    }

    /// Defines a built-in function.
    pub fn def_fn(&mut self, name: &str, func: BuiltinFn) {
        let id = FunctionId(self.next_func_id);
        self.next_func_id += 1;
        self.functions.insert(
            id,
            FunctionObj::Builtin {
                name: name.to_string(),
                func,
            },
        );
        self.function_names.insert(name.to_string(), id);
    }

    /// Registers the core list-manipulation builtins.
    pub fn register_builtins(&mut self) {
        self.def_fn("null?", null_p);
        self.def_fn("list?", list_p);
        self.def_fn("atom?", atom_p);
        self.def_fn("empty?", empty_p);
        self.def_fn("eq?", eq_p);
        self.def_fn("first", first);
        self.def_fn("rest", rest);
        self.def_fn("cons", cons);
        self.def_fn("append", append);
        self.def_fn("length", length);
        self.def_fn("nth", nth);
        self.def_fn("list", list);
        self.def_fn("help", help);
    }

    /// Registers JSON object manipulation builtins.
    pub fn register_json_builtins(&mut self) {
        self.def_fn("obj", obj);
        self.def_fn("arr", arr);
        self.def_fn("get", builtin_get);
        self.def_fn("keys", builtin_keys);
        self.def_fn("values", builtin_values);
        self.def_fn("assoc", builtin_assoc);
        self.def_fn("dissoc", builtin_dissoc);
        self.def_fn("merge", builtin_merge);
    }

    /// Looks up a function by name.
    fn lookup_fn(&self, name: &str) -> Option<&FunctionObj> {
        self.function_names
            .get(name)
            .and_then(|id| self.functions.get(id))
    }

    /// Loads an expression for evaluation.
    pub fn load(&mut self, expr: SExpr) {
        self.frames.clear();
        self.current_result = None;
        self.frames.push(Frame::new(FrameOp::Eval(expr)));
    }

    /// Resets the VM state.
    pub fn reset(&mut self) {
        self.frames.clear();
        self.current_result = None;
    }

    /// Pushes a result value (used for restarts).
    pub fn push_result(&mut self, value: SExpr) {
        self.current_result = Some(value);
    }

    /// Advances the VM by one step.
    pub fn step(&mut self) -> VmState {
        if let Some(frame) = self.frames.pop() {
            match self.step_frame(frame) {
                Ok(state) => state,
                Err(condition) => VmState::Suspended(condition),
            }
        } else {
            match self.current_result.take() {
                Some(result) => VmState::Finished(result),
                None => VmState::Finished(SExpr::List(vec![])),
            }
        }
    }

    /// Runs the VM to completion or suspension.
    pub fn run(&mut self) -> VmState {
        loop {
            match self.step() {
                VmState::Running => continue,
                other => return other,
            }
        }
    }

    /// Evaluates an expression to completion.
    pub fn eval(&mut self, expr: &SExpr) -> SResult<SExpr> {
        self.load(expr.clone());
        match self.run() {
            VmState::Finished(result) => Ok(result),
            VmState::Suspended(condition) => Err(condition_to_error(condition)),
            VmState::Running => unreachable!("run() should not return Running"),
        }
    }

    /// Steps a single frame.
    fn step_frame(&mut self, frame: Frame) -> Result<VmState, Condition> {
        match frame.op {
            FrameOp::Eval(expr) => self.step_eval(expr, frame.locals),
            FrameOp::Call {
                func_name,
                args,
                evaluated_count,
            } => self.step_call(func_name, args, evaluated_count, frame.locals),
            FrameOp::SpecialForm { name, args, state } => {
                self.step_special_form(name, args, state, frame.locals)
            }
        }
    }

    /// Steps an Eval frame.
    fn step_eval(
        &mut self,
        expr: SExpr,
        locals: HashMap<String, SExpr>,
    ) -> Result<VmState, Condition> {
        match expr {
            SExpr::Atom(ref s) => {
                // Check for variable binding
                if let Some(value) = locals.get(s) {
                    self.current_result = Some(value.clone());
                    return Ok(VmState::Running);
                }
                // Atom evaluates to itself
                self.current_result = Some(SExpr::Atom(s.clone()));
                Ok(VmState::Running)
            }
            SExpr::List(items) => {
                if items.is_empty() {
                    self.current_result = Some(SExpr::List(vec![]));
                    return Ok(VmState::Running);
                }

                let func_name = match &items[0] {
                    SExpr::Atom(s) => s.clone(),
                    _ => {
                        return Err(Condition::Custom {
                            code: "invalid-function-name".to_string(),
                            message: "First element of list must be an atom".to_string(),
                        });
                    }
                };

                let args: Vec<SExpr> = items[1..].to_vec();

                // Check for special forms
                match func_name.as_str() {
                    "quote" | "if" | "let" | "begin" | "->" | "->>" | "map" | "filter"
                    | "reduce" => {
                        self.frames.push(Frame::with_locals(
                            FrameOp::SpecialForm {
                                name: func_name,
                                args,
                                state: SpecialFormState::WaitingForValue,
                            },
                            locals,
                        ));
                        Ok(VmState::Running)
                    }
                    _ => {
                        // Regular function call - start evaluating arguments
                        self.frames.push(Frame::with_locals(
                            FrameOp::Call {
                                func_name,
                                args,
                                evaluated_count: 0,
                            },
                            locals,
                        ));
                        Ok(VmState::Running)
                    }
                }
            }
        }
    }

    /// Steps a Call frame (function application).
    fn step_call(
        &mut self,
        func_name: String,
        mut args: Vec<SExpr>,
        evaluated_count: usize,
        locals: HashMap<String, SExpr>,
    ) -> Result<VmState, Condition> {
        // If we have a result from a previous step, store it
        if let Some(result) = self.current_result.take() {
            if evaluated_count > 0 {
                args[evaluated_count - 1] = result;
            }
        }

        // If there are more arguments to evaluate
        if evaluated_count < args.len() {
            // Push the call frame back with incremented count
            self.frames.push(Frame::with_locals(
                FrameOp::Call {
                    func_name,
                    args: args.clone(),
                    evaluated_count: evaluated_count + 1,
                },
                locals.clone(),
            ));

            // Push an eval frame for the next argument
            self.frames.push(Frame::with_locals(
                FrameOp::Eval(args[evaluated_count].clone()),
                locals,
            ));

            return Ok(VmState::Running);
        }

        // All arguments evaluated - call the function
        let func = self.lookup_fn(&func_name).cloned();
        match func {
            Some(FunctionObj::Builtin { func, .. }) => match func(&args) {
                Ok(result) => {
                    self.current_result = Some(result);
                    Ok(VmState::Running)
                }
                Err(e) => Err(Condition::Custom {
                    code: "builtin-error".to_string(),
                    message: e.to_string(),
                }),
            },
            Some(FunctionObj::Lambda { .. }) => {
                // TODO(user): Implement lambda invocation
                Err(Condition::Custom {
                    code: "not-implemented".to_string(),
                    message: "Lambda invocation not yet implemented".to_string(),
                })
            }
            None => Err(Condition::FunctionNotFound { name: func_name }),
        }
    }

    /// Steps a SpecialForm frame.
    fn step_special_form(
        &mut self,
        name: String,
        args: Vec<SExpr>,
        state: SpecialFormState,
        locals: HashMap<String, SExpr>,
    ) -> Result<VmState, Condition> {
        match name.as_str() {
            "quote" => self.step_quote(args),
            "if" => self.step_if(args, state, locals),
            "let" => self.step_let(args, state, locals),
            "begin" => self.step_begin(args, state, locals),
            "->" => self.step_thread_first(args, state, locals),
            "->>" => self.step_thread_last(args, state, locals),
            "map" => self.step_map(args, state, locals),
            "filter" => self.step_filter(args, state, locals),
            "reduce" => self.step_reduce(args, state, locals),
            _ => Err(Condition::Custom {
                code: "unknown-special-form".to_string(),
                message: format!("Unknown special form: {}", name),
            }),
        }
    }

    /// Handles (quote expr).
    fn step_quote(&mut self, args: Vec<SExpr>) -> Result<VmState, Condition> {
        if args.len() != 1 {
            return Err(Condition::WrongArgumentCount {
                function: "quote".to_string(),
                expected: 1,
                actual: args.len(),
            });
        }
        self.current_result = Some(args[0].clone());
        Ok(VmState::Running)
    }

    /// Handles (if cond then else).
    fn step_if(
        &mut self,
        args: Vec<SExpr>,
        state: SpecialFormState,
        locals: HashMap<String, SExpr>,
    ) -> Result<VmState, Condition> {
        if args.len() != 3 {
            return Err(Condition::WrongArgumentCount {
                function: "if".to_string(),
                expected: 3,
                actual: args.len(),
            });
        }

        match state {
            SpecialFormState::WaitingForValue => {
                // Evaluate the condition
                self.frames.push(Frame::with_locals(
                    FrameOp::SpecialForm {
                        name: "if".to_string(),
                        args,
                        state: SpecialFormState::Index(0),
                    },
                    locals.clone(),
                ));
                self.frames
                    .push(Frame::with_locals(FrameOp::Eval(args[0].clone()), locals));
                Ok(VmState::Running)
            }
            SpecialFormState::Index(0) => {
                // Condition evaluated, decide which branch
                let cond = self.current_result.take().unwrap_or(SExpr::List(vec![]));
                let branch = if is_truthy(&cond) {
                    args[1].clone()
                } else {
                    args[2].clone()
                };
                self.frames
                    .push(Frame::with_locals(FrameOp::Eval(branch), locals));
                Ok(VmState::Running)
            }
            _ => Ok(VmState::Running),
        }
    }

    /// Handles (let ((var val) ...) body...).
    fn step_let(
        &mut self,
        args: Vec<SExpr>,
        state: SpecialFormState,
        mut locals: HashMap<String, SExpr>,
    ) -> Result<VmState, Condition> {
        if args.len() < 2 {
            return Err(Condition::WrongArgumentCount {
                function: "let".to_string(),
                expected: 2,
                actual: args.len(),
            });
        }

        let bindings = match &args[0] {
            SExpr::List(b) => b.clone(),
            _ => {
                return Err(Condition::Custom {
                    code: "invalid-let-bindings".to_string(),
                    message: "let bindings must be a list".to_string(),
                });
            }
        };

        match state {
            SpecialFormState::WaitingForValue => {
                if bindings.is_empty() {
                    // No bindings, evaluate body directly
                    self.frames.push(Frame::with_locals(
                        FrameOp::SpecialForm {
                            name: "let".to_string(),
                            args,
                            state: SpecialFormState::Index(bindings.len()),
                        },
                        locals,
                    ));
                } else {
                    // Start evaluating first binding
                    let first_binding = &bindings[0];
                    if let SExpr::List(pair) = first_binding {
                        if pair.len() != 2 {
                            return Err(Condition::Custom {
                                code: "invalid-binding".to_string(),
                                message: "Each binding must be (var value)".to_string(),
                            });
                        }
                        self.frames.push(Frame::with_locals(
                            FrameOp::SpecialForm {
                                name: "let".to_string(),
                                args,
                                state: SpecialFormState::Index(0),
                            },
                            locals.clone(),
                        ));
                        self.frames
                            .push(Frame::with_locals(FrameOp::Eval(pair[1].clone()), locals));
                    } else {
                        return Err(Condition::Custom {
                            code: "invalid-binding".to_string(),
                            message: "Each binding must be (var value)".to_string(),
                        });
                    }
                }
                Ok(VmState::Running)
            }
            SpecialFormState::Index(idx) => {
                // Store the evaluated value
                if idx < bindings.len() {
                    if let Some(result) = self.current_result.take() {
                        if let SExpr::List(pair) = &bindings[idx] {
                            if let SExpr::Atom(var_name) = &pair[0] {
                                locals.insert(var_name.clone(), result);
                            } else {
                                return Err(Condition::Custom {
                                    code: "invalid-binding-name".to_string(),
                                    message: "Binding name must be an atom".to_string(),
                                });
                            }
                        }
                    }

                    // Check if there are more bindings
                    if idx + 1 < bindings.len() {
                        let next_binding = &bindings[idx + 1];
                        if let SExpr::List(pair) = next_binding {
                            if pair.len() != 2 {
                                return Err(Condition::Custom {
                                    code: "invalid-binding".to_string(),
                                    message: "Each binding must be (var value)".to_string(),
                                });
                            }
                            self.frames.push(Frame::with_locals(
                                FrameOp::SpecialForm {
                                    name: "let".to_string(),
                                    args,
                                    state: SpecialFormState::Index(idx + 1),
                                },
                                locals.clone(),
                            ));
                            self.frames
                                .push(Frame::with_locals(FrameOp::Eval(pair[1].clone()), locals));
                        }
                        return Ok(VmState::Running);
                    }
                }

                // All bindings processed, evaluate body expressions
                let body = &args[1..];
                if body.is_empty() {
                    self.current_result = Some(SExpr::List(vec![]));
                } else if body.len() == 1 {
                    self.frames
                        .push(Frame::with_locals(FrameOp::Eval(body[0].clone()), locals));
                } else {
                    // Multiple body expressions - use begin
                    self.frames.push(Frame::with_locals(
                        FrameOp::SpecialForm {
                            name: "begin".to_string(),
                            args: body.to_vec(),
                            state: SpecialFormState::Index(0),
                        },
                        locals.clone(),
                    ));
                    self.frames
                        .push(Frame::with_locals(FrameOp::Eval(body[0].clone()), locals));
                }
                Ok(VmState::Running)
            }
        }
    }

    /// Handles (begin expr...).
    fn step_begin(
        &mut self,
        args: Vec<SExpr>,
        state: SpecialFormState,
        locals: HashMap<String, SExpr>,
    ) -> Result<VmState, Condition> {
        if args.is_empty() {
            return Err(Condition::WrongArgumentCount {
                function: "begin".to_string(),
                expected: 1,
                actual: 0,
            });
        }

        match state {
            SpecialFormState::WaitingForValue => {
                // Start evaluating first expression
                self.frames.push(Frame::with_locals(
                    FrameOp::SpecialForm {
                        name: "begin".to_string(),
                        args,
                        state: SpecialFormState::Index(0),
                    },
                    locals.clone(),
                ));
                self.frames
                    .push(Frame::with_locals(FrameOp::Eval(args[0].clone()), locals));
                Ok(VmState::Running)
            }
            SpecialFormState::Index(idx) => {
                if idx + 1 < args.len() {
                    // More expressions to evaluate
                    self.frames.push(Frame::with_locals(
                        FrameOp::SpecialForm {
                            name: "begin".to_string(),
                            args,
                            state: SpecialFormState::Index(idx + 1),
                        },
                        locals.clone(),
                    ));
                    self.frames.push(Frame::with_locals(
                        FrameOp::Eval(args[idx + 1].clone()),
                        locals,
                    ));
                }
                // Last expression's result is already in current_result
                Ok(VmState::Running)
            }
        }
    }

    /// Handles (-> value form...).
    fn step_thread_first(
        &mut self,
        args: Vec<SExpr>,
        state: SpecialFormState,
        locals: HashMap<String, SExpr>,
    ) -> Result<VmState, Condition> {
        if args.is_empty() {
            return Err(Condition::WrongArgumentCount {
                function: "->".to_string(),
                expected: 1,
                actual: 0,
            });
        }

        match state {
            SpecialFormState::WaitingForValue => {
                // Evaluate the initial value
                self.frames.push(Frame::with_locals(
                    FrameOp::SpecialForm {
                        name: "->".to_string(),
                        args,
                        state: SpecialFormState::Index(0),
                    },
                    locals.clone(),
                ));
                self.frames
                    .push(Frame::with_locals(FrameOp::Eval(args[0].clone()), locals));
                Ok(VmState::Running)
            }
            SpecialFormState::Index(idx) => {
                let value = self.current_result.take().unwrap_or(SExpr::List(vec![]));

                if idx + 1 >= args.len() {
                    // No more forms, return the value
                    self.current_result = Some(value);
                    return Ok(VmState::Running);
                }

                // Apply next form with value as first argument
                let form = &args[idx + 1];
                let threaded = thread_into(form, value, true);

                self.frames.push(Frame::with_locals(
                    FrameOp::SpecialForm {
                        name: "->".to_string(),
                        args,
                        state: SpecialFormState::Index(idx + 1),
                    },
                    locals.clone(),
                ));
                self.frames
                    .push(Frame::with_locals(FrameOp::Eval(threaded), locals));
                Ok(VmState::Running)
            }
        }
    }

    /// Handles (->> value form...).
    fn step_thread_last(
        &mut self,
        args: Vec<SExpr>,
        state: SpecialFormState,
        locals: HashMap<String, SExpr>,
    ) -> Result<VmState, Condition> {
        if args.is_empty() {
            return Err(Condition::WrongArgumentCount {
                function: "->>".to_string(),
                expected: 1,
                actual: 0,
            });
        }

        match state {
            SpecialFormState::WaitingForValue => {
                // Evaluate the initial value
                self.frames.push(Frame::with_locals(
                    FrameOp::SpecialForm {
                        name: "->>".to_string(),
                        args,
                        state: SpecialFormState::Index(0),
                    },
                    locals.clone(),
                ));
                self.frames
                    .push(Frame::with_locals(FrameOp::Eval(args[0].clone()), locals));
                Ok(VmState::Running)
            }
            SpecialFormState::Index(idx) => {
                let value = self.current_result.take().unwrap_or(SExpr::List(vec![]));

                if idx + 1 >= args.len() {
                    // No more forms, return the value
                    self.current_result = Some(value);
                    return Ok(VmState::Running);
                }

                // Apply next form with value as last argument
                let form = &args[idx + 1];
                let threaded = thread_into(form, value, false);

                self.frames.push(Frame::with_locals(
                    FrameOp::SpecialForm {
                        name: "->>".to_string(),
                        args,
                        state: SpecialFormState::Index(idx + 1),
                    },
                    locals.clone(),
                ));
                self.frames
                    .push(Frame::with_locals(FrameOp::Eval(threaded), locals));
                Ok(VmState::Running)
            }
        }
    }

    /// Handles (map func list).
    fn step_map(
        &mut self,
        args: Vec<SExpr>,
        state: SpecialFormState,
        locals: HashMap<String, SExpr>,
    ) -> Result<VmState, Condition> {
        if args.len() != 2 {
            return Err(Condition::WrongArgumentCount {
                function: "map".to_string(),
                expected: 2,
                actual: args.len(),
            });
        }

        let func_name = match &args[0] {
            SExpr::Atom(s) => s.clone(),
            _ => {
                return Err(Condition::Custom {
                    code: "invalid-function".to_string(),
                    message: "map requires a function name as first argument".to_string(),
                });
            }
        };

        match state {
            SpecialFormState::WaitingForValue => {
                // Evaluate the list argument
                self.frames.push(Frame::with_locals(
                    FrameOp::SpecialForm {
                        name: "map".to_string(),
                        args,
                        state: SpecialFormState::Index(0),
                    },
                    locals.clone(),
                ));
                self.frames
                    .push(Frame::with_locals(FrameOp::Eval(args[1].clone()), locals));
                Ok(VmState::Running)
            }
            SpecialFormState::Index(0) => {
                let list = self.current_result.take().unwrap_or(SExpr::List(vec![]));
                let items = match list {
                    SExpr::List(items) => items,
                    _ => {
                        return Err(Condition::TypeError {
                            expected: "list".to_string(),
                            actual: list,
                        });
                    }
                };

                if items.is_empty() {
                    self.current_result = Some(SExpr::List(vec![]));
                    return Ok(VmState::Running);
                }

                // Build a list of function applications and evaluate
                let mapped: Vec<SExpr> = items
                    .into_iter()
                    .map(|item| {
                        SExpr::List(vec![
                            SExpr::Atom(func_name.clone()),
                            SExpr::List(vec![SExpr::Atom("quote".to_string()), item]),
                        ])
                    })
                    .collect();

                // Evaluate as (list (f item1) (f item2) ...)
                let list_expr = SExpr::List(
                    std::iter::once(SExpr::Atom("list".to_string()))
                        .chain(mapped)
                        .collect(),
                );

                self.frames
                    .push(Frame::with_locals(FrameOp::Eval(list_expr), locals));
                Ok(VmState::Running)
            }
            _ => Ok(VmState::Running),
        }
    }

    /// Handles (filter func list).
    fn step_filter(
        &mut self,
        args: Vec<SExpr>,
        state: SpecialFormState,
        locals: HashMap<String, SExpr>,
    ) -> Result<VmState, Condition> {
        if args.len() != 2 {
            return Err(Condition::WrongArgumentCount {
                function: "filter".to_string(),
                expected: 2,
                actual: args.len(),
            });
        }

        let func_name = match &args[0] {
            SExpr::Atom(s) => s.clone(),
            _ => {
                return Err(Condition::Custom {
                    code: "invalid-function".to_string(),
                    message: "filter requires a function name as first argument".to_string(),
                });
            }
        };

        match state {
            SpecialFormState::WaitingForValue => {
                // Evaluate the list argument
                self.frames.push(Frame::with_locals(
                    FrameOp::SpecialForm {
                        name: "filter".to_string(),
                        args,
                        state: SpecialFormState::Index(0),
                    },
                    locals.clone(),
                ));
                self.frames
                    .push(Frame::with_locals(FrameOp::Eval(args[1].clone()), locals));
                Ok(VmState::Running)
            }
            SpecialFormState::Index(0) => {
                let list = self.current_result.take().unwrap_or(SExpr::List(vec![]));
                let items = match list {
                    SExpr::List(items) => items,
                    _ => {
                        return Err(Condition::TypeError {
                            expected: "list".to_string(),
                            actual: list,
                        });
                    }
                };

                if items.is_empty() {
                    self.current_result = Some(SExpr::List(vec![]));
                    return Ok(VmState::Running);
                }

                // We need to evaluate each predicate and filter synchronously
                // For simplicity, we do this eagerly here
                let func = self.lookup_fn(&func_name).cloned();
                match func {
                    Some(FunctionObj::Builtin { func, .. }) => {
                        let mut result = Vec::new();
                        for item in items {
                            match func(&[item.clone()]) {
                                Ok(pred_result) => {
                                    if is_truthy(&pred_result) {
                                        result.push(item);
                                    }
                                }
                                Err(e) => {
                                    return Err(Condition::Custom {
                                        code: "filter-error".to_string(),
                                        message: e.to_string(),
                                    });
                                }
                            }
                        }
                        self.current_result = Some(SExpr::List(result));
                        Ok(VmState::Running)
                    }
                    _ => Err(Condition::FunctionNotFound { name: func_name }),
                }
            }
            _ => Ok(VmState::Running),
        }
    }

    /// Handles (reduce func init list).
    fn step_reduce(
        &mut self,
        args: Vec<SExpr>,
        state: SpecialFormState,
        locals: HashMap<String, SExpr>,
    ) -> Result<VmState, Condition> {
        if args.len() != 3 {
            return Err(Condition::WrongArgumentCount {
                function: "reduce".to_string(),
                expected: 3,
                actual: args.len(),
            });
        }

        let func_name = match &args[0] {
            SExpr::Atom(s) => s.clone(),
            _ => {
                return Err(Condition::Custom {
                    code: "invalid-function".to_string(),
                    message: "reduce requires a function name as first argument".to_string(),
                });
            }
        };

        match state {
            SpecialFormState::WaitingForValue => {
                // Evaluate the initial value
                self.frames.push(Frame::with_locals(
                    FrameOp::SpecialForm {
                        name: "reduce".to_string(),
                        args,
                        state: SpecialFormState::Index(0),
                    },
                    locals.clone(),
                ));
                self.frames
                    .push(Frame::with_locals(FrameOp::Eval(args[1].clone()), locals));
                Ok(VmState::Running)
            }
            SpecialFormState::Index(0) => {
                let init = self.current_result.take().unwrap_or(SExpr::List(vec![]));
                // Now evaluate the list
                // Store init in locals temporarily
                let mut new_locals = locals.clone();
                new_locals.insert("__reduce_acc__".to_string(), init);

                self.frames.push(Frame::with_locals(
                    FrameOp::SpecialForm {
                        name: "reduce".to_string(),
                        args,
                        state: SpecialFormState::Index(1),
                    },
                    new_locals.clone(),
                ));
                self.frames.push(Frame::with_locals(
                    FrameOp::Eval(args[2].clone()),
                    new_locals,
                ));
                Ok(VmState::Running)
            }
            SpecialFormState::Index(1) => {
                let list = self.current_result.take().unwrap_or(SExpr::List(vec![]));
                let items = match list {
                    SExpr::List(items) => items,
                    _ => {
                        return Err(Condition::TypeError {
                            expected: "list".to_string(),
                            actual: list,
                        });
                    }
                };

                let init = locals
                    .get("__reduce_acc__")
                    .cloned()
                    .unwrap_or(SExpr::List(vec![]));

                if items.is_empty() {
                    self.current_result = Some(init);
                    return Ok(VmState::Running);
                }

                // Eagerly reduce
                let func = self.lookup_fn(&func_name).cloned();
                match func {
                    Some(FunctionObj::Builtin { func, .. }) => {
                        let mut acc = init;
                        for item in items {
                            match func(&[acc, item]) {
                                Ok(result) => acc = result,
                                Err(e) => {
                                    return Err(Condition::Custom {
                                        code: "reduce-error".to_string(),
                                        message: e.to_string(),
                                    });
                                }
                            }
                        }
                        self.current_result = Some(acc);
                        Ok(VmState::Running)
                    }
                    _ => Err(Condition::FunctionNotFound { name: func_name }),
                }
            }
            _ => Ok(VmState::Running),
        }
    }
}

/// Threads a value into a form.
fn thread_into(form: &SExpr, value: SExpr, first: bool) -> SExpr {
    match form {
        SExpr::Atom(func_name) => {
            // Bare function name: (func value)
            SExpr::List(vec![
                SExpr::Atom(func_name.clone()),
                SExpr::List(vec![SExpr::Atom("quote".to_string()), value]),
            ])
        }
        SExpr::List(items) if !items.is_empty() => {
            let mut new_items = vec![items[0].clone()];
            if first {
                new_items.push(SExpr::List(vec![SExpr::Atom("quote".to_string()), value]));
                new_items.extend(items[1..].iter().cloned());
            } else {
                new_items.extend(items[1..].iter().cloned());
                new_items.push(SExpr::List(vec![SExpr::Atom("quote".to_string()), value]));
            }
            SExpr::List(new_items)
        }
        _ => form.clone(),
    }
}

/// Determines if a value is truthy.
fn is_truthy(expr: &SExpr) -> bool {
    match expr {
        SExpr::Atom(s) => s != "#f" && s != "null",
        SExpr::List(items) => !items.is_empty(),
    }
}

/// Converts a Condition to an SError.
fn condition_to_error(condition: Condition) -> SError {
    match condition {
        Condition::FunctionNotFound { name } => SError::new("vm")
            .with_code("function-not-found")
            .with_message("Function not found")
            .with_string_field("name", &name),
        Condition::TypeError { expected, actual } => SError::new("vm")
            .with_code("type-error")
            .with_message("Type error")
            .with_string_field("expected", &expected)
            .with_field("actual", actual),
        Condition::WrongArgumentCount {
            function,
            expected,
            actual,
        } => SError::new("vm")
            .with_code("wrong-argument-count")
            .with_message("Wrong number of arguments")
            .with_string_field("function", &function)
            .with_atom_field("expected", expected)
            .with_atom_field("actual", actual),
        Condition::Custom { code, message } => {
            SError::new("vm").with_code(&code).with_message(&message)
        }
    }
}

// ============================================================================
// Built-in functions
// ============================================================================

fn null_p(args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 1 {
        return Err(SError::new("vm")
            .with_code("wrong-argument-count")
            .with_message("null? requires exactly one argument"));
    }
    let result = matches!(&args[0], SExpr::Atom(s) if s == "null");
    Ok(SExpr::Atom(if result { "#t" } else { "#f" }.to_string()))
}

fn list_p(args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 1 {
        return Err(SError::new("vm")
            .with_code("wrong-argument-count")
            .with_message("list? requires exactly one argument"));
    }
    let result = matches!(&args[0], SExpr::List(_));
    Ok(SExpr::Atom(if result { "#t" } else { "#f" }.to_string()))
}

fn atom_p(args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 1 {
        return Err(SError::new("vm")
            .with_code("wrong-argument-count")
            .with_message("atom? requires exactly one argument"));
    }
    let result = matches!(&args[0], SExpr::Atom(_));
    Ok(SExpr::Atom(if result { "#t" } else { "#f" }.to_string()))
}

fn empty_p(args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 1 {
        return Err(SError::new("vm")
            .with_code("wrong-argument-count")
            .with_message("empty? requires exactly one argument"));
    }
    let result = matches!(&args[0], SExpr::List(items) if items.is_empty());
    Ok(SExpr::Atom(if result { "#t" } else { "#f" }.to_string()))
}

fn eq_p(args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 2 {
        return Err(SError::new("vm")
            .with_code("wrong-argument-count")
            .with_message("eq? requires exactly two arguments"));
    }
    let result = args[0] == args[1];
    Ok(SExpr::Atom(if result { "#t" } else { "#f" }.to_string()))
}

fn first(args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 1 {
        return Err(SError::new("vm")
            .with_code("wrong-argument-count")
            .with_message("first requires exactly one argument"));
    }
    match &args[0] {
        SExpr::List(items) if !items.is_empty() => Ok(items[0].clone()),
        SExpr::List(_) => Ok(SExpr::Atom("null".to_string())),
        _ => Err(SError::new("vm")
            .with_code("type-error")
            .with_message("first requires a list argument")),
    }
}

fn rest(args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 1 {
        return Err(SError::new("vm")
            .with_code("wrong-argument-count")
            .with_message("rest requires exactly one argument"));
    }
    match &args[0] {
        SExpr::List(items) if !items.is_empty() => Ok(SExpr::List(items[1..].to_vec())),
        SExpr::List(_) => Ok(SExpr::List(vec![])),
        _ => Err(SError::new("vm")
            .with_code("type-error")
            .with_message("rest requires a list argument")),
    }
}

fn cons(args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 2 {
        return Err(SError::new("vm")
            .with_code("wrong-argument-count")
            .with_message("cons requires exactly two arguments"));
    }
    match &args[1] {
        SExpr::List(items) => {
            let mut new_items = vec![args[0].clone()];
            new_items.extend(items.iter().cloned());
            Ok(SExpr::List(new_items))
        }
        _ => Err(SError::new("vm")
            .with_code("type-error")
            .with_message("cons requires a list as second argument")),
    }
}

fn append(args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 2 {
        return Err(SError::new("vm")
            .with_code("wrong-argument-count")
            .with_message("append requires exactly two arguments"));
    }
    match (&args[0], &args[1]) {
        (SExpr::List(a), SExpr::List(b)) => {
            let mut result = a.clone();
            result.extend(b.iter().cloned());
            Ok(SExpr::List(result))
        }
        _ => Err(SError::new("vm")
            .with_code("type-error")
            .with_message("append requires two list arguments")),
    }
}

fn length(args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 1 {
        return Err(SError::new("vm")
            .with_code("wrong-argument-count")
            .with_message("length requires exactly one argument"));
    }
    match &args[0] {
        SExpr::List(items) => Ok(SExpr::Atom(items.len().to_string())),
        _ => Err(SError::new("vm")
            .with_code("type-error")
            .with_message("length requires a list argument")),
    }
}

fn nth(args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 2 {
        return Err(SError::new("vm")
            .with_code("wrong-argument-count")
            .with_message("nth requires exactly two arguments"));
    }
    let index = match &args[0] {
        SExpr::Atom(s) => s.parse::<usize>().map_err(|_| {
            SError::new("vm")
                .with_code("type-error")
                .with_message("nth requires an integer index")
        })?,
        _ => {
            return Err(SError::new("vm")
                .with_code("type-error")
                .with_message("nth requires an integer index"));
        }
    };
    match &args[1] {
        SExpr::List(items) => {
            if index < items.len() {
                Ok(items[index].clone())
            } else {
                Ok(SExpr::Atom("null".to_string()))
            }
        }
        _ => Err(SError::new("vm")
            .with_code("type-error")
            .with_message("nth requires a list as second argument")),
    }
}

fn list(args: &[SExpr]) -> SResult<SExpr> {
    Ok(SExpr::List(args.to_vec()))
}

fn help(args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 1 {
        return Err(SError::new("vm")
            .with_code("wrong-argument-count")
            .with_message("help requires exactly one argument"));
    }
    let name = match &args[0] {
        SExpr::Atom(s) => s.clone(),
        _ => {
            return Err(SError::new("vm")
                .with_code("type-error")
                .with_message("help requires a function name (atom)"));
        }
    };
    match get_help(&name) {
        Some(help_text) => Ok(SExpr::Atom(string_atom(&help_text).to_string())),
        None => Ok(SExpr::Atom(format!("\"No help available for '{}'\"", name))),
    }
}

// JSON builtins wrappers

fn obj(args: &[SExpr]) -> SResult<SExpr> {
    let mut items = vec![SExpr::Atom("obj".to_string())];
    items.extend(args.iter().cloned());
    Ok(SExpr::List(items))
}

fn arr(args: &[SExpr]) -> SResult<SExpr> {
    let mut items = vec![SExpr::Atom("arr".to_string())];
    items.extend(args.iter().cloned());
    Ok(SExpr::List(items))
}

fn builtin_get(args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 2 {
        return Err(SError::new("vm")
            .with_code("wrong-argument-count")
            .with_message("get requires exactly two arguments"));
    }
    let key = match &args[0] {
        SExpr::Atom(s) => {
            if s.starts_with('"') && s.ends_with('"') && s.len() >= 2 {
                s[1..s.len() - 1].to_string()
            } else {
                s.clone()
            }
        }
        _ => {
            return Err(SError::new("vm")
                .with_code("type-error")
                .with_message("get requires a string key"));
        }
    };
    Ok(get(&args[1], &key))
}

fn builtin_keys(args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 1 {
        return Err(SError::new("vm")
            .with_code("wrong-argument-count")
            .with_message("keys requires exactly one argument"));
    }
    Ok(keys(&args[0]))
}

fn builtin_values(args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 1 {
        return Err(SError::new("vm")
            .with_code("wrong-argument-count")
            .with_message("values requires exactly one argument"));
    }
    Ok(values(&args[0]))
}

fn builtin_assoc(args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 3 {
        return Err(SError::new("vm")
            .with_code("wrong-argument-count")
            .with_message("assoc requires exactly three arguments"));
    }
    Ok(assoc(&args[0], &args[1], &args[2]))
}

fn builtin_dissoc(args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 2 {
        return Err(SError::new("vm")
            .with_code("wrong-argument-count")
            .with_message("dissoc requires exactly two arguments"));
    }
    Ok(dissoc(&args[0], &args[1]))
}

fn builtin_merge(args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 2 {
        return Err(SError::new("vm")
            .with_code("wrong-argument-count")
            .with_message("merge requires exactly two arguments"));
    }
    Ok(merge(&args[0], &args[1]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::expr::Parser;

    fn setup_vm() -> Vm {
        let mut vm = Vm::new();
        vm.register_builtins();
        vm.register_json_builtins();
        vm
    }

    fn add(args: &[SExpr]) -> SResult<SExpr> {
        let mut sum = 0i64;
        for arg in args {
            if let SExpr::Atom(s) = arg {
                sum += s.parse::<i64>().unwrap_or(0);
            }
        }
        Ok(SExpr::Atom(sum.to_string()))
    }

    fn double(args: &[SExpr]) -> SResult<SExpr> {
        if let Some(SExpr::Atom(s)) = args.first() {
            let n: i64 = s.parse().unwrap_or(0);
            Ok(SExpr::Atom((n * 2).to_string()))
        } else {
            Ok(SExpr::Atom("0".to_string()))
        }
    }

    fn is_even(args: &[SExpr]) -> SResult<SExpr> {
        if let Some(SExpr::Atom(s)) = args.first() {
            let n: i64 = s.parse().unwrap_or(0);
            Ok(SExpr::Atom(
                if n % 2 == 0 { "#t" } else { "#f" }.to_string(),
            ))
        } else {
            Ok(SExpr::Atom("#f".to_string()))
        }
    }

    // ========================================================================
    // Environment tests
    // ========================================================================

    #[test]
    fn environment_new_is_empty() {
        let env = Environment::new();
        assert!(env.bindings.is_empty());
        assert!(env.parent.is_none());
        println!("DEBUG: new environment is empty");
    }

    #[test]
    fn environment_bind_and_lookup() {
        let mut env = Environment::new();
        env.bind("x", SExpr::Atom("42".to_string()));
        let result = env.lookup("x");
        assert_eq!(result, Some(SExpr::Atom("42".to_string())));
        println!("DEBUG: bind and lookup work correctly");
    }

    #[test]
    fn environment_lookup_not_found() {
        let env = Environment::new();
        let result = env.lookup("nonexistent");
        assert_eq!(result, None);
        println!("DEBUG: lookup returns None for nonexistent binding");
    }

    #[test]
    fn environment_child_inherits_parent_bindings() {
        let mut parent = Environment::new();
        parent.bind("x", SExpr::Atom("parent_x".to_string()));
        let parent_arc = Arc::new(parent);
        let child = parent_arc.child();
        let result = child.lookup("x");
        assert_eq!(result, Some(SExpr::Atom("parent_x".to_string())));
        println!("DEBUG: child environment inherits parent bindings");
    }

    #[test]
    fn environment_child_shadows_parent() {
        let mut parent = Environment::new();
        parent.bind("x", SExpr::Atom("parent_x".to_string()));
        let parent_arc = Arc::new(parent);
        let mut child = parent_arc.child();
        child.bind("x", SExpr::Atom("child_x".to_string()));
        let result = child.lookup("x");
        assert_eq!(result, Some(SExpr::Atom("child_x".to_string())));
        println!("DEBUG: child environment can shadow parent bindings");
    }

    #[test]
    fn environment_default_is_new() {
        let env: Environment = Default::default();
        assert!(env.bindings.is_empty());
        assert!(env.parent.is_none());
        println!("DEBUG: Environment::default() matches Environment::new()");
    }

    // ========================================================================
    // VM construction and registration tests
    // ========================================================================

    #[test]
    fn vm_new_is_empty() {
        let vm = Vm::new();
        assert!(vm.frames.is_empty());
        assert!(vm.functions.is_empty());
        assert!(vm.function_names.is_empty());
        assert_eq!(vm.next_func_id, 0);
        assert!(vm.current_result.is_none());
        println!("DEBUG: new VM is in empty state");
    }

    #[test]
    fn vm_default_matches_new() {
        let vm1 = Vm::new();
        let vm2: Vm = Default::default();
        assert_eq!(vm1.frames.len(), vm2.frames.len());
        assert_eq!(vm1.functions.len(), vm2.functions.len());
        println!("DEBUG: Vm::default() matches Vm::new()");
    }

    #[test]
    fn vm_def_fn_registers_function() {
        let mut vm = Vm::new();
        vm.def_fn("test-func", add);
        assert!(vm.function_names.contains_key("test-func"));
        assert_eq!(vm.functions.len(), 1);
        assert_eq!(vm.next_func_id, 1);
        println!("DEBUG: def_fn registers function correctly");
    }

    #[test]
    fn vm_register_builtins_adds_all_builtins() {
        let mut vm = Vm::new();
        vm.register_builtins();
        assert!(vm.function_names.contains_key("null?"));
        assert!(vm.function_names.contains_key("list?"));
        assert!(vm.function_names.contains_key("atom?"));
        assert!(vm.function_names.contains_key("empty?"));
        assert!(vm.function_names.contains_key("eq?"));
        assert!(vm.function_names.contains_key("first"));
        assert!(vm.function_names.contains_key("rest"));
        assert!(vm.function_names.contains_key("cons"));
        assert!(vm.function_names.contains_key("append"));
        assert!(vm.function_names.contains_key("length"));
        assert!(vm.function_names.contains_key("nth"));
        assert!(vm.function_names.contains_key("list"));
        assert!(vm.function_names.contains_key("help"));
        println!("DEBUG: register_builtins adds all expected functions");
    }

    #[test]
    fn vm_register_json_builtins_adds_all() {
        let mut vm = Vm::new();
        vm.register_json_builtins();
        assert!(vm.function_names.contains_key("obj"));
        assert!(vm.function_names.contains_key("arr"));
        assert!(vm.function_names.contains_key("get"));
        assert!(vm.function_names.contains_key("keys"));
        assert!(vm.function_names.contains_key("values"));
        assert!(vm.function_names.contains_key("assoc"));
        assert!(vm.function_names.contains_key("dissoc"));
        assert!(vm.function_names.contains_key("merge"));
        println!("DEBUG: register_json_builtins adds all expected functions");
    }

    // ========================================================================
    // VM state management tests
    // ========================================================================

    #[test]
    fn vm_load_clears_state() {
        let mut vm = setup_vm();
        vm.current_result = Some(SExpr::Atom("old".to_string()));
        vm.load(SExpr::Atom("new".to_string()));
        assert_eq!(vm.frames.len(), 1);
        assert!(vm.current_result.is_none());
        println!("DEBUG: load clears previous state");
    }

    #[test]
    fn vm_reset_clears_everything() {
        let mut vm = setup_vm();
        vm.load(SExpr::Atom("test".to_string()));
        vm.current_result = Some(SExpr::Atom("result".to_string()));
        vm.reset();
        assert!(vm.frames.is_empty());
        assert!(vm.current_result.is_none());
        println!("DEBUG: reset clears frames and result");
    }

    #[test]
    fn vm_push_result_sets_current_result() {
        let mut vm = setup_vm();
        let value = SExpr::Atom("pushed".to_string());
        vm.push_result(value.clone());
        assert_eq!(vm.current_result, Some(value));
        println!("DEBUG: push_result sets current_result");
    }

    // ========================================================================
    // is_truthy function tests
    // ========================================================================

    #[test]
    fn truthy_atom_true() {
        assert!(is_truthy(&SExpr::Atom("#t".to_string())));
        println!("DEBUG: #t is truthy");
    }

    #[test]
    fn truthy_atom_false() {
        assert!(!is_truthy(&SExpr::Atom("#f".to_string())));
        println!("DEBUG: #f is falsy");
    }

    #[test]
    fn truthy_atom_null() {
        assert!(!is_truthy(&SExpr::Atom("null".to_string())));
        println!("DEBUG: null is falsy");
    }

    #[test]
    fn truthy_any_other_atom() {
        assert!(is_truthy(&SExpr::Atom("anything".to_string())));
        assert!(is_truthy(&SExpr::Atom("0".to_string())));
        assert!(is_truthy(&SExpr::Atom("".to_string())));
        println!("DEBUG: other atoms are truthy (including empty string and 0)");
    }

    #[test]
    fn truthy_non_empty_list() {
        assert!(is_truthy(&SExpr::List(vec![SExpr::Atom("a".to_string())])));
        println!("DEBUG: non-empty list is truthy");
    }

    #[test]
    fn truthy_empty_list() {
        assert!(!is_truthy(&SExpr::List(vec![])));
        println!("DEBUG: empty list is falsy");
    }

    // ========================================================================
    // Error condition tests
    // ========================================================================

    #[test]
    fn vm_error_quote_wrong_arg_count() {
        let mut vm = setup_vm();
        let mut parser = Parser::new("(quote)");
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("wrong-argument-count"));
        println!("DEBUG: quote with no args returns error: {}", err);
    }

    #[test]
    fn vm_error_quote_too_many_args() {
        let mut vm = setup_vm();
        let mut parser = Parser::new("(quote a b)");
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr);
        assert!(result.is_err());
        println!(
            "DEBUG: quote with too many args returns error: {}",
            result.unwrap_err()
        );
    }

    #[test]
    fn vm_error_if_wrong_arg_count() {
        let mut vm = setup_vm();
        let mut parser = Parser::new("(if #t then-only)");
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("wrong-argument-count"));
        println!("DEBUG: if with wrong args returns error: {}", err);
    }

    #[test]
    fn vm_error_let_missing_body() {
        let mut vm = setup_vm();
        let mut parser = Parser::new("(let ((x 1)))");
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr);
        assert!(result.is_err());
        println!(
            "DEBUG: let with missing body returns error: {}",
            result.unwrap_err()
        );
    }

    #[test]
    fn vm_error_let_non_list_bindings() {
        let mut vm = setup_vm();
        let mut parser = Parser::new("(let not-a-list x)");
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("invalid-let-bindings"));
        println!("DEBUG: let with non-list bindings returns error: {}", err);
    }

    #[test]
    fn vm_error_let_malformed_binding() {
        let mut vm = setup_vm();
        let mut parser = Parser::new("(let ((x)) x)");
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("invalid-binding"));
        println!("DEBUG: let with malformed binding returns error: {}", err);
    }

    #[test]
    fn vm_error_let_non_atom_var_name() {
        let mut vm = setup_vm();
        let mut parser = Parser::new("(let (((nested) 1)) x)");
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("invalid-binding-name"));
        println!(
            "DEBUG: let with non-atom variable name returns error: {}",
            err
        );
    }

    #[test]
    fn vm_error_begin_no_expressions() {
        let mut vm = setup_vm();
        let mut parser = Parser::new("(begin)");
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("wrong-argument-count"));
        println!("DEBUG: begin with no expressions returns error: {}", err);
    }

    #[test]
    fn vm_error_thread_first_no_value() {
        let mut vm = setup_vm();
        let mut parser = Parser::new("(->)");
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr);
        assert!(result.is_err());
        println!(
            "DEBUG: -> with no value returns error: {}",
            result.unwrap_err()
        );
    }

    #[test]
    fn vm_error_thread_last_no_value() {
        let mut vm = setup_vm();
        let mut parser = Parser::new("(->>)");
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr);
        assert!(result.is_err());
        println!(
            "DEBUG: ->> with no value returns error: {}",
            result.unwrap_err()
        );
    }

    #[test]
    fn vm_error_map_wrong_arg_count() {
        let mut vm = setup_vm();
        vm.def_fn("double", double);
        let mut parser = Parser::new("(map double)");
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("wrong-argument-count"));
        println!("DEBUG: map with wrong args returns error: {}", err);
    }

    #[test]
    fn vm_error_map_non_atom_func() {
        let mut vm = setup_vm();
        let mut parser = Parser::new("(map (not-an-atom) (quote (1 2)))");
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("invalid-function"));
        println!("DEBUG: map with non-atom function returns error: {}", err);
    }

    #[test]
    fn vm_error_map_non_list_arg() {
        let mut vm = setup_vm();
        vm.def_fn("double", double);
        let mut parser = Parser::new("(map double not-a-list)");
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("type-error"));
        println!("DEBUG: map with non-list arg returns error: {}", err);
    }

    #[test]
    fn vm_error_filter_wrong_arg_count() {
        let mut vm = setup_vm();
        vm.def_fn("is-even", is_even);
        let mut parser = Parser::new("(filter is-even)");
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr);
        assert!(result.is_err());
        println!(
            "DEBUG: filter with wrong args returns error: {}",
            result.unwrap_err()
        );
    }

    #[test]
    fn vm_error_filter_non_atom_func() {
        let mut vm = setup_vm();
        let mut parser = Parser::new("(filter (nested) (quote (1)))");
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr);
        assert!(result.is_err());
        println!(
            "DEBUG: filter with non-atom function returns error: {}",
            result.unwrap_err()
        );
    }

    #[test]
    fn vm_error_filter_non_list_arg() {
        let mut vm = setup_vm();
        vm.def_fn("is-even", is_even);
        let mut parser = Parser::new("(filter is-even not-a-list)");
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr);
        assert!(result.is_err());
        println!(
            "DEBUG: filter with non-list arg returns error: {}",
            result.unwrap_err()
        );
    }

    #[test]
    fn vm_error_reduce_wrong_arg_count() {
        let mut vm = setup_vm();
        vm.def_fn("+", add);
        let mut parser = Parser::new("(reduce + 0)");
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr);
        assert!(result.is_err());
        println!(
            "DEBUG: reduce with wrong args returns error: {}",
            result.unwrap_err()
        );
    }

    #[test]
    fn vm_error_reduce_non_atom_func() {
        let mut vm = setup_vm();
        let mut parser = Parser::new("(reduce (nested) 0 (quote (1)))");
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr);
        assert!(result.is_err());
        println!(
            "DEBUG: reduce with non-atom function returns error: {}",
            result.unwrap_err()
        );
    }

    #[test]
    fn vm_error_reduce_non_list_arg() {
        let mut vm = setup_vm();
        vm.def_fn("+", add);
        let mut parser = Parser::new("(reduce + 0 not-a-list)");
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr);
        assert!(result.is_err());
        println!(
            "DEBUG: reduce with non-list arg returns error: {}",
            result.unwrap_err()
        );
    }

    #[test]
    fn vm_error_non_atom_function_name() {
        let mut vm = setup_vm();
        let mut parser = Parser::new("((not a function name) arg)");
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("invalid-function-name"));
        println!("DEBUG: non-atom function name returns error: {}", err);
    }

    // ========================================================================
    // Edge case tests
    // ========================================================================

    #[test]
    fn vm_map_empty_list() {
        let mut vm = setup_vm();
        vm.def_fn("double", double);
        let mut parser = Parser::new("(map double (quote ()))");
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr).unwrap();
        assert_eq!(result, SExpr::List(vec![]));
        println!("DEBUG: map on empty list returns empty list");
    }

    #[test]
    fn vm_filter_empty_list() {
        let mut vm = setup_vm();
        vm.def_fn("is-even", is_even);
        let mut parser = Parser::new("(filter is-even (quote ()))");
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr).unwrap();
        assert_eq!(result, SExpr::List(vec![]));
        println!("DEBUG: filter on empty list returns empty list");
    }

    #[test]
    fn vm_reduce_empty_list() {
        let mut vm = setup_vm();
        vm.def_fn("+", add);
        let mut parser = Parser::new("(reduce + 100 (quote ()))");
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr).unwrap();
        assert_eq!(result, SExpr::Atom("100".to_string()));
        println!("DEBUG: reduce on empty list returns initial value");
    }

    #[test]
    fn vm_let_empty_bindings() {
        let mut vm = setup_vm();
        vm.def_fn("+", add);
        let mut parser = Parser::new("(let () (+ 1 2))");
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr).unwrap();
        assert_eq!(result, SExpr::Atom("3".to_string()));
        println!("DEBUG: let with empty bindings evaluates body");
    }

    #[test]
    fn vm_thread_first_no_forms() {
        let mut vm = setup_vm();
        let mut parser = Parser::new("(-> 42)");
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr).unwrap();
        assert_eq!(result, SExpr::Atom("42".to_string()));
        println!("DEBUG: -> with no forms returns initial value");
    }

    #[test]
    fn vm_thread_last_no_forms() {
        let mut vm = setup_vm();
        let mut parser = Parser::new("(->> 42)");
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr).unwrap();
        assert_eq!(result, SExpr::Atom("42".to_string()));
        println!("DEBUG: ->> with no forms returns initial value");
    }

    #[test]
    fn vm_thread_first_with_bare_function_name() {
        let mut vm = setup_vm();
        vm.def_fn("double", double);
        let mut parser = Parser::new("(-> 5 double)");
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr).unwrap();
        assert_eq!(result, SExpr::Atom("10".to_string()));
        println!("DEBUG: -> with bare function name works");
    }

    #[test]
    fn vm_thread_last_with_bare_function_name() {
        let mut vm = setup_vm();
        vm.def_fn("double", double);
        let mut parser = Parser::new("(->> 5 double)");
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr).unwrap();
        assert_eq!(result, SExpr::Atom("10".to_string()));
        println!("DEBUG: ->> with bare function name works");
    }

    #[test]
    fn vm_if_with_null_condition() {
        let mut vm = setup_vm();
        vm.def_fn("+", add);
        let mut parser = Parser::new("(if null (+ 1 2) (+ 3 4))");
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr).unwrap();
        assert_eq!(result, SExpr::Atom("7".to_string()));
        println!("DEBUG: if with null condition takes else branch");
    }

    #[test]
    fn vm_if_with_empty_list_condition() {
        let mut vm = setup_vm();
        vm.def_fn("+", add);
        let mut parser = Parser::new("(if (quote ()) (+ 1 2) (+ 3 4))");
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr).unwrap();
        assert_eq!(result, SExpr::Atom("7".to_string()));
        println!("DEBUG: if with empty list condition takes else branch");
    }

    #[test]
    fn vm_if_with_non_empty_list_condition() {
        let mut vm = setup_vm();
        vm.def_fn("+", add);
        let mut parser = Parser::new("(if (quote (a)) (+ 1 2) (+ 3 4))");
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr).unwrap();
        assert_eq!(result, SExpr::Atom("3".to_string()));
        println!("DEBUG: if with non-empty list condition takes then branch");
    }

    #[test]
    fn vm_begin_single_expression() {
        let mut vm = setup_vm();
        vm.def_fn("+", add);
        let mut parser = Parser::new("(begin (+ 5 5))");
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr).unwrap();
        assert_eq!(result, SExpr::Atom("10".to_string()));
        println!("DEBUG: begin with single expression returns that expression's result");
    }

    #[test]
    fn vm_let_sequential_bindings() {
        let mut vm = setup_vm();
        vm.def_fn("+", add);
        let mut parser = Parser::new("(let ((x 5) (y (+ x 3))) y)");
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr).unwrap();
        assert_eq!(result, SExpr::Atom("8".to_string()));
        println!("DEBUG: let bindings can reference earlier bindings");
    }

    #[test]
    fn vm_let_multiple_body_expressions() {
        let mut vm = setup_vm();
        vm.def_fn("+", add);
        let mut parser = Parser::new("(let ((x 1)) (+ x 1) (+ x 2) (+ x 3))");
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr).unwrap();
        assert_eq!(result, SExpr::Atom("4".to_string()));
        println!("DEBUG: let with multiple body expressions returns last result");
    }

    #[test]
    fn vm_deeply_nested_if() {
        let mut vm = setup_vm();
        vm.def_fn("+", add);
        let mut parser = Parser::new("(if #t (if #f 1 (if #t 2 3)) 4)");
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr).unwrap();
        assert_eq!(result, SExpr::Atom("2".to_string()));
        println!("DEBUG: deeply nested if works correctly");
    }

    #[test]
    fn vm_filter_all_pass() {
        let mut vm = setup_vm();
        vm.def_fn("is-even", is_even);
        let mut parser = Parser::new("(filter is-even (quote (2 4 6)))");
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr).unwrap();
        assert_eq!(
            result,
            SExpr::List(vec![
                SExpr::Atom("2".to_string()),
                SExpr::Atom("4".to_string()),
                SExpr::Atom("6".to_string()),
            ])
        );
        println!("DEBUG: filter where all pass returns all");
    }

    #[test]
    fn vm_filter_none_pass() {
        let mut vm = setup_vm();
        vm.def_fn("is-even", is_even);
        let mut parser = Parser::new("(filter is-even (quote (1 3 5)))");
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr).unwrap();
        assert_eq!(result, SExpr::List(vec![]));
        println!("DEBUG: filter where none pass returns empty list");
    }

    #[test]
    fn vm_reduce_single_element() {
        let mut vm = setup_vm();
        vm.def_fn("+", add);
        let mut parser = Parser::new("(reduce + 0 (quote (42)))");
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr).unwrap();
        assert_eq!(result, SExpr::Atom("42".to_string()));
        println!("DEBUG: reduce on single element list works");
    }

    #[test]
    fn vm_map_single_element() {
        let mut vm = setup_vm();
        vm.def_fn("double", double);
        let mut parser = Parser::new("(map double (quote (21)))");
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr).unwrap();
        assert_eq!(result, SExpr::List(vec![SExpr::Atom("42".to_string())]));
        println!("DEBUG: map on single element list works");
    }

    // ========================================================================
    // Complex integration tests
    // ========================================================================

    #[test]
    fn vm_nested_let_with_shadowing() {
        let mut vm = setup_vm();
        vm.def_fn("+", add);
        let mut parser = Parser::new("(let ((x 1)) (let ((x 2)) (+ x 10)))");
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr).unwrap();
        assert_eq!(result, SExpr::Atom("12".to_string()));
        println!("DEBUG: nested let shadows outer binding");
    }

    #[test]
    fn vm_thread_first_multiple_forms() {
        let mut vm = setup_vm();
        vm.def_fn("+", add);
        vm.def_fn("double", double);
        let mut parser = Parser::new("(-> 1 (+ 2) double (+ 3))");
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr).unwrap();
        assert_eq!(result, SExpr::Atom("9".to_string()));
        println!("DEBUG: -> with multiple forms chains correctly");
    }

    #[test]
    fn vm_thread_last_multiple_forms() {
        let mut vm = setup_vm();
        vm.def_fn("+", add);
        vm.def_fn("double", double);
        let mut parser = Parser::new("(->> 1 (+ 2) double (+ 3))");
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr).unwrap();
        assert_eq!(result, SExpr::Atom("9".to_string()));
        println!("DEBUG: ->> with multiple forms chains correctly");
    }

    #[test]
    fn vm_map_filter_reduce_pipeline() {
        let mut vm = setup_vm();
        vm.def_fn("+", add);
        vm.def_fn("double", double);
        vm.def_fn("is-even", is_even);

        // Filter evens from (1 2 3 4), double them, reduce with +
        // (2 4) -> (4 8) -> 12
        let mut parser =
            Parser::new("(reduce + 0 (map double (filter is-even (quote (1 2 3 4)))))");
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr).unwrap();
        assert_eq!(result, SExpr::Atom("12".to_string()));
        println!("DEBUG: map/filter/reduce pipeline works");
    }

    #[test]
    fn vm_builtin_first_in_vm() {
        let mut vm = setup_vm();
        let mut parser = Parser::new("(first (quote (a b c)))");
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr).unwrap();
        assert_eq!(result, SExpr::Atom("a".to_string()));
        println!("DEBUG: builtin first works in VM");
    }

    #[test]
    fn vm_builtin_rest_in_vm() {
        let mut vm = setup_vm();
        let mut parser = Parser::new("(rest (quote (a b c)))");
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr).unwrap();
        assert_eq!(
            result,
            SExpr::List(vec![
                SExpr::Atom("b".to_string()),
                SExpr::Atom("c".to_string()),
            ])
        );
        println!("DEBUG: builtin rest works in VM");
    }

    #[test]
    fn vm_builtin_cons_in_vm() {
        let mut vm = setup_vm();
        let mut parser = Parser::new("(cons (quote a) (quote (b c)))");
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr).unwrap();
        assert_eq!(
            result,
            SExpr::List(vec![
                SExpr::Atom("a".to_string()),
                SExpr::Atom("b".to_string()),
                SExpr::Atom("c".to_string()),
            ])
        );
        println!("DEBUG: builtin cons works in VM");
    }

    #[test]
    fn vm_builtin_length_in_vm() {
        let mut vm = setup_vm();
        let mut parser = Parser::new("(length (quote (a b c d e)))");
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr).unwrap();
        assert_eq!(result, SExpr::Atom("5".to_string()));
        println!("DEBUG: builtin length works in VM");
    }

    #[test]
    fn vm_builtin_nth_in_vm() {
        let mut vm = setup_vm();
        let mut parser = Parser::new("(nth 2 (quote (a b c d)))");
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr).unwrap();
        assert_eq!(result, SExpr::Atom("c".to_string()));
        println!("DEBUG: builtin nth works in VM");
    }

    #[test]
    fn vm_builtin_list_p_true() {
        let mut vm = setup_vm();
        let mut parser = Parser::new("(list? (quote (a b)))");
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr).unwrap();
        assert_eq!(result, SExpr::Atom("#t".to_string()));
        println!("DEBUG: list? returns #t for list");
    }

    #[test]
    fn vm_builtin_list_p_false() {
        let mut vm = setup_vm();
        let mut parser = Parser::new("(list? (quote atom))");
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr).unwrap();
        assert_eq!(result, SExpr::Atom("#f".to_string()));
        println!("DEBUG: list? returns #f for atom");
    }

    #[test]
    fn vm_builtin_atom_p_true() {
        let mut vm = setup_vm();
        let mut parser = Parser::new("(atom? (quote hello))");
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr).unwrap();
        assert_eq!(result, SExpr::Atom("#t".to_string()));
        println!("DEBUG: atom? returns #t for atom");
    }

    #[test]
    fn vm_builtin_atom_p_false() {
        let mut vm = setup_vm();
        let mut parser = Parser::new("(atom? (quote (a b)))");
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr).unwrap();
        assert_eq!(result, SExpr::Atom("#f".to_string()));
        println!("DEBUG: atom? returns #f for list");
    }

    #[test]
    fn vm_builtin_empty_p_true() {
        let mut vm = setup_vm();
        let mut parser = Parser::new("(empty? (quote ()))");
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr).unwrap();
        assert_eq!(result, SExpr::Atom("#t".to_string()));
        println!("DEBUG: empty? returns #t for empty list");
    }

    #[test]
    fn vm_builtin_empty_p_false() {
        let mut vm = setup_vm();
        let mut parser = Parser::new("(empty? (quote (a)))");
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr).unwrap();
        assert_eq!(result, SExpr::Atom("#f".to_string()));
        println!("DEBUG: empty? returns #f for non-empty list");
    }

    #[test]
    fn vm_builtin_eq_p_true() {
        let mut vm = setup_vm();
        let mut parser = Parser::new("(eq? (quote a) (quote a))");
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr).unwrap();
        assert_eq!(result, SExpr::Atom("#t".to_string()));
        println!("DEBUG: eq? returns #t for equal atoms");
    }

    #[test]
    fn vm_builtin_eq_p_false() {
        let mut vm = setup_vm();
        let mut parser = Parser::new("(eq? (quote a) (quote b))");
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr).unwrap();
        assert_eq!(result, SExpr::Atom("#f".to_string()));
        println!("DEBUG: eq? returns #f for different atoms");
    }

    #[test]
    fn vm_builtin_null_p_true() {
        let mut vm = setup_vm();
        let mut parser = Parser::new("(null? null)");
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr).unwrap();
        assert_eq!(result, SExpr::Atom("#t".to_string()));
        println!("DEBUG: null? returns #t for null atom");
    }

    #[test]
    fn vm_builtin_null_p_false() {
        let mut vm = setup_vm();
        let mut parser = Parser::new("(null? foo)");
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr).unwrap();
        assert_eq!(result, SExpr::Atom("#f".to_string()));
        println!("DEBUG: null? returns #f for non-null atom");
    }

    #[test]
    fn vm_builtin_append_in_vm() {
        let mut vm = setup_vm();
        let mut parser = Parser::new("(append (quote (a b)) (quote (c d)))");
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr).unwrap();
        assert_eq!(
            result,
            SExpr::List(vec![
                SExpr::Atom("a".to_string()),
                SExpr::Atom("b".to_string()),
                SExpr::Atom("c".to_string()),
                SExpr::Atom("d".to_string()),
            ])
        );
        println!("DEBUG: builtin append works in VM");
    }

    #[test]
    fn vm_eval_suspended_converts_to_error() {
        let mut vm = setup_vm();
        let mut parser = Parser::new("(nonexistent-func 1 2)");
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("function-not-found"));
        println!(
            "DEBUG: eval converts Suspended(FunctionNotFound) to error: {}",
            err
        );
    }

    #[test]
    fn vm_step_empty_frames_with_result() {
        let mut vm = Vm::new();
        vm.current_result = Some(SExpr::Atom("result".to_string()));
        match vm.step() {
            VmState::Finished(result) => {
                assert_eq!(result, SExpr::Atom("result".to_string()));
                println!("DEBUG: step with empty frames and result returns Finished");
            }
            other => panic!("Expected Finished, got {:?}", other),
        }
    }

    #[test]
    fn vm_step_empty_frames_no_result() {
        let mut vm = Vm::new();
        match vm.step() {
            VmState::Finished(result) => {
                assert_eq!(result, SExpr::List(vec![]));
                println!("DEBUG: step with empty frames and no result returns empty list");
            }
            other => panic!("Expected Finished, got {:?}", other),
        }
    }

    // ========================================================================
    // Lambda tests (if supported)
    // ========================================================================

    #[test]
    fn lambda_function_obj_stores_params_body_env() {
        let params = vec!["x".to_string(), "y".to_string()];
        let body = SExpr::List(vec![
            SExpr::Atom("+".to_string()),
            SExpr::Atom("x".to_string()),
            SExpr::Atom("y".to_string()),
        ]);
        let env = Arc::new(Environment::new());
        let lambda = FunctionObj::Lambda {
            params: params.clone(),
            body: body.clone(),
            env: Arc::clone(&env),
        };
        match lambda {
            FunctionObj::Lambda {
                params: p,
                body: b,
                env: e,
            } => {
                assert_eq!(p, params);
                assert_eq!(b, body);
                assert!(Arc::ptr_eq(&e, &env));
                println!("DEBUG: Lambda stores params, body, and env correctly");
            }
            _ => panic!("Expected Lambda variant"),
        }
    }

    #[test]
    fn builtin_function_obj_stores_name_func() {
        let builtin = FunctionObj::Builtin {
            name: "test-fn".to_string(),
            func: add,
        };
        match builtin {
            FunctionObj::Builtin { name, func: _ } => {
                assert_eq!(name, "test-fn");
                println!("DEBUG: Builtin stores name correctly");
            }
            _ => panic!("Expected Builtin variant"),
        }
    }

    // ========================================================================
    // Restart enum tests
    // ========================================================================

    #[test]
    fn restart_abort_debug() {
        let restart = Restart::Abort;
        assert!(format!("{:?}", restart).contains("Abort"));
        println!("DEBUG: Restart::Abort debug format works");
    }

    #[test]
    fn restart_use_value_debug() {
        let restart = Restart::UseValue(SExpr::Atom("42".to_string()));
        let debug_str = format!("{:?}", restart);
        assert!(debug_str.contains("UseValue"));
        assert!(debug_str.contains("42"));
        println!("DEBUG: Restart::UseValue debug format works");
    }

    #[test]
    fn restart_retry_debug() {
        let restart = Restart::Retry;
        assert!(format!("{:?}", restart).contains("Retry"));
        println!("DEBUG: Restart::Retry debug format works");
    }

    // ========================================================================
    // VmState tests
    // ========================================================================

    #[test]
    fn vm_state_running_debug() {
        let state = VmState::Running;
        assert!(format!("{:?}", state).contains("Running"));
        println!("DEBUG: VmState::Running debug format works");
    }

    #[test]
    fn vm_state_finished_debug() {
        let state = VmState::Finished(SExpr::Atom("done".to_string()));
        let debug_str = format!("{:?}", state);
        assert!(debug_str.contains("Finished"));
        assert!(debug_str.contains("done"));
        println!("DEBUG: VmState::Finished debug format works");
    }

    #[test]
    fn vm_state_suspended_debug() {
        let state = VmState::Suspended(Condition::FunctionNotFound {
            name: "test".to_string(),
        });
        let debug_str = format!("{:?}", state);
        assert!(debug_str.contains("Suspended"));
        assert!(debug_str.contains("FunctionNotFound"));
        println!("DEBUG: VmState::Suspended debug format works");
    }
}

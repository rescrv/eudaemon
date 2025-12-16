//! Stackless virtual machine for S-expression evaluation.
//!
//! This module implements a VM with an explicit call stack, enabling features like:
//! - Steppable execution for debugging
//! - Restarts for error recovery without unwinding
//! - Hot-swapping of function definitions
//!
//! The VM stores functions in an arena and uses `FunctionId` references, allowing
//! function redefinition to affect all existing references.

use std::any::Any;
use std::any::TypeId;
use std::collections::HashMap;
use std::sync::Arc;

use crate::docs::get_help;
use crate::error::{SError, SResult};
use crate::expr::SExpr;
use crate::markdown::{markdown_to_sexpr, sexpr_to_markdown};
use crate::object::{assoc, dissoc, get, keys, merge, values};
use crate::util::{extract_string, string_atom};
use eudaemonty::Filesystem;

/// Unique identifier for a function in the arena.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FunctionId(usize);

/// Function signature for built-in functions.
///
/// Built-in functions receive a reference to the VM for access to filesystem
/// and other VM state, plus the evaluated arguments.
pub type BuiltinFn = fn(&Vm, &[SExpr]) -> SResult<SExpr>;

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
    /// A user-defined lambda function.
    ///
    /// Created by `(lambda (params) body)` or `(defun name (params) body)` special forms.
    /// Lambdas capture the lexical environment at definition time for closure semantics.
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
    env: Environment,
}

impl Frame {
    fn new(op: FrameOp) -> Self {
        Frame {
            op,
            env: Environment::new(),
        }
    }

    fn with_env(op: FrameOp, env: Environment) -> Self {
        Frame { op, env }
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
///
/// TODO(claude): Per PLAN.md summary, make the entire VM state serializable with serde
/// to enable "Save/Load Image" functionality. Add `#[derive(serde::Serialize, serde::Deserialize)]`
/// to Vm, Frame, FrameOp, SpecialFormState, FunctionObj, and related types.
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
    /// Global variable bindings (for REPL file bindings).
    globals: HashMap<String, SExpr>,
    /// Optional filesystem for file operations.
    filesystem: Option<Box<dyn Filesystem>>,
    /// User data storage, keyed by TypeId.
    user_data: HashMap<TypeId, Arc<dyn Any + Send + Sync>>,
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
            globals: HashMap::new(),
            filesystem: None,
            user_data: HashMap::new(),
        }
    }

    /// Returns a reference to the filesystem, if one is set.
    pub fn filesystem(&self) -> Option<&dyn Filesystem> {
        self.filesystem.as_deref()
    }

    /// Sets the filesystem for this VM.
    pub fn set_filesystem(&mut self, fs: Box<dyn Filesystem>) {
        self.filesystem = Some(fs);
    }

    /// Sets user data of type T.
    ///
    /// If user data of this type already exists, it is replaced.
    /// User data persists across evaluations and is not cleared by `reset()`.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let inspector = Arc::new(Mutex::new(Inspector::new("disk.img")?));
    /// vm.set_user_data(inspector);
    /// ```
    pub fn set_user_data<T: Any + Send + Sync + 'static>(&mut self, data: T) {
        let type_id = TypeId::of::<T>();
        self.user_data.insert(type_id, Arc::new(data));
    }

    /// Gets a reference to user data of type T.
    ///
    /// Returns `None` if no user data of this type has been set.
    ///
    /// # Example
    ///
    /// ```ignore
    /// fn builtin_foo(vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
    ///     let inspector = vm.get_user_data::<Arc<Mutex<Inspector>>>()
    ///         .ok_or_else(|| SError::new("foo").with_message("No inspector"))?;
    ///     // use inspector...
    /// }
    /// ```
    pub fn get_user_data<T: Any + Send + Sync + 'static>(&self) -> Option<&T> {
        let type_id = TypeId::of::<T>();
        self.user_data
            .get(&type_id)
            .and_then(|arc| arc.downcast_ref::<T>())
    }

    /// Removes and returns user data of type T.
    ///
    /// Returns `None` if no user data of this type has been set.
    pub fn take_user_data<T: Any + Send + Sync + Clone + 'static>(&mut self) -> Option<T> {
        let type_id = TypeId::of::<T>();
        self.user_data
            .remove(&type_id)
            .and_then(|arc| arc.downcast_ref::<T>().cloned())
    }

    /// Returns true if user data of type T exists.
    pub fn has_user_data<T: Any + Send + Sync + 'static>(&self) -> bool {
        let type_id = TypeId::of::<T>();
        self.user_data.contains_key(&type_id)
    }

    /// Binds a global variable.
    pub fn bind_global(&mut self, name: &str, value: SExpr) {
        self.globals.insert(name.to_string(), value);
    }

    /// Looks up a global variable.
    fn lookup_global(&self, name: &str) -> Option<&SExpr> {
        self.globals.get(name)
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
        self.def_fn("str", str_concat);
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

    /// Registers filesystem builtins for file I/O operations.
    ///
    /// These builtins require a filesystem to be set on the VM via [`Vm::set_filesystem`].
    /// If no filesystem is set, the builtins will return an error.
    pub fn register_filesystem_builtins(&mut self) {
        // High-level markdown operations
        self.def_fn("load", builtin_load);
        self.def_fn("save", builtin_save);
        self.def_fn("list-files", builtin_list_files);
        self.def_fn("splat", builtin_splat);

        // Basic file operations
        self.def_fn("fs-read", builtin_fs_read);
        self.def_fn("fs-write", builtin_fs_write);
        self.def_fn("fs-append", builtin_fs_append);
        self.def_fn("fs-exists?", builtin_fs_exists);
        self.def_fn("fs-is-dir?", builtin_fs_is_dir);

        // Directory operations
        self.def_fn("fs-mkdir", builtin_fs_mkdir);
        self.def_fn("fs-mkdir-all", builtin_fs_mkdir_all);
        self.def_fn("fs-readdir", builtin_fs_readdir);
        self.def_fn("fs-rmdir", builtin_fs_rmdir);

        // File metadata and manipulation
        self.def_fn("fs-stat", builtin_fs_stat);
        self.def_fn("fs-lstat", builtin_fs_lstat);
        self.def_fn("fs-unlink", builtin_fs_unlink);
        self.def_fn("fs-rename", builtin_fs_rename);

        // Symlink and link operations
        self.def_fn("fs-symlink", builtin_fs_symlink);
        self.def_fn("fs-readlink", builtin_fs_readlink);
        self.def_fn("fs-link", builtin_fs_link);

        // Debug/introspection operations
        self.def_fn("fs-tree", builtin_fs_tree);
        self.def_fn("fs-free-blocks", builtin_fs_free_blocks);
        self.def_fn("fs-total-blocks", builtin_fs_total_blocks);
        self.def_fn("fs-usage-percent", builtin_fs_usage_percent);
        self.def_fn("fs-clean", builtin_fs_clean);

        // Legacy aliases for compatibility
        self.def_fn("read-file", builtin_fs_read);
        self.def_fn("write-file", builtin_fs_write);
        self.def_fn("file-exists?", builtin_fs_exists);
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

    /// Applies a restart to recover from a suspended condition.
    pub fn apply_restart(&mut self, restart: Restart) {
        match restart {
            Restart::Abort => {
                self.reset();
            }
            Restart::UseValue(value) => {
                // Push the replacement value as the result and continue
                self.current_result = Some(value);
            }
            Restart::Retry => {
                // Re-execute the current frame (already on stack, nothing to do)
            }
        }
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
            FrameOp::Eval(expr) => self.step_eval(expr, frame.env),
            FrameOp::Call {
                func_name,
                args,
                evaluated_count,
            } => self.step_call(func_name, args, evaluated_count, frame.env),
            FrameOp::SpecialForm { name, args, state } => {
                self.step_special_form(name, args, state, frame.env)
            }
        }
    }

    /// Steps an Eval frame.
    fn step_eval(&mut self, expr: SExpr, env: Environment) -> Result<VmState, Condition> {
        match expr {
            SExpr::Atom(ref s) => {
                // Check for local variable binding first (uses Environment::lookup)
                if let Some(value) = env.lookup(s) {
                    self.current_result = Some(value);
                    return Ok(VmState::Running);
                }
                // Check for global variable binding
                if let Some(value) = self.lookup_global(s) {
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
                    | "reduce" | "lambda" | "defun" => {
                        self.frames.push(Frame::with_env(
                            FrameOp::SpecialForm {
                                name: func_name,
                                args,
                                state: SpecialFormState::WaitingForValue,
                            },
                            env,
                        ));
                        Ok(VmState::Running)
                    }
                    _ => {
                        // Regular function call - start evaluating arguments
                        self.frames.push(Frame::with_env(
                            FrameOp::Call {
                                func_name,
                                args,
                                evaluated_count: 0,
                            },
                            env,
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
        env: Environment,
    ) -> Result<VmState, Condition> {
        // If we have a result from a previous step, store it
        if let Some(result) = self.current_result.take()
            && evaluated_count > 0
        {
            args[evaluated_count - 1] = result;
        }

        // If there are more arguments to evaluate
        if evaluated_count < args.len() {
            // Push the call frame back with incremented count
            self.frames.push(Frame::with_env(
                FrameOp::Call {
                    func_name,
                    args: args.clone(),
                    evaluated_count: evaluated_count + 1,
                },
                env.clone(),
            ));

            // Push an eval frame for the next argument
            self.frames.push(Frame::with_env(
                FrameOp::Eval(args[evaluated_count].clone()),
                env,
            ));

            return Ok(VmState::Running);
        }

        // All arguments evaluated - call the function
        // First check if func_name is a local binding pointing to a lambda
        let resolved_name = if let Some(SExpr::Atom(name)) = env.lookup(&func_name) {
            name.clone()
        } else {
            func_name.clone()
        };
        let func = self.lookup_fn(&resolved_name).cloned();
        match func {
            Some(FunctionObj::Builtin { name, func }) => match func(self, &args) {
                Ok(result) => {
                    self.current_result = Some(result);
                    Ok(VmState::Running)
                }
                Err(e) => Err(Condition::Custom {
                    code: "builtin-error".to_string(),
                    message: format!("{}: {}", name, e),
                }),
            },
            Some(FunctionObj::Lambda {
                params,
                body,
                env: captured_env,
            }) => {
                // Check argument count
                if args.len() != params.len() {
                    return Err(Condition::WrongArgumentCount {
                        function: func_name,
                        expected: params.len(),
                        actual: args.len(),
                    });
                }

                // Create child environment from captured env and bind parameters
                let mut new_env = captured_env.child();
                for (param, arg) in params.iter().zip(args.iter()) {
                    new_env.bind(param, arg.clone());
                }

                // Push a frame to evaluate the body in the new environment
                self.frames
                    .push(Frame::with_env(FrameOp::Eval(body.clone()), new_env));
                Ok(VmState::Running)
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
        env: Environment,
    ) -> Result<VmState, Condition> {
        match name.as_str() {
            "quote" => self.step_quote(args),
            "if" => self.step_if(args, state, env),
            "let" => self.step_let(args, state, env),
            "begin" => self.step_begin(args, state, env),
            "->" => self.step_thread_first(args, state, env),
            "->>" => self.step_thread_last(args, state, env),
            "map" => self.step_map(args, state, env),
            "filter" => self.step_filter(args, state, env),
            "reduce" => self.step_reduce(args, state, env),
            "lambda" => self.step_lambda(args, env),
            "defun" => self.step_defun(args, env),
            _ => Err(Condition::Custom {
                code: "unknown-special-form".to_string(),
                message: format!("Unknown special form: {}", name),
            }),
        }
    }

    /// Handles (lambda (params...) body).
    fn step_lambda(&mut self, args: Vec<SExpr>, env: Environment) -> Result<VmState, Condition> {
        if args.len() != 2 {
            return Err(Condition::WrongArgumentCount {
                function: "lambda".to_string(),
                expected: 2,
                actual: args.len(),
            });
        }

        // Extract parameter names
        let params = match &args[0] {
            SExpr::List(items) => {
                let mut param_names = Vec::new();
                for item in items {
                    match item {
                        SExpr::Atom(name) => param_names.push(name.clone()),
                        _ => {
                            return Err(Condition::TypeError {
                                expected: "atom".to_string(),
                                actual: item.clone(),
                            });
                        }
                    }
                }
                param_names
            }
            _ => {
                return Err(Condition::TypeError {
                    expected: "list of parameters".to_string(),
                    actual: args[0].clone(),
                });
            }
        };

        let body = args[1].clone();

        // Capture the current environment
        let captured_env = Arc::new(env);

        // Create the lambda function object
        let lambda = FunctionObj::Lambda {
            params,
            body,
            env: captured_env,
        };

        // Register it in the arena with a generated name
        let id = FunctionId(self.next_func_id);
        self.next_func_id += 1;
        let lambda_name = format!("__lambda_{}", id.0);
        self.functions.insert(id, lambda);
        self.function_names.insert(lambda_name.clone(), id);

        // Return the lambda name as an atom so it can be called
        self.current_result = Some(SExpr::Atom(lambda_name));
        Ok(VmState::Running)
    }

    /// Handles (defun name (params...) body).
    fn step_defun(&mut self, args: Vec<SExpr>, env: Environment) -> Result<VmState, Condition> {
        if args.len() != 3 {
            return Err(Condition::WrongArgumentCount {
                function: "defun".to_string(),
                expected: 3,
                actual: args.len(),
            });
        }

        // Extract function name
        let func_name = match &args[0] {
            SExpr::Atom(name) => name.clone(),
            _ => {
                return Err(Condition::TypeError {
                    expected: "atom (function name)".to_string(),
                    actual: args[0].clone(),
                });
            }
        };

        // Extract parameter names
        let params = match &args[1] {
            SExpr::List(items) => {
                let mut param_names = Vec::new();
                for item in items {
                    match item {
                        SExpr::Atom(name) => param_names.push(name.clone()),
                        _ => {
                            return Err(Condition::TypeError {
                                expected: "atom".to_string(),
                                actual: item.clone(),
                            });
                        }
                    }
                }
                param_names
            }
            _ => {
                return Err(Condition::TypeError {
                    expected: "list of parameters".to_string(),
                    actual: args[1].clone(),
                });
            }
        };

        let body = args[2].clone();

        // Capture the current environment
        let captured_env = Arc::new(env);

        // Create the lambda function object
        let lambda = FunctionObj::Lambda {
            params,
            body,
            env: captured_env,
        };

        // Register it in the arena with the given name (or update if it exists)
        let id = if let Some(&existing_id) = self.function_names.get(&func_name) {
            // Hot-swap: reuse the existing ID
            self.functions.insert(existing_id, lambda);
            existing_id
        } else {
            let id = FunctionId(self.next_func_id);
            self.next_func_id += 1;
            self.functions.insert(id, lambda);
            self.function_names.insert(func_name.clone(), id);
            id
        };

        // Return the function name
        let _ = id; // Silence unused warning
        self.current_result = Some(SExpr::Atom(func_name));
        Ok(VmState::Running)
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
        env: Environment,
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
                let first_arg = args[0].clone();
                self.frames.push(Frame::with_env(
                    FrameOp::SpecialForm {
                        name: "if".to_string(),
                        args,
                        state: SpecialFormState::Index(0),
                    },
                    env.clone(),
                ));
                self.frames
                    .push(Frame::with_env(FrameOp::Eval(first_arg), env));
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
                    .push(Frame::with_env(FrameOp::Eval(branch), env));
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
        mut env: Environment,
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
                    self.frames.push(Frame::with_env(
                        FrameOp::SpecialForm {
                            name: "let".to_string(),
                            args,
                            state: SpecialFormState::Index(bindings.len()),
                        },
                        env,
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
                        self.frames.push(Frame::with_env(
                            FrameOp::SpecialForm {
                                name: "let".to_string(),
                                args,
                                state: SpecialFormState::Index(0),
                            },
                            env.clone(),
                        ));
                        self.frames
                            .push(Frame::with_env(FrameOp::Eval(pair[1].clone()), env));
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
                // Store the evaluated value using Environment::bind
                if idx < bindings.len()
                    && let Some(result) = self.current_result.take()
                    && let SExpr::List(pair) = &bindings[idx]
                {
                    if let SExpr::Atom(var_name) = &pair[0] {
                        env.bind(var_name, result);
                    } else {
                        return Err(Condition::Custom {
                            code: "invalid-binding-name".to_string(),
                            message: "Binding name must be an atom".to_string(),
                        });
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
                        self.frames.push(Frame::with_env(
                            FrameOp::SpecialForm {
                                name: "let".to_string(),
                                args,
                                state: SpecialFormState::Index(idx + 1),
                            },
                            env.clone(),
                        ));
                        self.frames
                            .push(Frame::with_env(FrameOp::Eval(pair[1].clone()), env));
                    }
                    return Ok(VmState::Running);
                }

                // All bindings processed, evaluate body expressions
                let body = &args[1..];
                if body.is_empty() {
                    self.current_result = Some(SExpr::List(vec![]));
                } else if body.len() == 1 {
                    self.frames
                        .push(Frame::with_env(FrameOp::Eval(body[0].clone()), env));
                } else {
                    // Multiple body expressions - use begin
                    self.frames.push(Frame::with_env(
                        FrameOp::SpecialForm {
                            name: "begin".to_string(),
                            args: body.to_vec(),
                            state: SpecialFormState::Index(0),
                        },
                        env.clone(),
                    ));
                    self.frames
                        .push(Frame::with_env(FrameOp::Eval(body[0].clone()), env));
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
        env: Environment,
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
                let first_arg = args[0].clone();
                self.frames.push(Frame::with_env(
                    FrameOp::SpecialForm {
                        name: "begin".to_string(),
                        args,
                        state: SpecialFormState::Index(0),
                    },
                    env.clone(),
                ));
                self.frames
                    .push(Frame::with_env(FrameOp::Eval(first_arg), env));
                Ok(VmState::Running)
            }
            SpecialFormState::Index(idx) => {
                if idx + 1 < args.len() {
                    // More expressions to evaluate
                    let next_arg = args[idx + 1].clone();
                    self.frames.push(Frame::with_env(
                        FrameOp::SpecialForm {
                            name: "begin".to_string(),
                            args,
                            state: SpecialFormState::Index(idx + 1),
                        },
                        env.clone(),
                    ));
                    self.frames
                        .push(Frame::with_env(FrameOp::Eval(next_arg), env));
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
        env: Environment,
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
                let first_arg = args[0].clone();
                self.frames.push(Frame::with_env(
                    FrameOp::SpecialForm {
                        name: "->".to_string(),
                        args,
                        state: SpecialFormState::Index(0),
                    },
                    env.clone(),
                ));
                self.frames
                    .push(Frame::with_env(FrameOp::Eval(first_arg), env));
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

                self.frames.push(Frame::with_env(
                    FrameOp::SpecialForm {
                        name: "->".to_string(),
                        args,
                        state: SpecialFormState::Index(idx + 1),
                    },
                    env.clone(),
                ));
                self.frames
                    .push(Frame::with_env(FrameOp::Eval(threaded), env));
                Ok(VmState::Running)
            }
        }
    }

    /// Handles (->> value form...).
    fn step_thread_last(
        &mut self,
        args: Vec<SExpr>,
        state: SpecialFormState,
        env: Environment,
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
                let first_arg = args[0].clone();
                self.frames.push(Frame::with_env(
                    FrameOp::SpecialForm {
                        name: "->>".to_string(),
                        args,
                        state: SpecialFormState::Index(0),
                    },
                    env.clone(),
                ));
                self.frames
                    .push(Frame::with_env(FrameOp::Eval(first_arg), env));
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

                self.frames.push(Frame::with_env(
                    FrameOp::SpecialForm {
                        name: "->>".to_string(),
                        args,
                        state: SpecialFormState::Index(idx + 1),
                    },
                    env.clone(),
                ));
                self.frames
                    .push(Frame::with_env(FrameOp::Eval(threaded), env));
                Ok(VmState::Running)
            }
        }
    }

    /// Handles (map func list).
    fn step_map(
        &mut self,
        args: Vec<SExpr>,
        state: SpecialFormState,
        env: Environment,
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
                let second_arg = args[1].clone();
                self.frames.push(Frame::with_env(
                    FrameOp::SpecialForm {
                        name: "map".to_string(),
                        args,
                        state: SpecialFormState::Index(0),
                    },
                    env.clone(),
                ));
                self.frames
                    .push(Frame::with_env(FrameOp::Eval(second_arg), env));
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
                    .push(Frame::with_env(FrameOp::Eval(list_expr), env));
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
        env: Environment,
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
                let second_arg = args[1].clone();
                self.frames.push(Frame::with_env(
                    FrameOp::SpecialForm {
                        name: "filter".to_string(),
                        args,
                        state: SpecialFormState::Index(0),
                    },
                    env.clone(),
                ));
                self.frames
                    .push(Frame::with_env(FrameOp::Eval(second_arg), env));
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
                            match func(self, std::slice::from_ref(&item)) {
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
        env: Environment,
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
                let second_arg = args[1].clone();
                self.frames.push(Frame::with_env(
                    FrameOp::SpecialForm {
                        name: "reduce".to_string(),
                        args,
                        state: SpecialFormState::Index(0),
                    },
                    env.clone(),
                ));
                self.frames
                    .push(Frame::with_env(FrameOp::Eval(second_arg), env));
                Ok(VmState::Running)
            }
            SpecialFormState::Index(0) => {
                let init = self.current_result.take().unwrap_or(SExpr::List(vec![]));
                // Now evaluate the list
                // Store init in env temporarily
                let mut new_env = env.clone();
                new_env.bind("__reduce_acc__", init);

                let third_arg = args[2].clone();
                self.frames.push(Frame::with_env(
                    FrameOp::SpecialForm {
                        name: "reduce".to_string(),
                        args,
                        state: SpecialFormState::Index(1),
                    },
                    new_env.clone(),
                ));
                self.frames
                    .push(Frame::with_env(FrameOp::Eval(third_arg), new_env));
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

                let init = env
                    .lookup("__reduce_acc__")
                    .unwrap_or_else(|| SExpr::List(vec![]));

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
                            match func(self, &[acc, item]) {
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

fn null_p(_vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 1 {
        return Err(SError::new("vm")
            .with_code("wrong-argument-count")
            .with_message("null? requires exactly one argument"));
    }
    let result = matches!(&args[0], SExpr::Atom(s) if s == "null");
    Ok(SExpr::Atom(if result { "#t" } else { "#f" }.to_string()))
}

fn list_p(_vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 1 {
        return Err(SError::new("vm")
            .with_code("wrong-argument-count")
            .with_message("list? requires exactly one argument"));
    }
    let result = matches!(&args[0], SExpr::List(_));
    Ok(SExpr::Atom(if result { "#t" } else { "#f" }.to_string()))
}

fn atom_p(_vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 1 {
        return Err(SError::new("vm")
            .with_code("wrong-argument-count")
            .with_message("atom? requires exactly one argument"));
    }
    let result = matches!(&args[0], SExpr::Atom(_));
    Ok(SExpr::Atom(if result { "#t" } else { "#f" }.to_string()))
}

fn empty_p(_vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 1 {
        return Err(SError::new("vm")
            .with_code("wrong-argument-count")
            .with_message("empty? requires exactly one argument"));
    }
    let result = matches!(&args[0], SExpr::List(items) if items.is_empty());
    Ok(SExpr::Atom(if result { "#t" } else { "#f" }.to_string()))
}

fn eq_p(_vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 2 {
        return Err(SError::new("vm")
            .with_code("wrong-argument-count")
            .with_message("eq? requires exactly two arguments"));
    }
    let result = args[0] == args[1];
    Ok(SExpr::Atom(if result { "#t" } else { "#f" }.to_string()))
}

fn first(_vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
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

fn rest(_vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
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

fn cons(_vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
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

fn append(_vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
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

fn length(_vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
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

fn nth(_vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
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

fn list(_vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
    Ok(SExpr::List(args.to_vec()))
}

fn str_concat(_vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
    let mut result = String::new();
    for arg in args {
        result.push_str(&extract_string(arg));
    }
    Ok(string_atom(&result))
}

fn help(_vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
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
        Some(help_text) => Ok(SExpr::Atom(string_atom(help_text).to_string())),
        None => Ok(SExpr::Atom(format!("\"No help available for '{}'\"", name))),
    }
}

// JSON builtins wrappers

fn obj(_vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
    let mut items = vec![SExpr::Atom("obj".to_string())];
    items.extend(args.iter().cloned());
    Ok(SExpr::List(items))
}

fn arr(_vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
    let mut items = vec![SExpr::Atom("arr".to_string())];
    items.extend(args.iter().cloned());
    Ok(SExpr::List(items))
}

fn builtin_get(_vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
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

fn builtin_keys(_vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 1 {
        return Err(SError::new("vm")
            .with_code("wrong-argument-count")
            .with_message("keys requires exactly one argument"));
    }
    Ok(keys(&args[0]))
}

fn builtin_values(_vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 1 {
        return Err(SError::new("vm")
            .with_code("wrong-argument-count")
            .with_message("values requires exactly one argument"));
    }
    Ok(values(&args[0]))
}

fn builtin_assoc(_vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 3 {
        return Err(SError::new("vm")
            .with_code("wrong-argument-count")
            .with_message("assoc requires exactly three arguments"));
    }
    let key = match &args[1] {
        SExpr::Atom(s) => extract_string_key(s),
        _ => {
            return Err(SError::new("vm")
                .with_code("type-error")
                .with_message("assoc key must be a string"));
        }
    };
    Ok(assoc(&args[0], &key, args[2].clone()))
}

fn builtin_dissoc(_vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 2 {
        return Err(SError::new("vm")
            .with_code("wrong-argument-count")
            .with_message("dissoc requires exactly two arguments"));
    }
    let key = match &args[1] {
        SExpr::Atom(s) => extract_string_key(s),
        _ => {
            return Err(SError::new("vm")
                .with_code("type-error")
                .with_message("dissoc key must be a string"));
        }
    };
    Ok(dissoc(&args[0], &key))
}

fn builtin_merge(_vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
    if args.len() < 2 {
        return Err(SError::new("vm")
            .with_code("wrong-argument-count")
            .with_message("merge requires at least two arguments"));
    }
    Ok(merge(args))
}

// ============================================================================
// Filesystem builtins
// ============================================================================

/// Returns a reference to the filesystem or an error if none is set.
fn require_filesystem(vm: &Vm) -> SResult<&dyn Filesystem> {
    vm.filesystem().ok_or_else(|| {
        SError::new("vm")
            .with_code("no-filesystem")
            .with_message("No filesystem is attached to this VM")
    })
}

/// Loads a markdown file and parses it to an s-expression.
///
/// `(load "path.md")` -> s-expression document
fn builtin_load(vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 1 {
        return Err(SError::new("load")
            .with_code("wrong-argument-count")
            .with_message("load requires exactly one argument: the file path")
            .with_atom_field("received", args.len()));
    }
    let path = extract_string(&args[0]);
    let fs = require_filesystem(vm)?;
    let content = fs.read_to_string(&path).map_err(|e| {
        SError::new("filesystem")
            .with_code("read-error")
            .with_message(&e.to_string())
    })?;
    markdown_to_sexpr(&content)
}

/// Saves an s-expression document to a markdown file.
///
/// `(save doc "path.md")` -> writes file, returns path
fn builtin_save(vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 2 {
        return Err(SError::new("save")
            .with_code("wrong-argument-count")
            .with_message("save requires exactly two arguments: document and file path")
            .with_atom_field("received", args.len()));
    }
    let path = extract_string(&args[1]);
    let fs = require_filesystem(vm)?;
    let markdown = sexpr_to_markdown(&args[0])?;
    fs.write_string(&path, &markdown).map_err(|e| {
        SError::new("filesystem")
            .with_code("write-error")
            .with_message(&e.to_string())
    })?;
    Ok(string_atom(&path))
}

/// Lists all markdown files in the filesystem.
///
/// `(list-files)` -> list of file paths
fn builtin_list_files(vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
    if !args.is_empty() {
        return Err(SError::new("list-files")
            .with_code("wrong-argument-count")
            .with_message("list-files takes no arguments")
            .with_atom_field("received", args.len()));
    }
    let fs = require_filesystem(vm)?;
    let files = fs.list_markdown_files().map_err(|e| {
        SError::new("filesystem")
            .with_code("list-error")
            .with_message(&e.to_string())
    })?;
    let items: Vec<SExpr> = files.into_iter().map(|f| string_atom(&f)).collect();
    Ok(SExpr::List(items))
}

/// Splats a document into a directory hierarchy based on header structure.
///
/// `(splat doc "prefix/")` -> list of written file paths
///
/// Given a document and a prefix, this function:
/// 1. Extracts sections from the document using header hierarchy
/// 2. For each top-level section with children, creates `prefix/slug/index.md`
/// 3. For leaf sections, creates `prefix/slug.md`
///
/// Returns a list of all file paths written.
fn builtin_splat(vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
    use crate::markdown::curation::splat;

    if args.len() != 2 {
        return Err(SError::new("splat")
            .with_code("wrong-argument-count")
            .with_message("splat requires exactly two arguments: document and prefix")
            .with_atom_field("received", args.len()));
    }
    let prefix = extract_string(&args[1]);
    let fs = require_filesystem(vm)?;

    splat(&args[0], &prefix, |path, content| {
        fs.write_string(path, content).map_err(|e| {
            SError::new("filesystem")
                .with_code("write-error")
                .with_message(&e.to_string())
        })
    })
}

// ============================================================================
// General filesystem builtins
// ============================================================================

/// Reads a file as a raw string.
///
/// `(fs-read "path")` -> string content
fn builtin_fs_read(vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 1 {
        return Err(SError::new("fs-read")
            .with_code("wrong-argument-count")
            .with_message("fs-read requires exactly one argument: the file path"));
    }
    let path = extract_string(&args[0]);
    let fs = require_filesystem(vm)?;
    let content = fs.read_to_string(&path).map_err(|e| {
        SError::new("fs-read")
            .with_code("read-error")
            .with_message(&e.to_string())
    })?;
    Ok(string_atom(&content))
}

/// Writes a string to a file.
///
/// `(fs-write "path" content)` -> writes file, returns path
fn builtin_fs_write(vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 2 {
        return Err(SError::new("fs-write")
            .with_code("wrong-argument-count")
            .with_message("fs-write requires exactly two arguments: path and content"));
    }
    let path = extract_string(&args[0]);
    let content = extract_string(&args[1]);
    let fs = require_filesystem(vm)?;
    fs.write_string(&path, &content).map_err(|e| {
        SError::new("fs-write")
            .with_code("write-error")
            .with_message(&e.to_string())
    })?;
    Ok(string_atom(&path))
}

/// Appends a string to a file.
///
/// `(fs-append "path" content)` -> appends to file, returns path
fn builtin_fs_append(vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 2 {
        return Err(SError::new("fs-append")
            .with_code("wrong-argument-count")
            .with_message("fs-append requires exactly two arguments: path and content"));
    }
    let path = extract_string(&args[0]);
    let content = extract_string(&args[1]);
    let fs = require_filesystem(vm)?;
    fs.append_string(&path, &content).map_err(|e| {
        SError::new("fs-append")
            .with_code("append-error")
            .with_message(&e.to_string())
    })?;
    Ok(string_atom(&path))
}

/// Checks if a path exists.
///
/// `(fs-exists? "path")` -> #t or #f
fn builtin_fs_exists(vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 1 {
        return Err(SError::new("fs-exists?")
            .with_code("wrong-argument-count")
            .with_message("fs-exists? requires exactly one argument: the file path"));
    }
    let path = extract_string(&args[0]);
    let fs = require_filesystem(vm)?;
    let exists = fs.exists(&path);
    Ok(SExpr::Atom(if exists { "#t" } else { "#f" }.to_string()))
}

/// Checks if a path is a directory.
///
/// `(fs-is-dir? "path")` -> #t or #f
fn builtin_fs_is_dir(vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 1 {
        return Err(SError::new("fs-is-dir?")
            .with_code("wrong-argument-count")
            .with_message("fs-is-dir? requires exactly one argument: the file path"));
    }
    let path = extract_string(&args[0]);
    let fs = require_filesystem(vm)?;
    let is_dir = fs.is_dir(&path);
    Ok(SExpr::Atom(if is_dir { "#t" } else { "#f" }.to_string()))
}

/// Creates a directory.
///
/// `(fs-mkdir "path")` -> returns path on success
fn builtin_fs_mkdir(vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 1 {
        return Err(SError::new("fs-mkdir")
            .with_code("wrong-argument-count")
            .with_message("fs-mkdir requires exactly one argument: the directory path"));
    }
    let path = extract_string(&args[0]);
    let fs = require_filesystem(vm)?;
    fs.mkdir(&path).map_err(|e| {
        SError::new("fs-mkdir")
            .with_code("mkdir-error")
            .with_message(&e.to_string())
    })?;
    Ok(string_atom(&path))
}

/// Creates a directory and all parent directories.
///
/// `(fs-mkdir-all "path")` -> returns path on success
fn builtin_fs_mkdir_all(vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 1 {
        return Err(SError::new("fs-mkdir-all")
            .with_code("wrong-argument-count")
            .with_message("fs-mkdir-all requires exactly one argument: the directory path"));
    }
    let path = extract_string(&args[0]);
    let fs = require_filesystem(vm)?;
    fs.mkdir_all(&path).map_err(|e| {
        SError::new("fs-mkdir-all")
            .with_code("mkdir-error")
            .with_message(&e.to_string())
    })?;
    Ok(string_atom(&path))
}

/// Converts a DirEntry to an S-expression.
fn dir_entry_to_sexpr(entry: &eudaemonty::DirEntry) -> SExpr {
    let type_str = match entry.file_type {
        eudaemonty::FileType::RegularFile => "file",
        eudaemonty::FileType::Directory => "directory",
        eudaemonty::FileType::Symlink => "symlink",
        eudaemonty::FileType::Other => "other",
    };
    SExpr::List(vec![
        SExpr::Atom("stat".to_string()),
        SExpr::List(vec![
            SExpr::Atom("type".to_string()),
            SExpr::Atom(type_str.to_string()),
        ]),
        SExpr::List(vec![
            SExpr::Atom("size".to_string()),
            SExpr::Atom(entry.size.to_string()),
        ]),
        SExpr::List(vec![
            SExpr::Atom("mtime".to_string()),
            SExpr::Atom(entry.mtime_ms.to_string()),
        ]),
        SExpr::List(vec![
            SExpr::Atom("atime".to_string()),
            SExpr::Atom(entry.atime_ms.to_string()),
        ]),
        SExpr::List(vec![
            SExpr::Atom("ino".to_string()),
            SExpr::Atom(entry.ino.to_string()),
        ]),
        SExpr::List(vec![
            SExpr::Atom("dev".to_string()),
            SExpr::Atom(entry.dev.to_string()),
        ]),
    ])
}

/// Lists directory contents.
///
/// `(fs-readdir "path")` -> ((name1 stat1) (name2 stat2) ...)
fn builtin_fs_readdir(vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 1 {
        return Err(SError::new("fs-readdir")
            .with_code("wrong-argument-count")
            .with_message("fs-readdir requires exactly one argument: the directory path"));
    }
    let path = extract_string(&args[0]);
    let fs = require_filesystem(vm)?;
    let entries = fs.read_dir(&path).map_err(|e| {
        SError::new("fs-readdir")
            .with_code("readdir-error")
            .with_message(&e.to_string())
    })?;
    let items: Vec<SExpr> = entries
        .iter()
        .map(|(name, entry)| SExpr::List(vec![string_atom(name), dir_entry_to_sexpr(entry)]))
        .collect();
    Ok(SExpr::List(items))
}

/// Removes an empty directory.
///
/// `(fs-rmdir "path")` -> returns path on success
fn builtin_fs_rmdir(vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 1 {
        return Err(SError::new("fs-rmdir")
            .with_code("wrong-argument-count")
            .with_message("fs-rmdir requires exactly one argument: the directory path"));
    }
    let path = extract_string(&args[0]);
    let fs = require_filesystem(vm)?;
    fs.rmdir(&path).map_err(|e| {
        SError::new("fs-rmdir")
            .with_code("rmdir-error")
            .with_message(&e.to_string())
    })?;
    Ok(string_atom(&path))
}

/// Gets file metadata (following symlinks).
///
/// `(fs-stat "path")` -> (stat (type TYPE) (size SIZE) ...)
fn builtin_fs_stat(vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 1 {
        return Err(SError::new("fs-stat")
            .with_code("wrong-argument-count")
            .with_message("fs-stat requires exactly one argument: the file path"));
    }
    let path = extract_string(&args[0]);
    let fs = require_filesystem(vm)?;
    let entry = fs.stat(&path).map_err(|e| {
        SError::new("fs-stat")
            .with_code("stat-error")
            .with_message(&e.to_string())
    })?;
    Ok(dir_entry_to_sexpr(&entry))
}

/// Gets file metadata (not following symlinks).
///
/// `(fs-lstat "path")` -> (stat (type TYPE) (size SIZE) ...)
fn builtin_fs_lstat(vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 1 {
        return Err(SError::new("fs-lstat")
            .with_code("wrong-argument-count")
            .with_message("fs-lstat requires exactly one argument: the file path"));
    }
    let path = extract_string(&args[0]);
    let fs = require_filesystem(vm)?;
    let entry = fs.lstat(&path).map_err(|e| {
        SError::new("fs-lstat")
            .with_code("lstat-error")
            .with_message(&e.to_string())
    })?;
    Ok(dir_entry_to_sexpr(&entry))
}

/// Removes a file or symbolic link.
///
/// `(fs-unlink "path")` -> returns path on success
fn builtin_fs_unlink(vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 1 {
        return Err(SError::new("fs-unlink")
            .with_code("wrong-argument-count")
            .with_message("fs-unlink requires exactly one argument: the file path"));
    }
    let path = extract_string(&args[0]);
    let fs = require_filesystem(vm)?;
    fs.unlink(&path).map_err(|e| {
        SError::new("fs-unlink")
            .with_code("unlink-error")
            .with_message(&e.to_string())
    })?;
    Ok(string_atom(&path))
}

/// Renames a file or directory.
///
/// `(fs-rename "src" "dst")` -> returns dst path on success
fn builtin_fs_rename(vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 2 {
        return Err(SError::new("fs-rename")
            .with_code("wrong-argument-count")
            .with_message("fs-rename requires exactly two arguments: src and dst paths"));
    }
    let src = extract_string(&args[0]);
    let dst = extract_string(&args[1]);
    let fs = require_filesystem(vm)?;
    fs.rename(&src, &dst).map_err(|e| {
        SError::new("fs-rename")
            .with_code("rename-error")
            .with_message(&e.to_string())
    })?;
    Ok(string_atom(&dst))
}

/// Creates a symbolic link.
///
/// `(fs-symlink "target" "linkpath")` -> returns linkpath on success
fn builtin_fs_symlink(vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 2 {
        return Err(SError::new("fs-symlink")
            .with_code("wrong-argument-count")
            .with_message("fs-symlink requires exactly two arguments: target and linkpath"));
    }
    let target = extract_string(&args[0]);
    let linkpath = extract_string(&args[1]);
    let fs = require_filesystem(vm)?;
    fs.symlink(&target, &linkpath).map_err(|e| {
        SError::new("fs-symlink")
            .with_code("symlink-error")
            .with_message(&e.to_string())
    })?;
    Ok(string_atom(&linkpath))
}

/// Reads the target of a symbolic link.
///
/// `(fs-readlink "path")` -> target string
fn builtin_fs_readlink(vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 1 {
        return Err(SError::new("fs-readlink")
            .with_code("wrong-argument-count")
            .with_message("fs-readlink requires exactly one argument: the symlink path"));
    }
    let path = extract_string(&args[0]);
    let fs = require_filesystem(vm)?;
    let target = fs.readlink(&path).map_err(|e| {
        SError::new("fs-readlink")
            .with_code("readlink-error")
            .with_message(&e.to_string())
    })?;
    Ok(string_atom(&target))
}

/// Creates a hard link.
///
/// `(fs-link "src" "dst")` -> returns dst path on success
fn builtin_fs_link(vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 2 {
        return Err(SError::new("fs-link")
            .with_code("wrong-argument-count")
            .with_message("fs-link requires exactly two arguments: src and dst paths"));
    }
    let src = extract_string(&args[0]);
    let dst = extract_string(&args[1]);
    let fs = require_filesystem(vm)?;
    fs.link(&src, &dst).map_err(|e| {
        SError::new("fs-link")
            .with_code("link-error")
            .with_message(&e.to_string())
    })?;
    Ok(string_atom(&dst))
}

// ============================================================================
// Filesystem debug/introspection builtins
// ============================================================================

/// Returns the filesystem tree as an S-expression string.
///
/// `(fs-tree)` -> tree string, or #f if unsupported
fn builtin_fs_tree(vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
    if !args.is_empty() {
        return Err(SError::new("fs-tree")
            .with_code("wrong-argument-count")
            .with_message("fs-tree takes no arguments"));
    }
    let fs = require_filesystem(vm)?;
    match fs.tree() {
        Some(tree) => Ok(string_atom(&tree)),
        None => Ok(SExpr::Atom("#f".to_string())),
    }
}

/// Returns the number of free blocks in the filesystem.
///
/// `(fs-free-blocks)` -> number, or #f if unsupported
fn builtin_fs_free_blocks(vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
    if !args.is_empty() {
        return Err(SError::new("fs-free-blocks")
            .with_code("wrong-argument-count")
            .with_message("fs-free-blocks takes no arguments"));
    }
    let fs = require_filesystem(vm)?;
    match fs.free_blocks() {
        Some(blocks) => Ok(SExpr::Atom(blocks.to_string())),
        None => Ok(SExpr::Atom("#f".to_string())),
    }
}

/// Returns the total number of blocks in the filesystem.
///
/// `(fs-total-blocks)` -> number, or #f if unsupported
fn builtin_fs_total_blocks(vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
    if !args.is_empty() {
        return Err(SError::new("fs-total-blocks")
            .with_code("wrong-argument-count")
            .with_message("fs-total-blocks takes no arguments"));
    }
    let fs = require_filesystem(vm)?;
    match fs.total_blocks() {
        Some(blocks) => Ok(SExpr::Atom(blocks.to_string())),
        None => Ok(SExpr::Atom("#f".to_string())),
    }
}

/// Returns the filesystem usage percentage.
///
/// `(fs-usage-percent)` -> number (0-100), or #f if unsupported
fn builtin_fs_usage_percent(vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
    if !args.is_empty() {
        return Err(SError::new("fs-usage-percent")
            .with_code("wrong-argument-count")
            .with_message("fs-usage-percent takes no arguments"));
    }
    let fs = require_filesystem(vm)?;
    match fs.usage_percent() {
        Some(percent) => Ok(SExpr::Atom(percent.to_string())),
        None => Ok(SExpr::Atom("#f".to_string())),
    }
}

/// Triggers filesystem garbage collection.
///
/// `(fs-clean)` -> number of blocks reclaimed, or #f if unsupported
fn builtin_fs_clean(vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
    if !args.is_empty() {
        return Err(SError::new("fs-clean")
            .with_code("wrong-argument-count")
            .with_message("fs-clean takes no arguments"));
    }
    let fs = require_filesystem(vm)?;
    match fs.clean() {
        Some(Ok(reclaimed)) => Ok(SExpr::Atom(reclaimed.to_string())),
        Some(Err(e)) => Err(SError::new("fs-clean")
            .with_code("clean-error")
            .with_message(&e.to_string())),
        None => Ok(SExpr::Atom("#f".to_string())),
    }
}

/// Extract a string key from an atom, handling quoted strings.
fn extract_string_key(s: &str) -> String {
    if s.starts_with('"') && s.ends_with('"') && s.len() >= 2 {
        s[1..s.len() - 1].to_string()
    } else {
        s.to_string()
    }
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

    fn add(_vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
        let mut sum = 0i64;
        for arg in args {
            if let SExpr::Atom(s) = arg {
                sum += s.parse::<i64>().unwrap_or(0);
            }
        }
        Ok(SExpr::Atom(sum.to_string()))
    }

    fn double(_vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
        if let Some(SExpr::Atom(s)) = args.first() {
            let n: i64 = s.parse().unwrap_or(0);
            Ok(SExpr::Atom((n * 2).to_string()))
        } else {
            Ok(SExpr::Atom("0".to_string()))
        }
    }

    fn is_even(_vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
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

    // ========================================================================
    // FunctionId tests
    // ========================================================================

    #[test]
    fn function_id_equality() {
        let id1 = FunctionId(0);
        let id2 = FunctionId(0);
        let id3 = FunctionId(1);
        assert_eq!(id1, id2);
        assert_ne!(id1, id3);
        println!("DEBUG: FunctionId equality works correctly");
    }

    #[test]
    fn function_id_copy() {
        let id1 = FunctionId(42);
        let id2 = id1;
        assert_eq!(id1, id2);
        println!("DEBUG: FunctionId is Copy");
    }

    #[test]
    fn function_id_hash() {
        use std::collections::HashSet;
        let mut set = HashSet::new();
        set.insert(FunctionId(0));
        set.insert(FunctionId(1));
        set.insert(FunctionId(0));
        assert_eq!(set.len(), 2);
        println!("DEBUG: FunctionId Hash works correctly");
    }

    #[test]
    fn function_id_debug() {
        let id = FunctionId(123);
        let debug_str = format!("{:?}", id);
        assert!(debug_str.contains("FunctionId"));
        assert!(debug_str.contains("123"));
        println!("DEBUG: FunctionId debug format: {}", debug_str);
    }

    // ========================================================================
    // Condition enum tests
    // ========================================================================

    #[test]
    fn condition_function_not_found_debug() {
        let cond = Condition::FunctionNotFound {
            name: "foo".to_string(),
        };
        let debug_str = format!("{:?}", cond);
        assert!(debug_str.contains("FunctionNotFound"));
        assert!(debug_str.contains("foo"));
        println!("DEBUG: Condition::FunctionNotFound debug: {}", debug_str);
    }

    #[test]
    fn condition_type_error_debug() {
        let cond = Condition::TypeError {
            expected: "list".to_string(),
            actual: SExpr::Atom("not-list".to_string()),
        };
        let debug_str = format!("{:?}", cond);
        assert!(debug_str.contains("TypeError"));
        assert!(debug_str.contains("list"));
        assert!(debug_str.contains("not-list"));
        println!("DEBUG: Condition::TypeError debug: {}", debug_str);
    }

    #[test]
    fn condition_wrong_argument_count_debug() {
        let cond = Condition::WrongArgumentCount {
            function: "cons".to_string(),
            expected: 2,
            actual: 3,
        };
        let debug_str = format!("{:?}", cond);
        assert!(debug_str.contains("WrongArgumentCount"));
        assert!(debug_str.contains("cons"));
        assert!(debug_str.contains("2"));
        assert!(debug_str.contains("3"));
        println!("DEBUG: Condition::WrongArgumentCount debug: {}", debug_str);
    }

    #[test]
    fn condition_custom_debug() {
        let cond = Condition::Custom {
            code: "test-code".to_string(),
            message: "test message".to_string(),
        };
        let debug_str = format!("{:?}", cond);
        assert!(debug_str.contains("Custom"));
        assert!(debug_str.contains("test-code"));
        assert!(debug_str.contains("test message"));
        println!("DEBUG: Condition::Custom debug: {}", debug_str);
    }

    #[test]
    fn condition_clone() {
        let cond1 = Condition::FunctionNotFound {
            name: "bar".to_string(),
        };
        let cond2 = cond1.clone();
        match (cond1, cond2) {
            (
                Condition::FunctionNotFound { name: n1 },
                Condition::FunctionNotFound { name: n2 },
            ) => {
                assert_eq!(n1, n2);
            }
            _ => panic!("Clone did not preserve variant"),
        }
        println!("DEBUG: Condition clone works correctly");
    }

    // ========================================================================
    // condition_to_error tests
    // ========================================================================

    #[test]
    fn condition_to_error_function_not_found() {
        let cond = Condition::FunctionNotFound {
            name: "missing-fn".to_string(),
        };
        let err = condition_to_error(cond);
        let err_str = err.to_string();
        assert!(err_str.contains("function-not-found"));
        assert!(err_str.contains("missing-fn"));
        println!("DEBUG: condition_to_error FunctionNotFound: {}", err_str);
    }

    #[test]
    fn condition_to_error_type_error() {
        let cond = Condition::TypeError {
            expected: "number".to_string(),
            actual: SExpr::Atom("string".to_string()),
        };
        let err = condition_to_error(cond);
        let err_str = err.to_string();
        assert!(err_str.contains("type-error"));
        assert!(err_str.contains("number"));
        println!("DEBUG: condition_to_error TypeError: {}", err_str);
    }

    #[test]
    fn condition_to_error_wrong_argument_count() {
        let cond = Condition::WrongArgumentCount {
            function: "test-fn".to_string(),
            expected: 2,
            actual: 5,
        };
        let err = condition_to_error(cond);
        let err_str = err.to_string();
        assert!(err_str.contains("wrong-argument-count"));
        assert!(err_str.contains("test-fn"));
        println!("DEBUG: condition_to_error WrongArgumentCount: {}", err_str);
    }

    #[test]
    fn condition_to_error_custom() {
        let cond = Condition::Custom {
            code: "my-code".to_string(),
            message: "my message".to_string(),
        };
        let err = condition_to_error(cond);
        let err_str = err.to_string();
        assert!(err_str.contains("my-code"));
        assert!(err_str.contains("my message"));
        println!("DEBUG: condition_to_error Custom: {}", err_str);
    }

    // ========================================================================
    // thread_into helper tests
    // ========================================================================

    #[test]
    fn thread_into_bare_atom_first() {
        let form = SExpr::Atom("func".to_string());
        let value = SExpr::Atom("val".to_string());
        let result = thread_into(&form, value, true);
        match result {
            SExpr::List(items) => {
                assert_eq!(items.len(), 2);
                assert_eq!(items[0], SExpr::Atom("func".to_string()));
                match &items[1] {
                    SExpr::List(quoted) => {
                        assert_eq!(quoted.len(), 2);
                        assert_eq!(quoted[0], SExpr::Atom("quote".to_string()));
                        assert_eq!(quoted[1], SExpr::Atom("val".to_string()));
                    }
                    _ => panic!("Expected quoted value"),
                }
            }
            _ => panic!("Expected list result"),
        }
        println!("DEBUG: thread_into with bare atom first position works");
    }

    #[test]
    fn thread_into_bare_atom_last() {
        let form = SExpr::Atom("func".to_string());
        let value = SExpr::Atom("val".to_string());
        let result = thread_into(&form, value, false);
        match result {
            SExpr::List(items) => {
                assert_eq!(items.len(), 2);
                assert_eq!(items[0], SExpr::Atom("func".to_string()));
            }
            _ => panic!("Expected list result"),
        }
        println!("DEBUG: thread_into with bare atom last position works");
    }

    #[test]
    fn thread_into_list_first() {
        let form = SExpr::List(vec![
            SExpr::Atom("func".to_string()),
            SExpr::Atom("arg1".to_string()),
        ]);
        let value = SExpr::Atom("val".to_string());
        let result = thread_into(&form, value, true);
        match result {
            SExpr::List(items) => {
                assert_eq!(items.len(), 3);
                assert_eq!(items[0], SExpr::Atom("func".to_string()));
                match &items[1] {
                    SExpr::List(quoted) => {
                        assert_eq!(quoted[0], SExpr::Atom("quote".to_string()));
                        assert_eq!(quoted[1], SExpr::Atom("val".to_string()));
                    }
                    _ => panic!("Expected quoted value in first position"),
                }
                assert_eq!(items[2], SExpr::Atom("arg1".to_string()));
            }
            _ => panic!("Expected list result"),
        }
        println!("DEBUG: thread_into list first position inserts after func");
    }

    #[test]
    fn thread_into_list_last() {
        let form = SExpr::List(vec![
            SExpr::Atom("func".to_string()),
            SExpr::Atom("arg1".to_string()),
        ]);
        let value = SExpr::Atom("val".to_string());
        let result = thread_into(&form, value, false);
        match result {
            SExpr::List(items) => {
                assert_eq!(items.len(), 3);
                assert_eq!(items[0], SExpr::Atom("func".to_string()));
                assert_eq!(items[1], SExpr::Atom("arg1".to_string()));
                match &items[2] {
                    SExpr::List(quoted) => {
                        assert_eq!(quoted[0], SExpr::Atom("quote".to_string()));
                        assert_eq!(quoted[1], SExpr::Atom("val".to_string()));
                    }
                    _ => panic!("Expected quoted value in last position"),
                }
            }
            _ => panic!("Expected list result"),
        }
        println!("DEBUG: thread_into list last position appends");
    }

    #[test]
    fn thread_into_empty_list_returns_clone() {
        let form = SExpr::List(vec![]);
        let value = SExpr::Atom("val".to_string());
        let result = thread_into(&form, value, true);
        assert_eq!(result, SExpr::List(vec![]));
        println!("DEBUG: thread_into with empty list returns clone");
    }

    // ========================================================================
    // extract_string_key helper tests
    // ========================================================================

    #[test]
    fn extract_string_key_quoted() {
        let result = extract_string_key("\"hello\"");
        assert_eq!(result, "hello");
        println!("DEBUG: extract_string_key removes quotes");
    }

    #[test]
    fn extract_string_key_unquoted() {
        let result = extract_string_key("hello");
        assert_eq!(result, "hello");
        println!("DEBUG: extract_string_key preserves unquoted");
    }

    #[test]
    fn extract_string_key_single_char_quoted() {
        let result = extract_string_key("\"x\"");
        assert_eq!(result, "x");
        println!("DEBUG: extract_string_key handles single char");
    }

    #[test]
    fn extract_string_key_empty_quoted() {
        let result = extract_string_key("\"\"");
        assert_eq!(result, "");
        println!("DEBUG: extract_string_key handles empty quoted string");
    }

    #[test]
    fn extract_string_key_only_start_quote() {
        let result = extract_string_key("\"hello");
        assert_eq!(result, "\"hello");
        println!("DEBUG: extract_string_key preserves if only start quote");
    }

    #[test]
    fn extract_string_key_only_end_quote() {
        let result = extract_string_key("hello\"");
        assert_eq!(result, "hello\"");
        println!("DEBUG: extract_string_key preserves if only end quote");
    }

    // ========================================================================
    // Builtin error condition tests
    // ========================================================================

    #[test]
    fn builtin_null_p_wrong_arg_count() {
        let vm = Vm::new();
        let result = null_p(&vm, &[]);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("wrong-argument-count"));
        println!("DEBUG: null? with no args returns error");
    }

    #[test]
    fn builtin_null_p_too_many_args() {
        let vm = Vm::new();
        let result = null_p(
            &vm,
            &[SExpr::Atom("a".to_string()), SExpr::Atom("b".to_string())],
        );
        assert!(result.is_err());
        println!("DEBUG: null? with too many args returns error");
    }

    #[test]
    fn builtin_list_p_wrong_arg_count() {
        let vm = Vm::new();
        let result = list_p(&vm, &[]);
        assert!(result.is_err());
        println!("DEBUG: list? with no args returns error");
    }

    #[test]
    fn builtin_atom_p_wrong_arg_count() {
        let vm = Vm::new();
        let result = atom_p(&vm, &[]);
        assert!(result.is_err());
        println!("DEBUG: atom? with no args returns error");
    }

    #[test]
    fn builtin_empty_p_wrong_arg_count() {
        let vm = Vm::new();
        let result = empty_p(&vm, &[]);
        assert!(result.is_err());
        println!("DEBUG: empty? with no args returns error");
    }

    #[test]
    fn builtin_eq_p_wrong_arg_count_zero() {
        let vm = Vm::new();
        let result = eq_p(&vm, &[]);
        assert!(result.is_err());
        println!("DEBUG: eq? with no args returns error");
    }

    #[test]
    fn builtin_eq_p_wrong_arg_count_one() {
        let vm = Vm::new();
        let result = eq_p(&vm, &[SExpr::Atom("a".to_string())]);
        assert!(result.is_err());
        println!("DEBUG: eq? with one arg returns error");
    }

    #[test]
    fn builtin_first_wrong_arg_count() {
        let vm = Vm::new();
        let result = first(&vm, &[]);
        assert!(result.is_err());
        println!("DEBUG: first with no args returns error");
    }

    #[test]
    fn builtin_first_non_list() {
        let vm = Vm::new();
        let result = first(&vm, &[SExpr::Atom("not-a-list".to_string())]);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("type-error"));
        println!("DEBUG: first on non-list returns type error");
    }

    #[test]
    fn builtin_first_empty_list() {
        let vm = Vm::new();
        let result = first(&vm, &[SExpr::List(vec![])]);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), SExpr::Atom("null".to_string()));
        println!("DEBUG: first on empty list returns null");
    }

    #[test]
    fn builtin_rest_wrong_arg_count() {
        let vm = Vm::new();
        let result = rest(&vm, &[]);
        assert!(result.is_err());
        println!("DEBUG: rest with no args returns error");
    }

    #[test]
    fn builtin_rest_non_list() {
        let vm = Vm::new();
        let result = rest(&vm, &[SExpr::Atom("not-a-list".to_string())]);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("type-error"));
        println!("DEBUG: rest on non-list returns type error");
    }

    #[test]
    fn builtin_rest_empty_list() {
        let vm = Vm::new();
        let result = rest(&vm, &[SExpr::List(vec![])]);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), SExpr::List(vec![]));
        println!("DEBUG: rest on empty list returns empty list");
    }

    #[test]
    fn builtin_cons_wrong_arg_count() {
        let vm = Vm::new();
        let result = cons(&vm, &[SExpr::Atom("a".to_string())]);
        assert!(result.is_err());
        println!("DEBUG: cons with one arg returns error");
    }

    #[test]
    fn builtin_cons_second_not_list() {
        let vm = Vm::new();
        let result = cons(
            &vm,
            &[
                SExpr::Atom("a".to_string()),
                SExpr::Atom("not-a-list".to_string()),
            ],
        );
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("type-error"));
        println!("DEBUG: cons with non-list second arg returns type error");
    }

    #[test]
    fn builtin_append_wrong_arg_count() {
        let vm = Vm::new();
        let result = append(&vm, &[SExpr::List(vec![])]);
        assert!(result.is_err());
        println!("DEBUG: append with one arg returns error");
    }

    #[test]
    fn builtin_append_first_not_list() {
        let vm = Vm::new();
        let result = append(
            &vm,
            &[SExpr::Atom("not-a-list".to_string()), SExpr::List(vec![])],
        );
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("type-error"));
        println!("DEBUG: append with non-list first arg returns type error");
    }

    #[test]
    fn builtin_append_second_not_list() {
        let vm = Vm::new();
        let result = append(
            &vm,
            &[SExpr::List(vec![]), SExpr::Atom("not-a-list".to_string())],
        );
        assert!(result.is_err());
        println!("DEBUG: append with non-list second arg returns type error");
    }

    #[test]
    fn builtin_length_wrong_arg_count() {
        let vm = Vm::new();
        let result = length(&vm, &[]);
        assert!(result.is_err());
        println!("DEBUG: length with no args returns error");
    }

    #[test]
    fn builtin_length_non_list() {
        let vm = Vm::new();
        let result = length(&vm, &[SExpr::Atom("not-a-list".to_string())]);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("type-error"));
        println!("DEBUG: length on non-list returns type error");
    }

    #[test]
    fn builtin_nth_wrong_arg_count() {
        let vm = Vm::new();
        let result = nth(&vm, &[SExpr::Atom("0".to_string())]);
        assert!(result.is_err());
        println!("DEBUG: nth with one arg returns error");
    }

    #[test]
    fn builtin_nth_non_integer_index() {
        let vm = Vm::new();
        let result = nth(
            &vm,
            &[
                SExpr::Atom("not-a-number".to_string()),
                SExpr::List(vec![SExpr::Atom("a".to_string())]),
            ],
        );
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("type-error"));
        println!("DEBUG: nth with non-integer index returns type error");
    }

    #[test]
    fn builtin_nth_list_index() {
        let vm = Vm::new();
        let result = nth(
            &vm,
            &[
                SExpr::List(vec![]),
                SExpr::List(vec![SExpr::Atom("a".to_string())]),
            ],
        );
        assert!(result.is_err());
        println!("DEBUG: nth with list as index returns type error");
    }

    #[test]
    fn builtin_nth_non_list_second_arg() {
        let vm = Vm::new();
        let result = nth(
            &vm,
            &[
                SExpr::Atom("0".to_string()),
                SExpr::Atom("not-a-list".to_string()),
            ],
        );
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("type-error"));
        println!("DEBUG: nth with non-list second arg returns type error");
    }

    #[test]
    fn builtin_nth_out_of_bounds() {
        let vm = Vm::new();
        let result = nth(
            &vm,
            &[
                SExpr::Atom("10".to_string()),
                SExpr::List(vec![SExpr::Atom("a".to_string())]),
            ],
        );
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), SExpr::Atom("null".to_string()));
        println!("DEBUG: nth out of bounds returns null");
    }

    #[test]
    fn builtin_list_creates_list() {
        let vm = Vm::new();
        let result = list(
            &vm,
            &[SExpr::Atom("a".to_string()), SExpr::Atom("b".to_string())],
        );
        assert!(result.is_ok());
        assert_eq!(
            result.unwrap(),
            SExpr::List(vec![
                SExpr::Atom("a".to_string()),
                SExpr::Atom("b".to_string()),
            ])
        );
        println!("DEBUG: list creates list from args");
    }

    #[test]
    fn builtin_list_empty() {
        let vm = Vm::new();
        let result = list(&vm, &[]);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), SExpr::List(vec![]));
        println!("DEBUG: list with no args creates empty list");
    }

    // ========================================================================
    // Help builtin tests
    // ========================================================================

    #[test]
    fn builtin_help_wrong_arg_count() {
        let vm = Vm::new();
        let result = help(&vm, &[]);
        assert!(result.is_err());
        println!("DEBUG: help with no args returns error");
    }

    #[test]
    fn builtin_help_non_atom_arg() {
        let vm = Vm::new();
        let result = help(&vm, &[SExpr::List(vec![])]);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("type-error"));
        println!("DEBUG: help with non-atom arg returns type error");
    }

    #[test]
    fn builtin_help_not_found() {
        let vm = Vm::new();
        let result = help(&vm, &[SExpr::Atom("nonexistent-function-xyz".to_string())]);
        assert!(result.is_ok());
        let help_text = result.unwrap();
        match help_text {
            SExpr::Atom(s) => {
                assert!(s.contains("No help available"));
            }
            _ => panic!("Expected atom result"),
        }
        println!("DEBUG: help for unknown function returns no help message");
    }

    // ========================================================================
    // JSON builtin tests
    // ========================================================================

    #[test]
    fn builtin_obj_creates_object() {
        let vm = Vm::new();
        let result = obj(
            &vm,
            &[
                SExpr::Atom("\"key\"".to_string()),
                SExpr::Atom("value".to_string()),
            ],
        );
        assert!(result.is_ok());
        match result.unwrap() {
            SExpr::List(items) => {
                assert_eq!(items[0], SExpr::Atom("obj".to_string()));
                assert_eq!(items.len(), 3);
            }
            _ => panic!("Expected list result"),
        }
        println!("DEBUG: obj creates object list");
    }

    #[test]
    fn builtin_obj_empty() {
        let vm = Vm::new();
        let result = obj(&vm, &[]);
        assert!(result.is_ok());
        match result.unwrap() {
            SExpr::List(items) => {
                assert_eq!(items.len(), 1);
                assert_eq!(items[0], SExpr::Atom("obj".to_string()));
            }
            _ => panic!("Expected list result"),
        }
        println!("DEBUG: obj with no args creates empty object");
    }

    #[test]
    fn builtin_arr_creates_array() {
        let vm = Vm::new();
        let result = arr(
            &vm,
            &[SExpr::Atom("1".to_string()), SExpr::Atom("2".to_string())],
        );
        assert!(result.is_ok());
        match result.unwrap() {
            SExpr::List(items) => {
                assert_eq!(items[0], SExpr::Atom("arr".to_string()));
                assert_eq!(items.len(), 3);
            }
            _ => panic!("Expected list result"),
        }
        println!("DEBUG: arr creates array list");
    }

    #[test]
    fn builtin_arr_empty() {
        let vm = Vm::new();
        let result = arr(&vm, &[]);
        assert!(result.is_ok());
        match result.unwrap() {
            SExpr::List(items) => {
                assert_eq!(items.len(), 1);
                assert_eq!(items[0], SExpr::Atom("arr".to_string()));
            }
            _ => panic!("Expected list result"),
        }
        println!("DEBUG: arr with no args creates empty array");
    }

    #[test]
    fn builtin_get_wrong_arg_count() {
        let vm = Vm::new();
        let result = builtin_get(&vm, &[SExpr::Atom("key".to_string())]);
        assert!(result.is_err());
        println!("DEBUG: get with one arg returns error");
    }

    #[test]
    fn builtin_get_non_atom_key() {
        let vm = Vm::new();
        let result = builtin_get(
            &vm,
            &[
                SExpr::List(vec![]),
                SExpr::List(vec![SExpr::Atom("obj".to_string())]),
            ],
        );
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("type-error"));
        println!("DEBUG: get with non-atom key returns type error");
    }

    #[test]
    fn builtin_get_quoted_key() {
        let vm = Vm::new();
        let obj = SExpr::List(vec![
            SExpr::Atom("obj".to_string()),
            SExpr::Atom("\"key\"".to_string()),
            SExpr::Atom("value".to_string()),
        ]);
        let result = builtin_get(&vm, &[SExpr::Atom("\"key\"".to_string()), obj]);
        assert!(result.is_ok());
        println!("DEBUG: get with quoted key works");
    }

    #[test]
    fn builtin_get_unquoted_key() {
        let vm = Vm::new();
        let obj = SExpr::List(vec![
            SExpr::Atom("obj".to_string()),
            SExpr::Atom("\"key\"".to_string()),
            SExpr::Atom("value".to_string()),
        ]);
        let result = builtin_get(&vm, &[SExpr::Atom("key".to_string()), obj]);
        assert!(result.is_ok());
        println!("DEBUG: get with unquoted key works");
    }

    #[test]
    fn builtin_keys_wrong_arg_count() {
        let vm = Vm::new();
        let result = builtin_keys(&vm, &[]);
        assert!(result.is_err());
        println!("DEBUG: keys with no args returns error");
    }

    #[test]
    fn builtin_keys_on_object() {
        let vm = Vm::new();
        let obj = SExpr::List(vec![
            SExpr::Atom("obj".to_string()),
            SExpr::Atom("\"a\"".to_string()),
            SExpr::Atom("1".to_string()),
            SExpr::Atom("\"b\"".to_string()),
            SExpr::Atom("2".to_string()),
        ]);
        let result = builtin_keys(&vm, &[obj]);
        assert!(result.is_ok());
        println!("DEBUG: keys on object works");
    }

    #[test]
    fn builtin_values_wrong_arg_count() {
        let vm = Vm::new();
        let result = builtin_values(&vm, &[]);
        assert!(result.is_err());
        println!("DEBUG: values with no args returns error");
    }

    #[test]
    fn builtin_values_on_object() {
        let vm = Vm::new();
        let obj = SExpr::List(vec![
            SExpr::Atom("obj".to_string()),
            SExpr::Atom("\"a\"".to_string()),
            SExpr::Atom("1".to_string()),
        ]);
        let result = builtin_values(&vm, &[obj]);
        assert!(result.is_ok());
        println!("DEBUG: values on object works");
    }

    #[test]
    fn builtin_assoc_wrong_arg_count() {
        let vm = Vm::new();
        let result = builtin_assoc(
            &vm,
            &[
                SExpr::List(vec![SExpr::Atom("obj".to_string())]),
                SExpr::Atom("key".to_string()),
            ],
        );
        assert!(result.is_err());
        println!("DEBUG: assoc with two args returns error");
    }

    #[test]
    fn builtin_assoc_non_atom_key() {
        let vm = Vm::new();
        let result = builtin_assoc(
            &vm,
            &[
                SExpr::List(vec![SExpr::Atom("obj".to_string())]),
                SExpr::List(vec![]),
                SExpr::Atom("value".to_string()),
            ],
        );
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("type-error"));
        println!("DEBUG: assoc with non-atom key returns type error");
    }

    #[test]
    fn builtin_assoc_quoted_key() {
        let vm = Vm::new();
        let obj = SExpr::List(vec![SExpr::Atom("obj".to_string())]);
        let result = builtin_assoc(
            &vm,
            &[
                obj,
                SExpr::Atom("\"key\"".to_string()),
                SExpr::Atom("value".to_string()),
            ],
        );
        assert!(result.is_ok());
        println!("DEBUG: assoc with quoted key works");
    }

    #[test]
    fn builtin_dissoc_wrong_arg_count() {
        let vm = Vm::new();
        let result = builtin_dissoc(&vm, &[SExpr::List(vec![SExpr::Atom("obj".to_string())])]);
        assert!(result.is_err());
        println!("DEBUG: dissoc with one arg returns error");
    }

    #[test]
    fn builtin_dissoc_non_atom_key() {
        let vm = Vm::new();
        let result = builtin_dissoc(
            &vm,
            &[
                SExpr::List(vec![SExpr::Atom("obj".to_string())]),
                SExpr::List(vec![]),
            ],
        );
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("type-error"));
        println!("DEBUG: dissoc with non-atom key returns type error");
    }

    #[test]
    fn builtin_dissoc_quoted_key() {
        let vm = Vm::new();
        let obj = SExpr::List(vec![
            SExpr::Atom("obj".to_string()),
            SExpr::Atom("\"key\"".to_string()),
            SExpr::Atom("value".to_string()),
        ]);
        let result = builtin_dissoc(&vm, &[obj, SExpr::Atom("\"key\"".to_string())]);
        assert!(result.is_ok());
        println!("DEBUG: dissoc with quoted key works");
    }

    #[test]
    fn builtin_merge_wrong_arg_count_zero() {
        let vm = Vm::new();
        let result = builtin_merge(&vm, &[]);
        assert!(result.is_err());
        println!("DEBUG: merge with no args returns error");
    }

    #[test]
    fn builtin_merge_wrong_arg_count_one() {
        let vm = Vm::new();
        let result = builtin_merge(&vm, &[SExpr::List(vec![SExpr::Atom("obj".to_string())])]);
        assert!(result.is_err());
        println!("DEBUG: merge with one arg returns error");
    }

    #[test]
    fn builtin_merge_two_objects() {
        let vm = Vm::new();
        let obj1 = SExpr::List(vec![
            SExpr::Atom("obj".to_string()),
            SExpr::Atom("\"a\"".to_string()),
            SExpr::Atom("1".to_string()),
        ]);
        let obj2 = SExpr::List(vec![
            SExpr::Atom("obj".to_string()),
            SExpr::Atom("\"b\"".to_string()),
            SExpr::Atom("2".to_string()),
        ]);
        let result = builtin_merge(&vm, &[obj1, obj2]);
        assert!(result.is_ok());
        println!("DEBUG: merge two objects works");
    }

    // ========================================================================
    // VM run and step tests
    // ========================================================================

    #[test]
    fn vm_run_returns_finished_for_atom() {
        let mut vm = setup_vm();
        vm.load(SExpr::Atom("hello".to_string()));
        let state = vm.run();
        match state {
            VmState::Finished(result) => {
                assert_eq!(result, SExpr::Atom("hello".to_string()));
            }
            other => panic!("Expected Finished, got {:?}", other),
        }
        println!("DEBUG: run returns Finished for atom");
    }

    #[test]
    fn vm_run_returns_finished_for_empty_list() {
        let mut vm = setup_vm();
        vm.load(SExpr::List(vec![]));
        let state = vm.run();
        match state {
            VmState::Finished(result) => {
                assert_eq!(result, SExpr::List(vec![]));
            }
            other => panic!("Expected Finished, got {:?}", other),
        }
        println!("DEBUG: run returns Finished with empty list for empty list input");
    }

    #[test]
    fn vm_lookup_fn_returns_none_for_unknown() {
        let vm = setup_vm();
        assert!(vm.lookup_fn("nonexistent").is_none());
        println!("DEBUG: lookup_fn returns None for unknown function");
    }

    #[test]
    fn vm_lookup_fn_returns_some_for_registered() {
        let vm = setup_vm();
        assert!(vm.lookup_fn("first").is_some());
        println!("DEBUG: lookup_fn returns Some for registered function");
    }

    // ========================================================================
    // Filter function error handling
    // ========================================================================

    #[test]
    fn vm_filter_unknown_function() {
        let mut vm = setup_vm();
        let mut parser = Parser::new("(filter unknown-pred (quote (1 2 3)))");
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("function-not-found"));
        println!("DEBUG: filter with unknown function returns error: {}", err);
    }

    // ========================================================================
    // Reduce function error handling
    // ========================================================================

    #[test]
    fn vm_reduce_unknown_function() {
        let mut vm = setup_vm();
        let mut parser = Parser::new("(reduce unknown-fn 0 (quote (1 2 3)))");
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("function-not-found"));
        println!("DEBUG: reduce with unknown function returns error: {}", err);
    }

    // ========================================================================
    // Restart clone test
    // ========================================================================

    #[test]
    fn restart_clone() {
        let r1 = Restart::UseValue(SExpr::Atom("42".to_string()));
        let r2 = r1.clone();
        match (r1, r2) {
            (Restart::UseValue(v1), Restart::UseValue(v2)) => {
                assert_eq!(v1, v2);
            }
            _ => panic!("Clone did not preserve variant"),
        }
        println!("DEBUG: Restart clone works correctly");
    }

    // ========================================================================
    // VmState clone test
    // ========================================================================

    #[test]
    fn vm_state_clone() {
        let s1 = VmState::Finished(SExpr::Atom("done".to_string()));
        let s2 = s1.clone();
        match (s1, s2) {
            (VmState::Finished(v1), VmState::Finished(v2)) => {
                assert_eq!(v1, v2);
            }
            _ => panic!("Clone did not preserve variant"),
        }
        println!("DEBUG: VmState clone works correctly");
    }

    // ========================================================================
    // FunctionObj clone test
    // ========================================================================

    #[test]
    fn function_obj_builtin_clone() {
        let f1 = FunctionObj::Builtin {
            name: "test".to_string(),
            func: add,
        };
        let f2 = f1.clone();
        match (f1, f2) {
            (FunctionObj::Builtin { name: n1, .. }, FunctionObj::Builtin { name: n2, .. }) => {
                assert_eq!(n1, n2);
            }
            _ => panic!("Clone did not preserve variant"),
        }
        println!("DEBUG: FunctionObj::Builtin clone works correctly");
    }

    #[test]
    fn function_obj_lambda_clone() {
        let env = Arc::new(Environment::new());
        let f1 = FunctionObj::Lambda {
            params: vec!["x".to_string()],
            body: SExpr::Atom("x".to_string()),
            env: Arc::clone(&env),
        };
        let f2 = f1.clone();
        match (f1, f2) {
            (
                FunctionObj::Lambda {
                    params: p1,
                    body: b1,
                    ..
                },
                FunctionObj::Lambda {
                    params: p2,
                    body: b2,
                    ..
                },
            ) => {
                assert_eq!(p1, p2);
                assert_eq!(b1, b2);
            }
            _ => panic!("Clone did not preserve variant"),
        }
        println!("DEBUG: FunctionObj::Lambda clone works correctly");
    }

    // ========================================================================
    // ========================================================================
    // VM with nested function calls
    // ========================================================================

    #[test]
    fn vm_nested_function_calls() {
        let mut vm = setup_vm();
        vm.def_fn("+", add);
        vm.def_fn("double", double);
        let mut parser = Parser::new("(+ (double 5) (double 3))");
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr).unwrap();
        assert_eq!(result, SExpr::Atom("16".to_string()));
        println!("DEBUG: nested function calls evaluate correctly");
    }

    #[test]
    fn vm_deeply_nested_function_calls() {
        let mut vm = setup_vm();
        vm.def_fn("+", add);
        let mut parser = Parser::new("(+ (+ (+ 1 2) 3) 4)");
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr).unwrap();
        assert_eq!(result, SExpr::Atom("10".to_string()));
        println!("DEBUG: deeply nested function calls work");
    }

    // ========================================================================
    // Variable binding edge cases
    // ========================================================================

    #[test]
    fn vm_variable_lookup_in_eval() {
        let mut vm = setup_vm();
        let mut parser = Parser::new("(let ((x 42)) x)");
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr).unwrap();
        assert_eq!(result, SExpr::Atom("42".to_string()));
        println!("DEBUG: variable lookup in let body works");
    }

    #[test]
    fn vm_unbound_variable_evaluates_to_itself() {
        let mut vm = setup_vm();
        let mut parser = Parser::new("unbound-var");
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr).unwrap();
        assert_eq!(result, SExpr::Atom("unbound-var".to_string()));
        println!("DEBUG: unbound variable evaluates to itself");
    }

    // ========================================================================
    // JSON builtins in VM context
    // ========================================================================

    #[test]
    fn vm_json_obj_creation() {
        let mut vm = setup_vm();
        let mut parser = Parser::new("(obj \"name\" \"alice\")");
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr).unwrap();
        match result {
            SExpr::List(items) => {
                assert_eq!(items[0], SExpr::Atom("obj".to_string()));
            }
            _ => panic!("Expected list"),
        }
        println!("DEBUG: obj builtin works in VM context");
    }

    #[test]
    fn vm_json_arr_creation() {
        let mut vm = setup_vm();
        let mut parser = Parser::new("(arr 1 2 3)");
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr).unwrap();
        match result {
            SExpr::List(items) => {
                assert_eq!(items[0], SExpr::Atom("arr".to_string()));
                assert_eq!(items.len(), 4);
            }
            _ => panic!("Expected list"),
        }
        println!("DEBUG: arr builtin works in VM context");
    }

    #[test]
    fn vm_json_get_value() {
        let mut vm = setup_vm();
        let mut parser = Parser::new("(get \"key\" (obj \"key\" \"value\"))");
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr);
        assert!(result.is_ok());
        println!("DEBUG: get builtin works in VM context");
    }

    #[test]
    fn vm_json_keys() {
        let mut vm = setup_vm();
        let mut parser = Parser::new("(keys (obj \"a\" 1 \"b\" 2))");
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr);
        assert!(result.is_ok());
        println!("DEBUG: keys builtin works in VM context");
    }

    #[test]
    fn vm_json_values() {
        let mut vm = setup_vm();
        let mut parser = Parser::new("(values (obj \"a\" 1 \"b\" 2))");
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr);
        assert!(result.is_ok());
        println!("DEBUG: values builtin works in VM context");
    }

    #[test]
    fn vm_json_assoc() {
        let mut vm = setup_vm();
        let mut parser = Parser::new("(assoc (obj) \"key\" \"value\")");
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr);
        assert!(result.is_ok());
        println!("DEBUG: assoc builtin works in VM context");
    }

    #[test]
    fn vm_json_dissoc() {
        let mut vm = setup_vm();
        let mut parser = Parser::new("(dissoc (obj \"key\" \"value\") \"key\")");
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr);
        assert!(result.is_ok());
        println!("DEBUG: dissoc builtin works in VM context");
    }

    #[test]
    fn vm_json_merge() {
        let mut vm = setup_vm();
        let mut parser = Parser::new("(merge (obj \"a\" 1) (obj \"b\" 2))");
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr);
        assert!(result.is_ok());
        println!("DEBUG: merge builtin works in VM context");
    }

    // ========================================================================
    // Lambda tests
    // ========================================================================

    #[test]
    fn vm_lambda_creates_callable() {
        let mut vm = setup_vm();
        vm.def_fn("+", add);
        // Create a lambda and immediately call it
        let mut parser = Parser::new("(let ((f (lambda (x) (+ x 1)))) (f 5))");
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr).unwrap();
        assert_eq!(result, SExpr::Atom("6".to_string()));
        println!("DEBUG: lambda creates callable function: {}", result);
    }

    #[test]
    fn vm_lambda_multiple_params() {
        let mut vm = setup_vm();
        vm.def_fn("+", add);
        let mut parser = Parser::new("(let ((f (lambda (x y) (+ x y)))) (f 3 4))");
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr).unwrap();
        assert_eq!(result, SExpr::Atom("7".to_string()));
        println!("DEBUG: lambda with multiple params: {}", result);
    }

    #[test]
    fn vm_lambda_zero_params() {
        let mut vm = setup_vm();
        let mut parser = Parser::new("(let ((f (lambda () (quote hello)))) (f))");
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr).unwrap();
        assert_eq!(result, SExpr::Atom("hello".to_string()));
        println!("DEBUG: lambda with zero params: {}", result);
    }

    #[test]
    fn vm_lambda_closure_captures_env() {
        let mut vm = setup_vm();
        vm.def_fn("+", add);
        // y is captured from the let binding
        let mut parser = Parser::new("(let ((y 10)) (let ((f (lambda (x) (+ x y)))) (f 5)))");
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr).unwrap();
        assert_eq!(result, SExpr::Atom("15".to_string()));
        println!("DEBUG: lambda captures environment: {}", result);
    }

    #[test]
    fn vm_lambda_wrong_arg_count_syntax() {
        let mut vm = setup_vm();
        let mut parser = Parser::new("(lambda (x))");
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr);
        assert!(result.is_err());
        println!(
            "DEBUG: lambda with wrong syntax returns error: {:?}",
            result
        );
    }

    #[test]
    fn vm_lambda_wrong_arg_count_call() {
        let mut vm = setup_vm();
        vm.def_fn("+", add);
        // Lambda expects 2 args, but we call with 1
        let mut parser = Parser::new("(let ((f (lambda (x y) (+ x y)))) (f 3))");
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("wrong-argument-count"));
        println!("DEBUG: lambda call with wrong arg count: {}", err);
    }

    // ========================================================================
    // Defun tests
    // ========================================================================

    #[test]
    fn vm_defun_defines_function() {
        let mut vm = setup_vm();
        vm.def_fn("+", add);
        // Define add-one and call it
        let mut parser = Parser::new("(begin (defun add-one (x) (+ x 1)) (add-one 5))");
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr).unwrap();
        assert_eq!(result, SExpr::Atom("6".to_string()));
        println!("DEBUG: defun defines callable function: {}", result);
    }

    #[test]
    fn vm_defun_returns_name() {
        let mut vm = setup_vm();
        vm.def_fn("+", add);
        let mut parser = Parser::new("(defun my-func (x) (+ x 1))");
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr).unwrap();
        assert_eq!(result, SExpr::Atom("my-func".to_string()));
        println!("DEBUG: defun returns function name: {}", result);
    }

    #[test]
    fn vm_defun_hot_swap() {
        let mut vm = setup_vm();
        vm.def_fn("+", add);
        // Define a function, call it, redefine it, call again
        let mut parser = Parser::new(
            "(begin (defun f (x) (+ x 1)) (let ((r1 (f 5))) (defun f (x) (+ x 10)) (+ r1 (f 5))))",
        );
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr).unwrap();
        // r1 = 6, f(5) after redefine = 15, total = 21
        assert_eq!(result, SExpr::Atom("21".to_string()));
        println!("DEBUG: defun hot-swap works: {}", result);
    }

    #[test]
    fn vm_defun_wrong_arg_count() {
        let mut vm = setup_vm();
        let mut parser = Parser::new("(defun (x))");
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr);
        assert!(result.is_err());
        println!("DEBUG: defun with wrong syntax returns error: {:?}", result);
    }

    #[test]
    fn vm_defun_non_atom_name() {
        let mut vm = setup_vm();
        let mut parser = Parser::new("(defun (a b) (x) x)");
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("type-error"));
        println!("DEBUG: defun with non-atom name: {}", err);
    }

    #[test]
    fn vm_defun_non_list_params() {
        let mut vm = setup_vm();
        let mut parser = Parser::new("(defun f x x)");
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("type-error"));
        println!("DEBUG: defun with non-list params: {}", err);
    }

    #[test]
    fn vm_defun_recursive() {
        let mut vm = setup_vm();
        vm.def_fn("+", add);

        fn sub(_vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
            let mut result = 0i64;
            for (i, arg) in args.iter().enumerate() {
                if let SExpr::Atom(s) = arg {
                    let n = s.parse::<i64>().unwrap_or(0);
                    if i == 0 {
                        result = n;
                    } else {
                        result -= n;
                    }
                }
            }
            Ok(SExpr::Atom(result.to_string()))
        }

        fn eq(_vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
            if args.len() != 2 {
                return Ok(SExpr::Atom("#f".to_string()));
            }
            let result = args[0] == args[1];
            Ok(SExpr::Atom(if result { "#t" } else { "#f" }.to_string()))
        }

        vm.def_fn("-", sub);
        vm.def_fn("=", eq);

        // Factorial: (defun fact (n) (if (= n 0) 1 (* n (fact (- n 1)))))
        // Use a simpler recursive function: sum from n to 0
        // (defun sum-to (n) (if (= n 0) 0 (+ n (sum-to (- n 1)))))
        let mut parser = Parser::new(
            "(begin (defun sum-to (n) (if (= n 0) 0 (+ n (sum-to (- n 1))))) (sum-to 5))",
        );
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr).unwrap();
        // sum-to(5) = 5 + 4 + 3 + 2 + 1 + 0 = 15
        assert_eq!(result, SExpr::Atom("15".to_string()));
        println!("DEBUG: recursive defun works: {}", result);
    }

    // ========================================================================
    // Comprehensive Lambda tests
    // ========================================================================

    // ------------------------------------------------------------------------
    // Lambda syntax validation tests
    // ------------------------------------------------------------------------

    #[test]
    fn lambda_requires_exactly_two_args() {
        let mut vm = setup_vm();
        let mut parser = Parser::new("(lambda)");
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("wrong-argument-count"));
        println!(
            "DEBUG: lambda with no args returns wrong-argument-count: {}",
            err
        );
    }

    #[test]
    fn lambda_requires_two_args_not_one() {
        let mut vm = setup_vm();
        let mut parser = Parser::new("(lambda (x))");
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("wrong-argument-count"));
        println!(
            "DEBUG: lambda with one arg returns wrong-argument-count: {}",
            err
        );
    }

    #[test]
    fn lambda_requires_two_args_not_three() {
        let mut vm = setup_vm();
        let mut parser = Parser::new("(lambda (x) body extra)");
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("wrong-argument-count"));
        println!(
            "DEBUG: lambda with three args returns wrong-argument-count: {}",
            err
        );
    }

    #[test]
    fn lambda_params_must_be_list() {
        let mut vm = setup_vm();
        let mut parser = Parser::new("(lambda not-a-list body)");
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("type-error"));
        assert!(err.to_string().contains("list of parameters"));
        println!(
            "DEBUG: lambda with non-list params returns type-error: {}",
            err
        );
    }

    #[test]
    fn lambda_param_names_must_be_atoms() {
        let mut vm = setup_vm();
        let mut parser = Parser::new("(lambda ((nested list) x) body)");
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("type-error"));
        assert!(err.to_string().contains("atom"));
        println!(
            "DEBUG: lambda with non-atom param name returns type-error: {}",
            err
        );
    }

    #[test]
    fn lambda_all_params_must_be_atoms() {
        let mut vm = setup_vm();
        let mut parser = Parser::new("(lambda (x (y z)) body)");
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("type-error"));
        println!(
            "DEBUG: lambda with mixed atom/list params returns type-error: {}",
            err
        );
    }

    // ------------------------------------------------------------------------
    // Lambda returns callable reference tests
    // ------------------------------------------------------------------------

    #[test]
    fn lambda_returns_internal_name() {
        let mut vm = setup_vm();
        let mut parser = Parser::new("(lambda (x) x)");
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr).unwrap();
        match result {
            SExpr::Atom(name) => {
                assert!(name.starts_with("__lambda_"));
                println!("DEBUG: lambda returns internal name: {}", name);
            }
            _ => panic!("Expected atom, got {:?}", result),
        }
    }

    #[test]
    fn lambda_sequential_creates_unique_names() {
        let mut vm = setup_vm();
        let mut parser1 = Parser::new("(lambda (x) x)");
        let mut parser2 = Parser::new("(lambda (y) y)");
        let expr1 = parser1.parse().unwrap();
        let expr2 = parser2.parse().unwrap();
        let result1 = vm.eval(&expr1).unwrap();
        let result2 = vm.eval(&expr2).unwrap();
        match (&result1, &result2) {
            (SExpr::Atom(name1), SExpr::Atom(name2)) => {
                assert_ne!(name1, name2);
                println!(
                    "DEBUG: sequential lambdas have unique names: {} vs {}",
                    name1, name2
                );
            }
            _ => panic!("Expected atoms"),
        }
    }

    // ------------------------------------------------------------------------
    // Lambda parameter binding tests
    // ------------------------------------------------------------------------

    #[test]
    fn lambda_binds_single_param() {
        let mut vm = setup_vm();
        let mut parser = Parser::new("(let ((f (lambda (x) x))) (f 42))");
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr).unwrap();
        assert_eq!(result, SExpr::Atom("42".to_string()));
        println!("DEBUG: lambda binds single param correctly: {}", result);
    }

    #[test]
    fn lambda_binds_multiple_params_in_order() {
        let mut vm = setup_vm();
        let mut parser = Parser::new("(let ((f (lambda (a b c) b))) (f 1 2 3))");
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr).unwrap();
        assert_eq!(result, SExpr::Atom("2".to_string()));
        println!(
            "DEBUG: lambda binds multiple params in order, b=2: {}",
            result
        );
    }

    #[test]
    fn lambda_binds_first_param() {
        let mut vm = setup_vm();
        let mut parser = Parser::new("(let ((f (lambda (a b c) a))) (f 1 2 3))");
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr).unwrap();
        assert_eq!(result, SExpr::Atom("1".to_string()));
        println!("DEBUG: lambda binds first param a=1: {}", result);
    }

    #[test]
    fn lambda_binds_last_param() {
        let mut vm = setup_vm();
        let mut parser = Parser::new("(let ((f (lambda (a b c) c))) (f 1 2 3))");
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr).unwrap();
        assert_eq!(result, SExpr::Atom("3".to_string()));
        println!("DEBUG: lambda binds last param c=3: {}", result);
    }

    #[test]
    fn lambda_params_shadow_outer_bindings() {
        let mut vm = setup_vm();
        let mut parser = Parser::new("(let ((x 100)) (let ((f (lambda (x) x))) (f 42)))");
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr).unwrap();
        assert_eq!(result, SExpr::Atom("42".to_string()));
        println!("DEBUG: lambda params shadow outer bindings: {}", result);
    }

    // ------------------------------------------------------------------------
    // Lambda closure/environment capture tests
    // ------------------------------------------------------------------------

    #[test]
    fn lambda_captures_single_binding() {
        let mut vm = setup_vm();
        vm.def_fn("+", add);
        let mut parser = Parser::new("(let ((y 10)) (let ((f (lambda (x) (+ x y)))) (f 5)))");
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr).unwrap();
        assert_eq!(result, SExpr::Atom("15".to_string()));
        println!("DEBUG: lambda captures single binding y=10: {}", result);
    }

    #[test]
    fn lambda_captures_multiple_bindings() {
        let mut vm = setup_vm();
        vm.def_fn("+", add);
        let mut parser = Parser::new(
            "(let ((a 1) (b 2) (c 3)) (let ((f (lambda (x) (+ x (+ a (+ b c)))))) (f 10)))",
        );
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr).unwrap();
        // x=10, a=1, b=2, c=3 -> 10 + 1 + 2 + 3 = 16
        assert_eq!(result, SExpr::Atom("16".to_string()));
        println!(
            "DEBUG: lambda captures multiple bindings a=1,b=2,c=3: {}",
            result
        );
    }

    #[test]
    fn lambda_captures_nested_let_bindings() {
        let mut vm = setup_vm();
        vm.def_fn("+", add);
        let mut parser = Parser::new(
            "(let ((a 1)) (let ((b 2)) (let ((f (lambda (x) (+ x (+ a b))))) (f 10))))",
        );
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr).unwrap();
        // x=10, a=1, b=2 -> 10 + 1 + 2 = 13
        assert_eq!(result, SExpr::Atom("13".to_string()));
        println!(
            "DEBUG: lambda captures nested let bindings a=1,b=2: {}",
            result
        );
    }

    #[test]
    fn lambda_closure_preserves_captured_values() {
        let mut vm = setup_vm();
        vm.def_fn("+", add);
        // Create closure with y=10, then call it after y would be "out of scope"
        let mut parser = Parser::new("(let ((f (let ((y 10)) (lambda (x) (+ x y))))) (f 5))");
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr).unwrap();
        assert_eq!(result, SExpr::Atom("15".to_string()));
        println!(
            "DEBUG: closure preserves captured value y=10 after scope: {}",
            result
        );
    }

    #[test]
    fn lambda_captures_at_definition_time() {
        let mut vm = setup_vm();
        vm.def_fn("+", add);
        // Define closure with y=10, rebind y to 100, call closure - should use 10
        let mut parser =
            Parser::new("(let ((y 10)) (let ((f (lambda (x) (+ x y)))) (let ((y 100)) (f 5))))");
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr).unwrap();
        // Closure captured y=10 at definition, not y=100
        assert_eq!(result, SExpr::Atom("15".to_string()));
        println!(
            "DEBUG: lambda captures at definition time (y=10 not 100): {}",
            result
        );
    }

    // ------------------------------------------------------------------------
    // Lambda argument count validation tests
    // ------------------------------------------------------------------------

    #[test]
    fn lambda_call_too_few_args() {
        let mut vm = setup_vm();
        vm.def_fn("+", add);
        let mut parser = Parser::new("(let ((f (lambda (x y) (+ x y)))) (f 5))");
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("wrong-argument-count"));
        println!("DEBUG: lambda call with too few args: {}", err);
    }

    #[test]
    fn lambda_call_too_many_args() {
        let mut vm = setup_vm();
        vm.def_fn("+", add);
        let mut parser = Parser::new("(let ((f (lambda (x) (+ x 1)))) (f 5 6 7))");
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("wrong-argument-count"));
        println!("DEBUG: lambda call with too many args: {}", err);
    }

    #[test]
    fn lambda_zero_params_requires_zero_args() {
        let mut vm = setup_vm();
        let mut parser = Parser::new("(let ((f (lambda () (quote ok)))) (f extra))");
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("wrong-argument-count"));
        println!("DEBUG: zero-param lambda rejects args: {}", err);
    }

    // ------------------------------------------------------------------------
    // Nested lambda tests
    // ------------------------------------------------------------------------

    #[test]
    fn nested_lambda_inner_captures_outer_param() {
        let mut vm = setup_vm();
        vm.def_fn("+", add);
        // (lambda (x) (lambda (y) (+ x y))) - curried add
        let mut parser = Parser::new(
            "(let ((make-adder (lambda (x) (lambda (y) (+ x y))))) (let ((add5 (make-adder 5))) (add5 3)))",
        );
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr).unwrap();
        assert_eq!(result, SExpr::Atom("8".to_string()));
        println!(
            "DEBUG: nested lambda inner captures outer param (currying): {}",
            result
        );
    }

    #[test]
    fn nested_lambda_multiple_levels() {
        let mut vm = setup_vm();
        vm.def_fn("+", add);
        // Three-level nesting: (lambda (a) (lambda (b) (lambda (c) (+ a (+ b c)))))
        let mut parser = Parser::new(
            "(let ((f (lambda (a) (lambda (b) (lambda (c) (+ a (+ b c))))))) (let ((g (f 1))) (let ((h (g 2))) (h 3))))",
        );
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr).unwrap();
        // a=1, b=2, c=3 -> 1 + 2 + 3 = 6
        assert_eq!(result, SExpr::Atom("6".to_string()));
        println!("DEBUG: three-level nested lambda: {}", result);
    }

    #[test]
    fn lambda_returns_lambda() {
        let mut vm = setup_vm();
        // Lambda that returns another lambda without calling it
        let mut parser = Parser::new("(let ((f (lambda (x) (lambda (y) y)))) (f 1))");
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr).unwrap();
        match result {
            SExpr::Atom(name) => {
                assert!(name.starts_with("__lambda_"));
                println!("DEBUG: lambda can return lambda: {}", name);
            }
            _ => panic!("Expected lambda reference atom"),
        }
    }

    // ------------------------------------------------------------------------
    // Lambda with body expressions tests
    // ------------------------------------------------------------------------

    #[test]
    fn lambda_body_can_use_quote() {
        let mut vm = setup_vm();
        let mut parser = Parser::new("(let ((f (lambda (x) (quote hello)))) (f anything))");
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr).unwrap();
        assert_eq!(result, SExpr::Atom("hello".to_string()));
        println!("DEBUG: lambda body can use quote: {}", result);
    }

    #[test]
    fn lambda_body_can_use_if() {
        let mut vm = setup_vm();
        let mut parser =
            Parser::new("(let ((f (lambda (x) (if x (quote yes) (quote no))))) (f #t))");
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr).unwrap();
        assert_eq!(result, SExpr::Atom("yes".to_string()));
        println!("DEBUG: lambda body can use if (true case): {}", result);
    }

    #[test]
    fn lambda_body_if_false_case() {
        let mut vm = setup_vm();
        let mut parser =
            Parser::new("(let ((f (lambda (x) (if x (quote yes) (quote no))))) (f #f))");
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr).unwrap();
        assert_eq!(result, SExpr::Atom("no".to_string()));
        println!("DEBUG: lambda body can use if (false case): {}", result);
    }

    #[test]
    fn lambda_body_can_use_let() {
        let mut vm = setup_vm();
        vm.def_fn("+", add);
        let mut parser = Parser::new("(let ((f (lambda (x) (let ((y 10)) (+ x y))))) (f 5))");
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr).unwrap();
        assert_eq!(result, SExpr::Atom("15".to_string()));
        println!("DEBUG: lambda body can use let: {}", result);
    }

    #[test]
    fn lambda_body_can_use_begin() {
        let mut vm = setup_vm();
        vm.def_fn("+", add);
        let mut parser =
            Parser::new("(let ((f (lambda (x) (begin (+ x 1) (+ x 2) (+ x 3))))) (f 10))");
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr).unwrap();
        // begin returns last expression: 10 + 3 = 13
        assert_eq!(result, SExpr::Atom("13".to_string()));
        println!("DEBUG: lambda body can use begin: {}", result);
    }

    // ------------------------------------------------------------------------
    // Lambda with builtins tests
    // ------------------------------------------------------------------------

    #[test]
    fn lambda_can_call_builtins() {
        let mut vm = setup_vm();
        let mut parser = Parser::new("(let ((f (lambda (lst) (first lst)))) (f (quote (a b c))))");
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr).unwrap();
        assert_eq!(result, SExpr::Atom("a".to_string()));
        println!("DEBUG: lambda can call builtins (first): {}", result);
    }

    #[test]
    fn lambda_with_rest_builtin() {
        let mut vm = setup_vm();
        let mut parser = Parser::new("(let ((f (lambda (lst) (rest lst)))) (f (quote (a b c))))");
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr).unwrap();
        assert_eq!(
            result,
            SExpr::List(vec![
                SExpr::Atom("b".to_string()),
                SExpr::Atom("c".to_string())
            ])
        );
        println!("DEBUG: lambda can call rest builtin: {:?}", result);
    }

    #[test]
    fn lambda_with_cons_builtin() {
        let mut vm = setup_vm();
        let mut parser =
            Parser::new("(let ((f (lambda (x lst) (cons x lst)))) (f (quote a) (quote (b c))))");
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr).unwrap();
        assert_eq!(
            result,
            SExpr::List(vec![
                SExpr::Atom("a".to_string()),
                SExpr::Atom("b".to_string()),
                SExpr::Atom("c".to_string())
            ])
        );
        println!("DEBUG: lambda can call cons builtin: {:?}", result);
    }

    #[test]
    fn lambda_with_length_builtin() {
        let mut vm = setup_vm();
        let mut parser =
            Parser::new("(let ((f (lambda (lst) (length lst)))) (f (quote (a b c d e))))");
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr).unwrap();
        assert_eq!(result, SExpr::Atom("5".to_string()));
        println!("DEBUG: lambda can call length builtin: {}", result);
    }

    // ------------------------------------------------------------------------
    // Lambda with map/filter/reduce tests
    // ------------------------------------------------------------------------

    #[test]
    fn lambda_used_with_map() {
        let mut vm = setup_vm();
        vm.def_fn("+", add);
        // map expects a function name, and lambda returns one
        let mut parser =
            Parser::new("(let ((inc (lambda (x) (+ x 1)))) (map inc (quote (1 2 3))))");
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr).unwrap();
        assert_eq!(
            result,
            SExpr::List(vec![
                SExpr::Atom("2".to_string()),
                SExpr::Atom("3".to_string()),
                SExpr::Atom("4".to_string())
            ])
        );
        println!("DEBUG: lambda used with map: {:?}", result);
    }

    // ------------------------------------------------------------------------
    // Lambda identity and simple transformations
    // ------------------------------------------------------------------------

    #[test]
    fn lambda_identity_function() {
        let mut vm = setup_vm();
        let mut parser = Parser::new("(let ((id (lambda (x) x))) (id (quote (a b c))))");
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr).unwrap();
        assert_eq!(
            result,
            SExpr::List(vec![
                SExpr::Atom("a".to_string()),
                SExpr::Atom("b".to_string()),
                SExpr::Atom("c".to_string())
            ])
        );
        println!("DEBUG: identity lambda: {:?}", result);
    }

    #[test]
    fn lambda_constant_function() {
        let mut vm = setup_vm();
        let mut parser =
            Parser::new("(let ((always42 (lambda (x) 42))) (always42 (quote anything)))");
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr).unwrap();
        assert_eq!(result, SExpr::Atom("42".to_string()));
        println!("DEBUG: constant lambda returns 42: {}", result);
    }

    #[test]
    fn lambda_swap_function() {
        let mut vm = setup_vm();
        let mut parser = Parser::new("(let ((swap (lambda (a b) (list b a)))) (swap 1 2))");
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr).unwrap();
        assert_eq!(
            result,
            SExpr::List(vec![
                SExpr::Atom("2".to_string()),
                SExpr::Atom("1".to_string())
            ])
        );
        println!("DEBUG: swap lambda: {:?}", result);
    }

    // ------------------------------------------------------------------------
    // Lambda with list arguments tests
    // ------------------------------------------------------------------------

    #[test]
    fn lambda_accepts_list_argument() {
        let mut vm = setup_vm();
        let mut parser = Parser::new("(let ((f (lambda (lst) (first lst)))) (f (quote (1 2 3))))");
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr).unwrap();
        assert_eq!(result, SExpr::Atom("1".to_string()));
        println!("DEBUG: lambda accepts list argument: {}", result);
    }

    #[test]
    fn lambda_accepts_empty_list_argument() {
        let mut vm = setup_vm();
        let mut parser = Parser::new("(let ((f (lambda (lst) (empty? lst)))) (f (quote ())))");
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr).unwrap();
        assert_eq!(result, SExpr::Atom("#t".to_string()));
        println!("DEBUG: lambda accepts empty list argument: {}", result);
    }

    // ------------------------------------------------------------------------
    // Lambda reuse tests
    // ------------------------------------------------------------------------

    #[test]
    fn lambda_can_be_called_multiple_times() {
        let mut vm = setup_vm();
        vm.def_fn("+", add);
        let mut parser = Parser::new("(let ((f (lambda (x) (+ x 1)))) (+ (f 1) (+ (f 2) (f 3))))");
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr).unwrap();
        // f(1)=2, f(2)=3, f(3)=4 -> 2+3+4 = 9
        assert_eq!(result, SExpr::Atom("9".to_string()));
        println!(
            "DEBUG: lambda can be called multiple times: f(1)+f(2)+f(3)={}",
            result
        );
    }

    #[test]
    fn lambda_stateless_across_calls() {
        let mut vm = setup_vm();
        vm.def_fn("+", add);
        // Each call to f should be independent
        let mut parser =
            Parser::new("(let ((f (lambda (x) (let ((y 10)) (+ x y))))) (+ (f 1) (f 2)))");
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr).unwrap();
        // f(1)=11, f(2)=12 -> 23
        assert_eq!(result, SExpr::Atom("23".to_string()));
        println!("DEBUG: lambda stateless across calls: {}", result);
    }

    // ------------------------------------------------------------------------
    // Lambda edge cases
    // ------------------------------------------------------------------------

    #[test]
    fn lambda_empty_param_list() {
        let mut vm = setup_vm();
        let mut parser = Parser::new("(let ((f (lambda () (quote constant)))) (f))");
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr).unwrap();
        assert_eq!(result, SExpr::Atom("constant".to_string()));
        println!("DEBUG: lambda with empty param list: {}", result);
    }

    #[test]
    fn lambda_many_params() {
        let mut vm = setup_vm();
        vm.def_fn("+", add);
        let mut parser =
            Parser::new("(let ((f (lambda (a b c d e) (+ a (+ b (+ c (+ d e))))))) (f 1 2 3 4 5))");
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr).unwrap();
        assert_eq!(result, SExpr::Atom("15".to_string()));
        println!("DEBUG: lambda with many params: {}", result);
    }

    #[test]
    fn lambda_body_evaluates_atom() {
        let mut vm = setup_vm();
        // Body is just an atom, should evaluate to that atom
        let mut parser = Parser::new("(let ((f (lambda (x) x))) (f hello))");
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr).unwrap();
        assert_eq!(result, SExpr::Atom("hello".to_string()));
        println!("DEBUG: lambda body evaluates atom: {}", result);
    }

    #[test]
    fn lambda_body_evaluates_to_empty_list() {
        let mut vm = setup_vm();
        let mut parser = Parser::new("(let ((f (lambda (x) (quote ())))) (f anything))");
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr).unwrap();
        assert_eq!(result, SExpr::List(vec![]));
        println!("DEBUG: lambda body evaluates to empty list: {:?}", result);
    }

    #[test]
    fn lambda_arg_evaluation_order() {
        let mut vm = setup_vm();
        vm.def_fn("+", add);
        // Arguments should be evaluated left-to-right before call
        // We test by building a list to verify order
        let mut parser =
            Parser::new("(let ((f (lambda (a b c) (list a b c)))) (f (+ 1 0) (+ 2 0) (+ 3 0)))");
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr).unwrap();
        assert_eq!(
            result,
            SExpr::List(vec![
                SExpr::Atom("1".to_string()),
                SExpr::Atom("2".to_string()),
                SExpr::Atom("3".to_string())
            ])
        );
        println!("DEBUG: lambda arg evaluation order: {:?}", result);
    }

    // ------------------------------------------------------------------------
    // Lambda interaction with thread macros
    // ------------------------------------------------------------------------

    #[test]
    fn lambda_in_thread_first() {
        let mut vm = setup_vm();
        vm.def_fn("+", add);
        let mut parser = Parser::new("(let ((f (lambda (x) (+ x 10)))) (-> 5 f))");
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr).unwrap();
        assert_eq!(result, SExpr::Atom("15".to_string()));
        println!("DEBUG: lambda in thread-first: {}", result);
    }

    #[test]
    fn lambda_in_thread_last() {
        let mut vm = setup_vm();
        vm.def_fn("+", add);
        let mut parser = Parser::new("(let ((f (lambda (x) (+ x 10)))) (->> 5 f))");
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr).unwrap();
        assert_eq!(result, SExpr::Atom("15".to_string()));
        println!("DEBUG: lambda in thread-last: {}", result);
    }

    // ------------------------------------------------------------------------
    // Lambda with globals tests
    // ------------------------------------------------------------------------

    #[test]
    fn lambda_can_access_globals() {
        let mut vm = setup_vm();
        vm.def_fn("+", add);
        vm.bind_global("global-val", SExpr::Atom("100".to_string()));
        let mut parser = Parser::new("(let ((f (lambda (x) (+ x global-val)))) (f 5))");
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr).unwrap();
        assert_eq!(result, SExpr::Atom("105".to_string()));
        println!("DEBUG: lambda can access globals: {}", result);
    }

    #[test]
    fn lambda_param_shadows_global() {
        let mut vm = setup_vm();
        vm.bind_global("x", SExpr::Atom("global".to_string()));
        let mut parser = Parser::new("(let ((f (lambda (x) x))) (f (quote local)))");
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr).unwrap();
        assert_eq!(result, SExpr::Atom("local".to_string()));
        println!("DEBUG: lambda param shadows global: {}", result);
    }

    // ------------------------------------------------------------------------
    // Lambda FunctionObj structure tests
    // ------------------------------------------------------------------------

    #[test]
    fn lambda_function_obj_captures_empty_env() {
        let params = vec![];
        let body = SExpr::Atom("x".to_string());
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
                assert!(p.is_empty());
                assert_eq!(b, body);
                assert!(e.bindings.is_empty());
                println!("DEBUG: Lambda FunctionObj with empty params and env");
            }
            _ => panic!("Expected Lambda variant"),
        }
    }

    #[test]
    fn lambda_function_obj_captures_populated_env() {
        let params = vec!["x".to_string()];
        let body = SExpr::Atom("x".to_string());
        let mut parent_env = Environment::new();
        parent_env.bind("captured", SExpr::Atom("value".to_string()));
        let env = Arc::new(parent_env);
        let lambda = FunctionObj::Lambda {
            params: params.clone(),
            body: body.clone(),
            env: Arc::clone(&env),
        };
        match lambda {
            FunctionObj::Lambda { env: e, .. } => {
                assert!(e.lookup("captured").is_some());
                assert_eq!(e.lookup("captured"), Some(SExpr::Atom("value".to_string())));
                println!("DEBUG: Lambda FunctionObj captures populated env");
            }
            _ => panic!("Expected Lambda variant"),
        }
    }

    // ------------------------------------------------------------------------
    // Lambda calling lambda tests
    // ------------------------------------------------------------------------

    #[test]
    fn lambda_calls_another_lambda() {
        let mut vm = setup_vm();
        vm.def_fn("+", add);
        let mut parser =
            Parser::new("(let ((g (lambda (x) (+ x 1)))) (let ((f (lambda (y) (g y)))) (f 5)))");
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr).unwrap();
        assert_eq!(result, SExpr::Atom("6".to_string()));
        println!("DEBUG: lambda calls another lambda: {}", result);
    }

    #[test]
    fn lambda_passes_lambda_as_argument() {
        let mut vm = setup_vm();
        // apply takes a function reference name and an arg, calls (f arg)
        let mut parser = Parser::new(
            "(let ((apply-fn (lambda (f x) (f x)))) (let ((inc (lambda (n) (cons n (quote ()))))) (apply-fn inc 42)))",
        );
        let expr = parser.parse().unwrap();
        let result = vm.eval(&expr).unwrap();
        assert_eq!(result, SExpr::List(vec![SExpr::Atom("42".to_string())]));
        println!("DEBUG: lambda passes lambda as argument: {:?}", result);
    }
}

//! REPL for evaluating s-expressions against markdown documents in a directory.
//!
//! Provides an interactive environment for:
//! - Reading and writing markdown files directly from disk
//! - Parsing markdown to s-expressions
//! - Evaluating s-expressions with registered markdown functions
//! - Converting s-expressions back to markdown
//!
//! The REPL uses a stackless VM for evaluation, enabling:
//! - Steppable execution for debugging
//! - Restarts for error recovery without unwinding
//! - Multi-line s-expression input with parenthesis validation

use std::borrow::Cow;
use std::fs;
use std::path::{Path, PathBuf};

use rustyline::EditMode;
use rustyline::completion::{Completer, Pair};
use rustyline::config::Configurer;
use rustyline::error::ReadlineError;
use rustyline::highlight::{CmdKind, Highlighter};
use rustyline::hint::Hinter;
use rustyline::validate::{ValidationContext, ValidationResult, Validator};
use rustyline::{Context, Editor, Helper};

use super::error::{SError, SResult};
use super::expr::{Parser, SExpr};
use super::filesystem::DirectoryFilesystem;
use super::markdown::curation::{
    LinkInfo, extract_sections, find_undefined_references, generate_toc, get_external_links,
    get_image_links, get_internal_links, link_info_to_sexpr, mark_deprecated, normalize_headers,
    scan_link_definitions, scan_links_to_sexpr, section_to_doc, slugify_text, update_link,
    wrap_in_callout, wrap_in_details,
};
use super::markdown::mutations::{
    append_child, graft, hoist, insert_after, insert_before, prepend_child, prune, replace_at,
};
use super::markdown::{
    get_frontmatter, get_frontmatter_content, get_frontmatter_field, markdown_to_sexpr,
    parse_yaml_frontmatter, remove_frontmatter, remove_frontmatter_field, set_frontmatter,
    sexpr_to_markdown, upsert_frontmatter_field,
};
use super::nodeid::{
    NodeId, PathId, get_by_path, get_context, get_node, get_parent, get_siblings,
    to_annotated_sexpr,
};
use super::util::{extract_string, find_markdown_files, string_atom};
use super::vm::{Restart, Vm, VmState};

// NOTE: All builtin functions take `_vm: &Vm` as the first parameter but don't use it.
// This is because the BuiltinFn signature requires it for filesystem access in other builtins.

// ============================================================================
// LispHelper - rustyline integration for multi-line input and autocomplete
// ============================================================================

/// Helper struct for rustyline that provides:
/// - Multi-line s-expression input via parenthesis validation
/// - Contextual autocomplete for function names
#[derive(Default)]
pub struct LispHelper {
    /// Known function names for autocomplete.
    function_names: Vec<String>,
}

impl LispHelper {
    /// Creates a new helper with the given function names for autocomplete.
    pub fn new(function_names: Vec<String>) -> Self {
        LispHelper { function_names }
    }
}

impl Validator for LispHelper {
    fn validate(&self, ctx: &mut ValidationContext) -> Result<ValidationResult, ReadlineError> {
        let input = ctx.input();
        if is_balanced(input) {
            Ok(ValidationResult::Valid(None))
        } else {
            Ok(ValidationResult::Incomplete)
        }
    }
}

impl Completer for LispHelper {
    type Candidate = Pair;

    fn complete(
        &self,
        line: &str,
        pos: usize,
        _ctx: &Context<'_>,
    ) -> Result<(usize, Vec<Pair>), ReadlineError> {
        // Find the start of the current word
        let start = line[..pos]
            .rfind(|c: char| c.is_whitespace() || c == '(' || c == ')')
            .map(|i| i + 1)
            .unwrap_or(0);
        let prefix = &line[start..pos];

        if prefix.is_empty() {
            return Ok((pos, Vec::new()));
        }

        let matches: Vec<Pair> = self
            .function_names
            .iter()
            .filter(|name| name.starts_with(prefix))
            .map(|name| Pair {
                display: name.clone(),
                replacement: name.clone(),
            })
            .collect();

        Ok((start, matches))
    }
}

impl Hinter for LispHelper {
    type Hint = String;

    fn hint(&self, _line: &str, _pos: usize, _ctx: &Context<'_>) -> Option<Self::Hint> {
        None
    }
}

impl Highlighter for LispHelper {
    fn highlight<'l>(&self, line: &'l str, _pos: usize) -> Cow<'l, str> {
        Cow::Borrowed(line)
    }

    fn highlight_char(&self, _line: &str, _pos: usize, _kind: CmdKind) -> bool {
        false
    }
}

impl Helper for LispHelper {}

/// Checks if parentheses are balanced in the input string.
/// Returns true if balanced, false if more closing parens are needed.
fn is_balanced(input: &str) -> bool {
    let mut depth = 0i32;
    let mut in_string = false;
    let mut escape_next = false;

    for c in input.chars() {
        if escape_next {
            escape_next = false;
            continue;
        }

        match c {
            '\\' if in_string => {
                escape_next = true;
            }
            '"' => {
                in_string = !in_string;
            }
            '(' if !in_string => {
                depth += 1;
            }
            ')' if !in_string => {
                depth -= 1;
            }
            _ => {}
        }
    }

    depth <= 0 && !in_string
}

// ============================================================================
// Repl
// ============================================================================

/// REPL state containing the working directory.
///
/// Uses a stackless VM for evaluation, enabling steppable execution and restarts.
/// Documents are read from and written to the filesystem on each operation.
pub struct Repl {
    /// Current working directory.
    working_dir: PathBuf,
}

impl Repl {
    /// Creates a new REPL rooted at the given directory.
    pub fn new(dir: impl AsRef<Path>) -> SResult<Self> {
        let working_dir = dir.as_ref().to_path_buf();
        if !working_dir.is_dir() {
            return Err(SError::new("repl")
                .with_code("not-a-directory")
                .with_message("Path is not a directory")
                .with_string_field("path", &working_dir.display().to_string()));
        }

        Ok(Repl { working_dir })
    }

    /// Creates a new VM with all builtins registered (no filesystem).
    fn create_vm(&self) -> Vm {
        let mut vm = Vm::new();
        vm.register_builtins();
        vm.register_json_builtins();
        vm.register_filesystem_builtins();
        register_markdown_builtins(&mut vm);
        vm
    }

    /// Creates a VM with filesystem attached.
    ///
    /// The filesystem is rooted at the REPL's working directory and allows
    /// file operations via builtins like `load`, `save`, `list-files`, etc.
    fn create_vm_with_filesystem(&self) -> SResult<Vm> {
        let mut vm = self.create_vm();
        let fs = DirectoryFilesystem::new(&self.working_dir)?;
        vm.set_filesystem(Box::new(fs));
        Ok(vm)
    }

    /// Returns the list of known function names for autocomplete.
    fn get_function_names(&self) -> Vec<String> {
        // Core builtins
        let names = vec![
            "null?",
            "list?",
            "atom?",
            "empty?",
            "eq?",
            "first",
            "rest",
            "cons",
            "append",
            "length",
            "nth",
            "list",
            "help",
            "quote",
            "if",
            "let",
            "begin",
            "->",
            "->>",
            "map",
            "filter",
            "reduce",
            "obj",
            "arr",
            "get",
            "keys",
            "values",
            "assoc",
            "dissoc",
            "merge",
            "markdown-to-sexpr",
            "sexpr-to-markdown",
            "get-frontmatter",
            "get-frontmatter-content",
            "set-frontmatter",
            "remove-frontmatter",
            "parse-yaml-frontmatter",
            "get-fm-field",
            "upsert-fm-field",
            "remove-fm-field",
            "get-by-path",
            "get-node",
            "get-parent",
            "get-siblings",
            "get-context",
            "annotate",
            "replace-at",
            "prune",
            "insert-before",
            "insert-after",
            "append-child",
            "prepend-child",
            "hoist",
            "graft",
            "wrap-in-callout",
            "wrap-in-details",
            "normalize-headers",
            "mark-deprecated",
            "generate-toc",
            "scan-links",
            "get-internal-links",
            "get-external-links",
            "get-image-links",
            "scan-link-defs",
            "find-undef-refs",
            "update-link",
            "extract-sections",
            "section-to-doc",
            "slugify",
            // Filesystem builtins
            "load",
            "save",
            "list-files",
            "read-file",
            "write-file",
            "file-exists?",
        ];
        names.iter().map(|s| s.to_string()).collect()
    }

    /// Returns the current working directory.
    pub fn working_dir(&self) -> &Path {
        &self.working_dir
    }

    /// Lists all markdown files in the working directory (recursively).
    pub fn list_files(&self) -> SResult<Vec<String>> {
        Ok(find_markdown_files(&self.working_dir))
    }

    /// Reads a markdown file from disk and returns it as an s-expression.
    pub fn read_file(&self, filename: &str) -> SResult<SExpr> {
        let path = self.working_dir.join(filename);
        let content = fs::read_to_string(&path).map_err(|e| {
            SError::new("repl")
                .with_code("io-error")
                .with_message("Failed to read file")
                .with_string_field("path", &path.display().to_string())
                .with_string_field("error", &e.to_string())
        })?;

        markdown_to_sexpr(&content)
    }

    /// Writes an s-expression document back to a file.
    pub fn write_file(&self, filename: &str, doc: &SExpr) -> SResult<()> {
        let markdown = sexpr_to_markdown(doc)?;
        let path = self.working_dir.join(filename);
        fs::write(&path, markdown).map_err(|e| {
            SError::new("repl")
                .with_code("io-error")
                .with_message("Failed to write file")
                .with_string_field("path", &path.display().to_string())
                .with_string_field("error", &e.to_string())
        })
    }

    /// Edits a document by reading from disk, applying a transform, and writing back.
    ///
    /// The transform is an s-expression that takes the document as its first argument
    /// (thread-first style). The result of the transform is written back to disk.
    ///
    /// # Arguments
    ///
    /// * `filename` - The name of the file to edit (relative to working directory).
    /// * `transform` - An s-expression string representing the transform to apply.
    ///
    /// # Examples
    ///
    /// ```text
    /// :edit readme.md (prune "1.2")
    /// :edit readme.md (replace-at "1" (h1 "New Title"))
    /// ```
    pub fn edit_document(&self, filename: &str, transform: &str) -> SResult<()> {
        // Read document from disk
        let doc = self.read_file(filename)?;

        // Parse the transform expression
        let mut parser = Parser::new(transform);
        let transform_expr = parser.parse()?;

        // Build the expression with document as first argument (thread-first style)
        // (prune "1.2") becomes (prune doc "1.2")
        let expr = match transform_expr {
            SExpr::List(items) if !items.is_empty() => {
                // Insert the document as the first argument after the function name
                let mut new_items = Vec::with_capacity(items.len() + 1);
                new_items.push(items[0].clone()); // function name
                new_items.push(SExpr::List(vec![
                    SExpr::Atom("quote".to_string()),
                    doc.clone(),
                ]));
                new_items.extend(items[1..].iter().cloned()); // remaining args
                SExpr::List(new_items)
            }
            SExpr::Atom(func_name) => {
                // Single function name: (func doc)
                SExpr::List(vec![
                    SExpr::Atom(func_name),
                    SExpr::List(vec![SExpr::Atom("quote".to_string()), doc.clone()]),
                ])
            }
            _ => {
                return Err(SError::new("repl")
                    .with_code("invalid-transform")
                    .with_message("Transform must be a function call or function name")
                    .with_field("transform", transform_expr));
            }
        };

        // Create VM with filesystem and evaluate
        let mut vm = self.create_vm_with_filesystem()?;
        let result = vm.eval(&expr)?;

        // Write result back to disk
        self.write_file(filename, &result)
    }

    /// Evaluates an s-expression string in the REPL environment.
    ///
    /// Use `(load "filename.md")` to load files from the working directory.
    pub fn eval(&self, input: &str) -> SResult<SExpr> {
        let mut parser = Parser::new(input);
        let expr = parser.parse()?;

        let mut vm = self.create_vm_with_filesystem()?;
        vm.eval(&expr)
    }

    /// Runs the interactive REPL loop using rustyline with vim keybindings.
    ///
    /// Features:
    /// - Multi-line s-expression input (continues prompting until parens are balanced)
    /// - Tab completion for function names
    /// - Steppable VM with restart support for error recovery
    pub fn run_interactive(&self) -> SResult<()> {
        let helper = LispHelper::new(self.get_function_names());
        let mut rl: Editor<LispHelper, rustyline::history::DefaultHistory> = Editor::new()
            .map_err(|e| {
                SError::new("repl")
                    .with_code("readline-error")
                    .with_message("Failed to initialize readline")
                    .with_string_field("error", &e.to_string())
            })?;

        rl.set_helper(Some(helper));
        rl.set_edit_mode(EditMode::Vi);

        // Load history from file if it exists
        let history_path = self.working_dir.join(".lispdown_history");
        let _ = rl.load_history(&history_path);

        println!("lispdown REPL (vim mode)");
        println!("Working directory: {}", self.working_dir.display());
        println!("Type :help for commands, :quit to exit\n");

        loop {
            match rl.readline("λ> ") {
                Ok(line) => {
                    let input = line.trim();
                    if input.is_empty() {
                        continue;
                    }

                    // Add to history
                    let _ = rl.add_history_entry(&line);

                    // Handle REPL commands
                    if input.starts_with(':') {
                        match self.handle_command(input) {
                            Ok(true) => continue,
                            Ok(false) => break,
                            Err(e) => {
                                println!("Error: {}", e);
                                continue;
                            }
                        }
                    }

                    // Evaluate with VM, supporting restarts
                    self.eval_with_restarts(input, &mut rl);
                }
                Err(ReadlineError::Interrupted) => {
                    println!("^C");
                    continue;
                }
                Err(ReadlineError::Eof) => {
                    println!("^D");
                    break;
                }
                Err(err) => {
                    println!("Error: {:?}", err);
                    break;
                }
            }
        }

        // Save history
        let _ = rl.save_history(&history_path);

        Ok(())
    }

    /// Evaluates input with VM restart support.
    fn eval_with_restarts(
        &self,
        input: &str,
        rl: &mut Editor<LispHelper, rustyline::history::DefaultHistory>,
    ) {
        let mut parser = Parser::new(input);
        let expr = match parser.parse() {
            Ok(e) => e,
            Err(e) => {
                println!("Parse error: {}", e);
                return;
            }
        };

        let mut vm = match self.create_vm_with_filesystem() {
            Ok(vm) => vm,
            Err(e) => {
                println!("Error creating VM: {}", e);
                return;
            }
        };

        vm.load(expr);

        loop {
            match vm.run() {
                VmState::Finished(result) => {
                    println!("{}", result);
                    break;
                }
                VmState::Suspended(condition) => {
                    println!("Condition: {:?}", condition);
                    println!("Restarts:");
                    println!("  [1] abort - abandon computation");
                    println!("  [2] use <expr> - supply a replacement value");

                    // Loop until user provides a valid restart choice
                    loop {
                        match rl.readline("restart> ") {
                            Ok(choice) => {
                                let choice = choice.trim();
                                if choice == "1" || choice == "abort" {
                                    vm.reset();
                                    println!("Aborted.");
                                    return;
                                } else if choice == "2" || choice.starts_with("use ") {
                                    let val_input = if choice.starts_with("use ") {
                                        choice.strip_prefix("use ").unwrap().to_string()
                                    } else {
                                        // Prompt for value
                                        match rl.readline("value> ") {
                                            Ok(v) => v.trim().to_string(),
                                            Err(ReadlineError::Interrupted) => {
                                                println!("^C - still in restart loop");
                                                continue;
                                            }
                                            Err(ReadlineError::Eof) => {
                                                vm.reset();
                                                println!("Aborted.");
                                                return;
                                            }
                                            Err(_) => continue,
                                        }
                                    };

                                    let mut val_parser = Parser::new(&val_input);
                                    match val_parser.parse() {
                                        Ok(val) => {
                                            vm.apply_restart(Restart::UseValue(val));
                                            break; // Exit restart loop, continue VM execution
                                        }
                                        Err(e) => {
                                            println!("Parse error: {} - try again", e);
                                            continue;
                                        }
                                    }
                                } else {
                                    println!("Invalid choice. Enter 1, abort, 2, or use <expr>");
                                    continue;
                                }
                            }
                            Err(ReadlineError::Interrupted) => {
                                println!("^C - still in restart loop (use 'abort' to exit)");
                                continue;
                            }
                            Err(ReadlineError::Eof) => {
                                vm.reset();
                                println!("Aborted.");
                                return;
                            }
                            Err(_) => continue,
                        }
                    }
                }
                VmState::Running => {
                    unreachable!("run() should not return Running");
                }
            }
        }
    }

    /// Handles a REPL command (lines starting with ':').
    /// Returns Ok(true) to continue, Ok(false) to quit.
    fn handle_command(&self, input: &str) -> SResult<bool> {
        let parts: Vec<&str> = input.split_whitespace().collect();
        if parts.is_empty() {
            return Ok(true);
        }

        match parts[0] {
            ":quit" | ":q" | ":exit" => return Ok(false),

            ":help" | ":h" | ":?" => {
                print_help();
            }

            ":ls" | ":list" => {
                let files = self.list_files()?;
                if files.is_empty() {
                    println!("No markdown files in directory");
                } else {
                    for f in files {
                        println!("  {}", f);
                    }
                }
            }

            ":show" => {
                if parts.len() < 2 {
                    return Err(SError::new("repl")
                        .with_code("missing-argument")
                        .with_message("Usage: :show <filename>"));
                }
                let doc = self.read_file(parts[1])?;
                println!("{}", doc);
            }

            ":md" => {
                if parts.len() < 2 {
                    return Err(SError::new("repl")
                        .with_code("missing-argument")
                        .with_message("Usage: :md <filename>"));
                }
                let doc = self.read_file(parts[1])?;
                let md = sexpr_to_markdown(&doc)?;
                println!("{}", md);
            }

            ":annotate" | ":ann" => {
                if parts.len() < 2 {
                    return Err(SError::new("repl")
                        .with_code("missing-argument")
                        .with_message("Usage: :annotate <filename>"));
                }
                let doc = self.read_file(parts[1])?;
                let annotated = to_annotated_sexpr(&doc);
                println!("{}", annotated);
            }

            ":pwd" => {
                println!("{}", self.working_dir.display());
            }

            ":fns" | ":functions" => {
                print_functions();
            }

            ":edit" | ":e" => {
                if parts.len() < 3 {
                    return Err(SError::new("repl")
                        .with_code("missing-argument")
                        .with_message("Usage: :edit <filename> <transform>"));
                }
                let filename = parts[1];
                // Reconstruct the transform expression from the remaining parts
                let transform = parts[2..].join(" ");
                self.edit_document(filename, &transform)?;
                println!("Edited and saved: {}", filename);
            }

            _ => {
                println!("Unknown command: {}. Type :help for commands.", parts[0]);
            }
        }

        Ok(true)
    }
}

/// Prints help message.
fn print_help() {
    println!(
        "Commands:
  :help, :h, :?     Show this help
  :quit, :q, :exit  Exit the REPL
  :ls, :list        List markdown files in directory
  :show <file>      Show document as s-expression (reads from disk)
  :md <file>        Show document as markdown (reads from disk)
  :annotate <file>  Show document with path/content IDs
  :edit <file> <transform>  Edit document (reads, transforms, writes back)
  :pwd              Print working directory
  :fns              List available functions

S-expression evaluation:
  Type any s-expression to evaluate it.
  All markdown files in the directory are available as variables by filename.
  Files are read fresh from disk on each evaluation.
  
Examples:
  (get-by-path readme.md \"1\")
  (generate-toc readme.md)
  (->> readme.md (generate-toc))
  :edit readme.md (prune \"1.2\")"
    );
}

/// Prints available functions.
fn print_functions() {
    println!(
        r#"Markdown Functions:
  (markdown-to-sexpr str)        Parse markdown to s-expr
  (sexpr-to-markdown expr)       Convert s-expr to markdown

Frontmatter:
  (get-frontmatter doc)          Get frontmatter node
  (get-frontmatter-content doc)  Get frontmatter as string
  (set-frontmatter doc fmt str)  Set frontmatter (fmt: "yaml"/"toml")
  (remove-frontmatter doc)       Remove frontmatter
  (parse-yaml-frontmatter str)   Parse YAML to object
  (get-fm-field obj key)         Get field from parsed frontmatter
  (upsert-fm-field str k v)      Update/insert field in YAML
  (remove-fm-field str key)      Remove field from YAML

Node Navigation:
  (get-by-path doc path)         Get node by path (e.g., "1.2")
  (get-node doc id)              Get node by path or content ID
  (get-parent doc path)          Get parent node
  (get-siblings doc path)        Get sibling nodes
  (get-context doc path radius)  Get surrounding nodes
  (annotate doc)                 Add path/content IDs to all nodes

Mutations:
  (replace-at doc path node)     Replace node at path
  (prune doc path)               Remove node at path
  (insert-before doc path node)  Insert before node
  (insert-after doc path node)   Insert after node
  (append-child doc path node)   Append child to node
  (prepend-child doc path node)  Prepend child to node
  (hoist doc path delta)         Change heading level
  (graft doc src dst idx)        Move subtree

Curation:
  (wrap-in-callout doc paths type title)  Wrap in blockquote callout
  (wrap-in-details doc paths summary)     Wrap in HTML details
  (normalize-headers doc map)             Remap heading levels
  (mark-deprecated doc paths reason link) Add deprecation notice
  (generate-toc doc)                      Generate table of contents

Links:
  (scan-links doc)               Find all links
  (get-internal-links doc)       Get internal links
  (get-external-links doc)       Get external links
  (get-image-links doc)          Get image links
  (scan-link-defs doc)           Find link definitions
  (find-undef-refs doc)          Find undefined references
  (update-link doc path url)     Update a link's URL

Standard builtins: first, rest, cons, append, length, nth, list,
  null?, list?, atom?, empty?, eq?, map, filter, reduce
"#
    );
}

// ============================================================================
// Markdown builtin functions
// ============================================================================

/// Registers all markdown-related functions in the VM.
pub fn register_markdown_builtins(vm: &mut Vm) {
    // Conversion
    vm.def_fn("markdown-to-sexpr", builtin_markdown_to_sexpr);
    vm.def_fn("sexpr-to-markdown", builtin_sexpr_to_markdown);

    // Frontmatter
    vm.def_fn("get-frontmatter", builtin_get_frontmatter);
    vm.def_fn("get-frontmatter-content", builtin_get_frontmatter_content);
    vm.def_fn("set-frontmatter", builtin_set_frontmatter);
    vm.def_fn("remove-frontmatter", builtin_remove_frontmatter);
    vm.def_fn("parse-yaml-frontmatter", builtin_parse_yaml_frontmatter);
    vm.def_fn("get-fm-field", builtin_get_fm_field);
    vm.def_fn("upsert-fm-field", builtin_upsert_fm_field);
    vm.def_fn("remove-fm-field", builtin_remove_fm_field);

    // Node navigation
    vm.def_fn("get-by-path", builtin_get_by_path);
    vm.def_fn("get-node", builtin_get_node);
    vm.def_fn("get-parent", builtin_get_parent);
    vm.def_fn("get-siblings", builtin_get_siblings);
    vm.def_fn("get-context", builtin_get_context);
    vm.def_fn("annotate", builtin_annotate);

    // Mutations
    vm.def_fn("replace-at", builtin_replace_at);
    vm.def_fn("prune", builtin_prune);
    vm.def_fn("insert-before", builtin_insert_before);
    vm.def_fn("insert-after", builtin_insert_after);
    vm.def_fn("append-child", builtin_append_child);
    vm.def_fn("prepend-child", builtin_prepend_child);
    vm.def_fn("hoist", builtin_hoist);
    vm.def_fn("graft", builtin_graft);

    // Curation
    vm.def_fn("wrap-in-callout", builtin_wrap_in_callout);
    vm.def_fn("wrap-in-details", builtin_wrap_in_details);
    vm.def_fn("normalize-headers", builtin_normalize_headers);
    vm.def_fn("mark-deprecated", builtin_mark_deprecated);
    vm.def_fn("generate-toc", builtin_generate_toc);

    // Links
    vm.def_fn("scan-links", builtin_scan_links);
    vm.def_fn("get-internal-links", builtin_get_internal_links);
    vm.def_fn("get-external-links", builtin_get_external_links);
    vm.def_fn("get-image-links", builtin_get_image_links);
    vm.def_fn("scan-link-defs", builtin_scan_link_defs);
    vm.def_fn("find-undef-refs", builtin_find_undef_refs);
    vm.def_fn("update-link", builtin_update_link);

    // Sectioning
    vm.def_fn("extract-sections", builtin_extract_sections);
    vm.def_fn("section-to-doc", builtin_section_to_doc);
    vm.def_fn("slugify", builtin_slugify);
}

// Conversion functions

fn builtin_markdown_to_sexpr(_vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 1 {
        return Err(SError::new("markdown-to-sexpr")
            .with_code("wrong-argument-count")
            .with_message("Requires exactly one string argument")
            .with_atom_field("received", args.len()));
    }
    let md = extract_string(&args[0]);
    markdown_to_sexpr(&md)
}

fn builtin_sexpr_to_markdown(_vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 1 {
        return Err(SError::new("sexpr-to-markdown")
            .with_code("wrong-argument-count")
            .with_message("Requires exactly one s-expression argument")
            .with_atom_field("received", args.len()));
    }
    let md = sexpr_to_markdown(&args[0])?;
    Ok(string_atom(&md))
}

// Frontmatter functions

fn builtin_get_frontmatter(_vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 1 {
        return Err(SError::new("get-frontmatter")
            .with_code("wrong-argument-count")
            .with_message("Requires exactly one document argument")
            .with_atom_field("received", args.len()));
    }
    match get_frontmatter(&args[0]) {
        Some(fm) => Ok(fm),
        None => Ok(SExpr::Atom("null".to_string())),
    }
}

fn builtin_get_frontmatter_content(_vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 1 {
        return Err(SError::new("get-frontmatter-content")
            .with_code("wrong-argument-count")
            .with_message("Requires exactly one document argument")
            .with_atom_field("received", args.len()));
    }
    match get_frontmatter_content(&args[0]) {
        Some(content) => Ok(string_atom(&content)),
        None => Ok(SExpr::Atom("null".to_string())),
    }
}

fn builtin_set_frontmatter(_vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 3 {
        return Err(SError::new("set-frontmatter")
            .with_code("wrong-argument-count")
            .with_message("Requires document, format, and content arguments")
            .with_atom_field("received", args.len()));
    }
    let format = extract_string(&args[1]);
    let content = extract_string(&args[2]);
    set_frontmatter(&args[0], &format, &content)
}

fn builtin_remove_frontmatter(_vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 1 {
        return Err(SError::new("remove-frontmatter")
            .with_code("wrong-argument-count")
            .with_message("Requires exactly one document argument")
            .with_atom_field("received", args.len()));
    }
    Ok(remove_frontmatter(&args[0]))
}

fn builtin_parse_yaml_frontmatter(_vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 1 {
        return Err(SError::new("parse-yaml-frontmatter")
            .with_code("wrong-argument-count")
            .with_message("Requires exactly one string argument")
            .with_atom_field("received", args.len()));
    }
    let content = extract_string(&args[0]);
    Ok(parse_yaml_frontmatter(&content))
}

fn builtin_get_fm_field(_vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 2 {
        return Err(SError::new("get-fm-field")
            .with_code("wrong-argument-count")
            .with_message("Requires frontmatter object and key")
            .with_atom_field("received", args.len()));
    }
    let key = extract_string(&args[1]);
    match get_frontmatter_field(&args[0], &key) {
        Some(value) => Ok(string_atom(&value)),
        None => Ok(SExpr::Atom("null".to_string())),
    }
}

fn builtin_upsert_fm_field(_vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 3 {
        return Err(SError::new("upsert-fm-field")
            .with_code("wrong-argument-count")
            .with_message("Requires content, key, and value")
            .with_atom_field("received", args.len()));
    }
    let content = extract_string(&args[0]);
    let key = extract_string(&args[1]);
    let value = extract_string(&args[2]);
    let result = upsert_frontmatter_field(&content, &key, &value);
    Ok(string_atom(&result))
}

fn builtin_remove_fm_field(_vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 2 {
        return Err(SError::new("remove-fm-field")
            .with_code("wrong-argument-count")
            .with_message("Requires content and key")
            .with_atom_field("received", args.len()));
    }
    let content = extract_string(&args[0]);
    let key = extract_string(&args[1]);
    let result = remove_frontmatter_field(&content, &key);
    Ok(string_atom(&result))
}

// Node navigation functions

fn builtin_get_by_path(_vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 2 {
        return Err(SError::new("get-by-path")
            .with_code("wrong-argument-count")
            .with_message("Requires document and path")
            .with_atom_field("received", args.len()));
    }
    let path_str = extract_string(&args[1]);
    let path = PathId::parse(&path_str)?;
    match get_by_path(&args[0], &path) {
        Some(node) => Ok(node),
        None => Ok(SExpr::Atom("null".to_string())),
    }
}

fn builtin_get_node(_vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 2 {
        return Err(SError::new("get-node")
            .with_code("wrong-argument-count")
            .with_message("Requires document and node ID")
            .with_atom_field("received", args.len()));
    }
    let id_str = extract_string(&args[1]);
    let id = NodeId::parse(&id_str)?;
    match get_node(&args[0], &id) {
        Some((_path, node)) => Ok(node),
        None => Ok(SExpr::Atom("null".to_string())),
    }
}

fn builtin_get_parent(_vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 2 {
        return Err(SError::new("get-parent")
            .with_code("wrong-argument-count")
            .with_message("Requires document and path")
            .with_atom_field("received", args.len()));
    }
    let path_str = extract_string(&args[1]);
    let path = PathId::parse(&path_str)?;
    match get_parent(&args[0], &path) {
        Some((_parent_path, node)) => Ok(node),
        None => Ok(SExpr::Atom("null".to_string())),
    }
}

fn builtin_get_siblings(_vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 2 {
        return Err(SError::new("get-siblings")
            .with_code("wrong-argument-count")
            .with_message("Requires document and path")
            .with_atom_field("received", args.len()));
    }
    let path_str = extract_string(&args[1]);
    let path = PathId::parse(&path_str)?;
    let siblings = get_siblings(&args[0], &path);
    Ok(SExpr::List(siblings.into_iter().map(|(_, n)| n).collect()))
}

fn builtin_get_context(_vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 3 {
        return Err(SError::new("get-context")
            .with_code("wrong-argument-count")
            .with_message("Requires document, path, and radius")
            .with_atom_field("received", args.len()));
    }
    let path_str = extract_string(&args[1]);
    let path = PathId::parse(&path_str)?;
    let radius = extract_string(&args[2]).parse::<usize>().map_err(|_| {
        SError::new("get-context")
            .with_code("invalid-radius")
            .with_message("Radius must be a non-negative integer")
    })?;
    let context = get_context(&args[0], &path, radius);
    Ok(SExpr::List(context.into_iter().map(|(_, n)| n).collect()))
}

fn builtin_annotate(_vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 1 {
        return Err(SError::new("annotate")
            .with_code("wrong-argument-count")
            .with_message("Requires exactly one document argument")
            .with_atom_field("received", args.len()));
    }
    Ok(to_annotated_sexpr(&args[0]))
}

// Mutation functions

fn builtin_replace_at(_vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 3 {
        return Err(SError::new("replace-at")
            .with_code("wrong-argument-count")
            .with_message("Requires document, path, and new node")
            .with_atom_field("received", args.len()));
    }
    let path_str = extract_string(&args[1]);
    let path = PathId::parse(&path_str)?;
    replace_at(&args[0], &path, args[2].clone())
}

fn builtin_prune(_vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 2 {
        return Err(SError::new("prune")
            .with_code("wrong-argument-count")
            .with_message("Requires document and path")
            .with_atom_field("received", args.len()));
    }
    let path_str = extract_string(&args[1]);
    let path = PathId::parse(&path_str)?;
    prune(&args[0], &path)
}

fn builtin_insert_before(_vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 3 {
        return Err(SError::new("insert-before")
            .with_code("wrong-argument-count")
            .with_message("Requires document, path, and new node")
            .with_atom_field("received", args.len()));
    }
    let path_str = extract_string(&args[1]);
    let path = PathId::parse(&path_str)?;
    insert_before(&args[0], &path, args[2].clone())
}

fn builtin_insert_after(_vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 3 {
        return Err(SError::new("insert-after")
            .with_code("wrong-argument-count")
            .with_message("Requires document, path, and new node")
            .with_atom_field("received", args.len()));
    }
    let path_str = extract_string(&args[1]);
    let path = PathId::parse(&path_str)?;
    insert_after(&args[0], &path, args[2].clone())
}

fn builtin_append_child(_vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 3 {
        return Err(SError::new("append-child")
            .with_code("wrong-argument-count")
            .with_message("Requires document, parent path, and child node")
            .with_atom_field("received", args.len()));
    }
    let path_str = extract_string(&args[1]);
    let path = PathId::parse(&path_str)?;
    append_child(&args[0], &path, args[2].clone())
}

fn builtin_prepend_child(_vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 3 {
        return Err(SError::new("prepend-child")
            .with_code("wrong-argument-count")
            .with_message("Requires document, parent path, and child node")
            .with_atom_field("received", args.len()));
    }
    let path_str = extract_string(&args[1]);
    let path = PathId::parse(&path_str)?;
    prepend_child(&args[0], &path, args[2].clone())
}

fn builtin_hoist(_vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 3 {
        return Err(SError::new("hoist")
            .with_code("wrong-argument-count")
            .with_message("Requires document, path, and delta")
            .with_atom_field("received", args.len()));
    }
    let path_str = extract_string(&args[1]);
    let path = PathId::parse(&path_str)?;
    let delta = extract_string(&args[2]).parse::<i32>().map_err(|_| {
        SError::new("hoist")
            .with_code("invalid-delta")
            .with_message("Delta must be an integer")
    })?;
    hoist(&args[0], &path, delta)
}

fn builtin_graft(_vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 4 {
        return Err(SError::new("graft")
            .with_code("wrong-argument-count")
            .with_message("Requires document, source path, target parent path, and target index")
            .with_atom_field("received", args.len()));
    }
    let src_str = extract_string(&args[1]);
    let src_path = PathId::parse(&src_str)?;
    let dst_str = extract_string(&args[2]);
    let dst_path = PathId::parse(&dst_str)?;
    let idx = extract_string(&args[3]).parse::<usize>().map_err(|_| {
        SError::new("graft")
            .with_code("invalid-index")
            .with_message("Target index must be a non-negative integer")
    })?;
    graft(&args[0], &src_path, &dst_path, idx)
}

// Curation functions

fn builtin_wrap_in_callout(_vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 4 {
        return Err(SError::new("wrap-in-callout")
            .with_code("wrong-argument-count")
            .with_message("Requires document, paths list, callout type, and title")
            .with_atom_field("received", args.len()));
    }
    let paths = parse_path_list(&args[1])?;
    let callout_type = extract_string(&args[2]);
    let title = extract_string(&args[3]);
    let title_opt = if title.is_empty() {
        None
    } else {
        Some(title.as_str())
    };
    wrap_in_callout(&args[0], &paths, &callout_type, title_opt)
}

fn builtin_wrap_in_details(_vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 3 {
        return Err(SError::new("wrap-in-details")
            .with_code("wrong-argument-count")
            .with_message("Requires document, paths list, and summary text")
            .with_atom_field("received", args.len()));
    }
    let paths = parse_path_list(&args[1])?;
    let summary = extract_string(&args[2]);
    wrap_in_details(&args[0], &paths, &summary)
}

fn builtin_normalize_headers(_vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 2 {
        return Err(SError::new("normalize-headers")
            .with_code("wrong-argument-count")
            .with_message("Requires document and depth map")
            .with_atom_field("received", args.len()));
    }
    let depth_map = parse_depth_map(&args[1])?;
    normalize_headers(&args[0], &depth_map)
}

/// Parses a depth map from s-expression format to a Vec of tuples.
/// Expected format: ((1 2) (2 3) ...) mapping old depth to new depth.
fn parse_depth_map(expr: &SExpr) -> SResult<Vec<(u8, u8)>> {
    let mut map = Vec::new();
    if let SExpr::List(items) = expr {
        for item in items {
            if let SExpr::List(pair) = item
                && pair.len() == 2
            {
                let from = extract_string(&pair[0]).parse::<u8>().map_err(|_| {
                    SError::new("normalize-headers")
                        .with_code("invalid-depth")
                        .with_message("Depth must be 1-6")
                })?;
                let to = extract_string(&pair[1]).parse::<u8>().map_err(|_| {
                    SError::new("normalize-headers")
                        .with_code("invalid-depth")
                        .with_message("Depth must be 1-6")
                })?;
                map.push((from, to));
            }
        }
    }
    Ok(map)
}

fn builtin_mark_deprecated(_vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 4 {
        return Err(SError::new("mark-deprecated")
            .with_code("wrong-argument-count")
            .with_message("Requires document, paths list, reason, and replacement link")
            .with_atom_field("received", args.len()));
    }
    let paths = parse_path_list(&args[1])?;
    let reason = extract_string(&args[2]);
    let link = extract_string(&args[3]);
    let reason_opt = if reason.is_empty() {
        None
    } else {
        Some(reason.as_str())
    };
    let link_opt = if link.is_empty() {
        None
    } else {
        Some(link.as_str())
    };
    mark_deprecated(&args[0], &paths, reason_opt, link_opt)
}

fn builtin_generate_toc(_vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 1 {
        return Err(SError::new("generate-toc")
            .with_code("wrong-argument-count")
            .with_message("Requires exactly one document argument")
            .with_atom_field("received", args.len()));
    }
    Ok(generate_toc(&args[0]))
}

// Link functions

fn builtin_scan_links(_vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 1 {
        return Err(SError::new("scan-links")
            .with_code("wrong-argument-count")
            .with_message("Requires exactly one document argument")
            .with_atom_field("received", args.len()));
    }
    Ok(scan_links_to_sexpr(&args[0]))
}

fn builtin_get_internal_links(_vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 1 {
        return Err(SError::new("get-internal-links")
            .with_code("wrong-argument-count")
            .with_message("Requires exactly one document argument")
            .with_atom_field("received", args.len()));
    }
    let links = get_internal_links(&args[0]);
    Ok(links_to_sexpr(&links))
}

fn builtin_get_external_links(_vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 1 {
        return Err(SError::new("get-external-links")
            .with_code("wrong-argument-count")
            .with_message("Requires exactly one document argument")
            .with_atom_field("received", args.len()));
    }
    let links = get_external_links(&args[0]);
    Ok(links_to_sexpr(&links))
}

fn builtin_get_image_links(_vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 1 {
        return Err(SError::new("get-image-links")
            .with_code("wrong-argument-count")
            .with_message("Requires exactly one document argument")
            .with_atom_field("received", args.len()));
    }
    let links = get_image_links(&args[0]);
    Ok(links_to_sexpr(&links))
}

fn builtin_scan_link_defs(_vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 1 {
        return Err(SError::new("scan-link-defs")
            .with_code("wrong-argument-count")
            .with_message("Requires exactly one document argument")
            .with_atom_field("received", args.len()));
    }
    let defs = scan_link_definitions(&args[0]);
    let list: Vec<SExpr> = defs
        .iter()
        .map(|d| {
            SExpr::List(vec![
                SExpr::Atom("link-def".to_string()),
                string_atom(&d.identifier),
                string_atom(&d.url),
                string_atom(&d.title),
                string_atom(&d.path.to_string()),
            ])
        })
        .collect();
    Ok(SExpr::List(list))
}

fn builtin_find_undef_refs(_vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 1 {
        return Err(SError::new("find-undef-refs")
            .with_code("wrong-argument-count")
            .with_message("Requires exactly one document argument")
            .with_atom_field("received", args.len()));
    }
    let refs = find_undefined_references(&args[0]);
    Ok(links_to_sexpr(&refs))
}

fn builtin_update_link(_vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 3 {
        return Err(SError::new("update-link")
            .with_code("wrong-argument-count")
            .with_message("Requires document, path, and new URL")
            .with_atom_field("received", args.len()));
    }
    let path_str = extract_string(&args[1]);
    let path = PathId::parse(&path_str)?;
    let new_url = extract_string(&args[2]);
    update_link(&args[0], &path, &new_url)
}

// Sectioning functions

fn builtin_extract_sections(_vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 1 {
        return Err(SError::new("extract-sections")
            .with_code("wrong-argument-count")
            .with_message("Requires exactly one document argument")
            .with_atom_field("received", args.len()));
    }
    Ok(extract_sections(&args[0]))
}

fn builtin_section_to_doc(_vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 1 {
        return Err(SError::new("section-to-doc")
            .with_code("wrong-argument-count")
            .with_message("Requires exactly one section argument")
            .with_atom_field("received", args.len()));
    }
    section_to_doc(&args[0])
}

fn builtin_slugify(_vm: &Vm, args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 1 {
        return Err(SError::new("slugify")
            .with_code("wrong-argument-count")
            .with_message("Requires exactly one string argument")
            .with_atom_field("received", args.len()));
    }
    let text = extract_string(&args[0]);
    Ok(string_atom(&slugify_text(&text)))
}

// Helper functions

/// Parses a list of path strings into PathId vector.
fn parse_path_list(expr: &SExpr) -> SResult<Vec<PathId>> {
    match expr {
        SExpr::List(items) => {
            let mut paths = Vec::new();
            for item in items {
                let path_str = extract_string(item);
                paths.push(PathId::parse(&path_str)?);
            }
            Ok(paths)
        }
        SExpr::Atom(s) => {
            // Single path as atom
            Ok(vec![PathId::parse(s)?])
        }
    }
}

/// Converts a vector of LinkInfo to an s-expression list.
fn links_to_sexpr(links: &[LinkInfo]) -> SExpr {
    SExpr::List(links.iter().map(link_info_to_sexpr).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    #[test]
    fn repl_creation() {
        let dir = env::current_dir().unwrap();
        let repl = Repl::new(&dir);
        assert!(repl.is_ok());
    }

    #[test]
    fn eval_markdown_to_sexpr() {
        let dir = env::current_dir().unwrap();
        let repl = Repl::new(&dir).unwrap();
        let input = "(markdown-to-sexpr \"# Hello\")";
        let result = repl.eval(input);
        assert!(result.is_ok());
        let expr = result.unwrap();
        assert!(expr.to_string().contains("(doc"));
        assert!(expr.to_string().contains("(h1"));
        println!("DEBUG result: {}", expr);
    }

    #[test]
    fn eval_generate_toc() {
        let dir = env::current_dir().unwrap();
        let repl = Repl::new(&dir).unwrap();
        let input = "(generate-toc (markdown-to-sexpr \"# One\\n\\n## Two\\n\\n### Three\"))";
        let result = repl.eval(input);
        assert!(result.is_ok());
        let expr = result.unwrap();
        assert!(expr.to_string().contains("ul"));
        println!("DEBUG toc: {}", expr);
    }

    #[test]
    fn eval_get_frontmatter() {
        let dir = env::current_dir().unwrap();
        let repl = Repl::new(&dir).unwrap();
        // Document without frontmatter
        let input = "(get-frontmatter (markdown-to-sexpr \"# Hello\"))";
        let result = repl.eval(input);
        assert!(result.is_ok());
        assert_eq!(result.unwrap().to_string(), "null");
    }

    #[test]
    fn eval_annotate() {
        let dir = env::current_dir().unwrap();
        let repl = Repl::new(&dir).unwrap();
        let input = "(annotate (markdown-to-sexpr \"# Hello\"))";
        let result = repl.eval(input);
        assert!(result.is_ok());
        let expr = result.unwrap();
        assert!(expr.to_string().contains("(@"));
        println!("DEBUG annotated: {}", expr);
    }

    #[test]
    fn eval_thread_last_pipeline() {
        let dir = env::current_dir().unwrap();
        let repl = Repl::new(&dir).unwrap();
        // Test piping markdown through annotation
        let input = "(->> \"# Title\" (markdown-to-sexpr) (annotate))";
        let result = repl.eval(input);
        assert!(result.is_ok());
        let expr = result.unwrap();
        assert!(expr.to_string().contains("(@"));
        println!("DEBUG pipeline: {}", expr);
    }

    #[test]
    fn edit_document_prune() {
        let dir = env::current_dir().unwrap();
        let repl = Repl::new(&dir).unwrap();

        // Create a temporary test file
        let test_file = "test_repl_edit_prune.md";
        fs::write(dir.join(test_file), "# Title\n\n## Section\n\nParagraph").unwrap();

        // Edit to prune the paragraph at path "3"
        let result = repl.edit_document(test_file, "(prune \"3\")");
        assert!(result.is_ok(), "Edit should succeed: {:?}", result);

        // Verify the paragraph was removed by reading the file back
        let edited_content = fs::read_to_string(dir.join(test_file)).unwrap();
        assert!(
            !edited_content.contains("Paragraph"),
            "Paragraph should be removed: {}",
            edited_content
        );
        println!("DEBUG: edited document: {}", edited_content);

        // Cleanup
        fs::remove_file(dir.join(test_file)).unwrap();
    }

    #[test]
    fn edit_document_not_found() {
        let dir = env::current_dir().unwrap();
        let repl = Repl::new(&dir).unwrap();

        let result = repl.edit_document("nonexistent_repl_test.md", "(prune \"1\")");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.to_string().contains("io-error"),
            "Error should mention io-error: {}",
            err
        );
        println!("DEBUG: expected error: {}", err);
    }

    #[test]
    fn edit_document_with_single_function() {
        let dir = env::current_dir().unwrap();
        let repl = Repl::new(&dir).unwrap();

        // Create a temporary test file
        let test_file = "test_repl_edit_single.md";
        fs::write(dir.join(test_file), "# Title\n\n## Section").unwrap();

        // Use generate-toc which produces valid markdown output
        let result = repl.edit_document(test_file, "generate-toc");
        println!("DEBUG: edit result: {:?}", result);
        assert!(
            result.is_ok(),
            "Edit with single function should succeed: {:?}",
            result
        );

        // Read back - generate-toc produces a ul list
        let edited_content = fs::read_to_string(dir.join(test_file)).unwrap();
        println!("DEBUG: edited document: {}", edited_content);

        // Cleanup
        fs::remove_file(dir.join(test_file)).unwrap();
    }

    #[test]
    fn read_file_from_disk() {
        let dir = env::current_dir().unwrap();
        let repl = Repl::new(&dir).unwrap();

        // Create a temporary test file
        let test_file = "test_repl_read.md";
        fs::write(dir.join(test_file), "# Hello World").unwrap();

        let result = repl.read_file(test_file);
        assert!(result.is_ok());
        let expr = result.unwrap();
        assert!(expr.to_string().contains("h1"));
        println!("DEBUG: read file: {}", expr);

        // Cleanup
        fs::remove_file(dir.join(test_file)).unwrap();
    }
}

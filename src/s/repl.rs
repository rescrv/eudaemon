//! REPL for evaluating s-expressions against markdown documents in a directory.
//!
//! Provides an interactive environment for:
//! - Loading and saving markdown files
//! - Parsing markdown to s-expressions
//! - Evaluating s-expressions with registered markdown functions
//! - Converting s-expressions back to markdown

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use rustyline::config::Configurer;
use rustyline::error::ReadlineError;
use rustyline::{DefaultEditor, EditMode};

use super::error::{SError, SResult};
use super::eval::{Env, register_builtins};
use super::expr::{Parser, SExpr};
use super::markdown::curation::{
    LinkInfo, find_undefined_references, generate_toc, get_external_links, get_image_links,
    get_internal_links, link_info_to_sexpr, mark_deprecated, normalize_headers,
    scan_link_definitions, scan_links_to_sexpr, update_link, wrap_in_callout, wrap_in_details,
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
use super::util::{extract_string, string_atom};

/// REPL state containing the working directory and loaded documents.
pub struct Repl {
    /// Current working directory.
    working_dir: PathBuf,
    /// Loaded markdown documents keyed by filename.
    documents: HashMap<String, SExpr>,
    /// Evaluation environment with registered functions.
    env: Env,
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

        let mut env = Env::new();
        register_builtins(&mut env);
        register_markdown_builtins(&mut env);

        Ok(Repl {
            working_dir,
            documents: HashMap::new(),
            env,
        })
    }

    /// Returns the current working directory.
    pub fn working_dir(&self) -> &Path {
        &self.working_dir
    }

    /// Lists all markdown files in the working directory.
    pub fn list_files(&self) -> SResult<Vec<String>> {
        let mut files = Vec::new();
        let entries = fs::read_dir(&self.working_dir).map_err(|e| {
            SError::new("repl")
                .with_code("io-error")
                .with_message("Failed to read directory")
                .with_string_field("error", &e.to_string())
        })?;

        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file()
                && path
                    .extension()
                    .is_some_and(|ext| ext == "md" || ext == "markdown")
                && let Some(name) = path.file_name()
            {
                files.push(name.to_string_lossy().to_string());
            }
        }
        files.sort();
        Ok(files)
    }

    /// Loads a markdown file into the REPL.
    pub fn load(&mut self, filename: &str) -> SResult<&SExpr> {
        let path = self.working_dir.join(filename);
        let content = fs::read_to_string(&path).map_err(|e| {
            SError::new("repl")
                .with_code("io-error")
                .with_message("Failed to read file")
                .with_string_field("path", &path.display().to_string())
                .with_string_field("error", &e.to_string())
        })?;

        let sexpr = markdown_to_sexpr(&content)?;
        self.documents.insert(filename.to_string(), sexpr);
        Ok(self.documents.get(filename).unwrap())
    }

    /// Saves a document back to file.
    pub fn save(&self, filename: &str) -> SResult<()> {
        let doc = self.documents.get(filename).ok_or_else(|| {
            SError::new("repl")
                .with_code("not-loaded")
                .with_message("Document not loaded")
                .with_string_field("filename", filename)
        })?;

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

    /// Gets a loaded document.
    pub fn get(&self, filename: &str) -> Option<&SExpr> {
        self.documents.get(filename)
    }

    /// Sets a document in the REPL state.
    pub fn set(&mut self, filename: &str, doc: SExpr) {
        self.documents.insert(filename.to_string(), doc);
    }

    /// Returns list of loaded document names.
    pub fn loaded(&self) -> Vec<&str> {
        self.documents.keys().map(|s| s.as_str()).collect()
    }

    /// Evaluates an s-expression string in the REPL environment.
    ///
    /// Loaded documents are available as variables using their filename.
    /// For example, after `:load foo.md`, you can reference it as `foo.md` in expressions.
    pub fn eval(&self, input: &str) -> SResult<SExpr> {
        let mut parser = Parser::new(input);
        let expr = parser.parse()?;

        // Create a child environment with loaded documents as bindings
        let mut eval_env = self.env.clone();
        for (name, doc) in &self.documents {
            eval_env.bind(name, doc.clone());
        }

        super::eval::eval(&expr, &eval_env)
    }

    /// Runs the interactive REPL loop using rustyline with vim keybindings.
    pub fn run_interactive(&mut self) -> SResult<()> {
        let mut rl = DefaultEditor::new().map_err(|e| {
            SError::new("repl")
                .with_code("readline-error")
                .with_message("Failed to initialize readline")
                .with_string_field("error", &e.to_string())
        })?;

        // Configure vim keybindings
        rl.set_edit_mode(EditMode::Vi);

        // Load history from file if it exists
        let history_path = self.working_dir.join(".agentkb_history");
        let _ = rl.load_history(&history_path);

        println!("agentkb REPL (vim mode)");
        println!("Working directory: {}", self.working_dir.display());
        println!("Type :help for commands, :quit to exit\n");

        loop {
            match rl.readline("agentkb> ") {
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

                    // Evaluate as s-expression
                    match self.eval(input) {
                        Ok(result) => println!("{}", result),
                        Err(e) => println!("Error: {}", e),
                    }
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

    /// Handles a REPL command (lines starting with ':').
    /// Returns Ok(true) to continue, Ok(false) to quit.
    fn handle_command(&mut self, input: &str) -> SResult<bool> {
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
                        let loaded = if self.documents.contains_key(&f) {
                            "*"
                        } else {
                            " "
                        };
                        println!("{} {}", loaded, f);
                    }
                }
            }

            ":load" | ":l" => {
                if parts.len() < 2 {
                    return Err(SError::new("repl")
                        .with_code("missing-argument")
                        .with_message("Usage: :load <filename>"));
                }
                let doc = self.load(parts[1])?;
                println!("Loaded: {}", parts[1]);
                println!("{}", doc);
            }

            ":save" | ":s" => {
                if parts.len() < 2 {
                    return Err(SError::new("repl")
                        .with_code("missing-argument")
                        .with_message("Usage: :save <filename>"));
                }
                self.save(parts[1])?;
                println!("Saved: {}", parts[1]);
            }

            ":show" => {
                if parts.len() < 2 {
                    return Err(SError::new("repl")
                        .with_code("missing-argument")
                        .with_message("Usage: :show <filename>"));
                }
                if let Some(doc) = self.get(parts[1]) {
                    println!("{}", doc);
                } else {
                    println!("Document not loaded: {}", parts[1]);
                }
            }

            ":md" => {
                if parts.len() < 2 {
                    return Err(SError::new("repl")
                        .with_code("missing-argument")
                        .with_message("Usage: :md <filename>"));
                }
                if let Some(doc) = self.get(parts[1]) {
                    let md = sexpr_to_markdown(doc)?;
                    println!("{}", md);
                } else {
                    println!("Document not loaded: {}", parts[1]);
                }
            }

            ":annotate" | ":ann" => {
                if parts.len() < 2 {
                    return Err(SError::new("repl")
                        .with_code("missing-argument")
                        .with_message("Usage: :annotate <filename>"));
                }
                if let Some(doc) = self.get(parts[1]) {
                    let annotated = to_annotated_sexpr(doc);
                    println!("{}", annotated);
                } else {
                    println!("Document not loaded: {}", parts[1]);
                }
            }

            ":loaded" => {
                let loaded = self.loaded();
                if loaded.is_empty() {
                    println!("No documents loaded");
                } else {
                    for name in loaded {
                        println!("  {}", name);
                    }
                }
            }

            ":pwd" => {
                println!("{}", self.working_dir.display());
            }

            ":fns" | ":functions" => {
                print_functions();
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
  :load <file>      Load a markdown file
  :save <file>      Save a loaded document
  :show <file>      Show document as s-expression
  :md <file>        Show document as markdown
  :annotate <file>  Show document with path/content IDs
  :loaded           List loaded documents
  :pwd              Print working directory
  :fns              List available functions

S-expression evaluation:
  Type any s-expression to evaluate it.
  Loaded documents are available as variables by filename.
  
Examples:
  :load docs/readme.md
  (get-by-path docs/readme.md \"1\")
  (generate-toc docs/readme.md)
  (->> docs/readme.md (generate-toc))"
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

/// Registers all markdown-related functions in the environment.
pub fn register_markdown_builtins(env: &mut Env) {
    // Conversion
    env.def_fn("markdown-to-sexpr", builtin_markdown_to_sexpr);
    env.def_fn("sexpr-to-markdown", builtin_sexpr_to_markdown);

    // Frontmatter
    env.def_fn("get-frontmatter", builtin_get_frontmatter);
    env.def_fn("get-frontmatter-content", builtin_get_frontmatter_content);
    env.def_fn("set-frontmatter", builtin_set_frontmatter);
    env.def_fn("remove-frontmatter", builtin_remove_frontmatter);
    env.def_fn("parse-yaml-frontmatter", builtin_parse_yaml_frontmatter);
    env.def_fn("get-fm-field", builtin_get_fm_field);
    env.def_fn("upsert-fm-field", builtin_upsert_fm_field);
    env.def_fn("remove-fm-field", builtin_remove_fm_field);

    // Node navigation
    env.def_fn("get-by-path", builtin_get_by_path);
    env.def_fn("get-node", builtin_get_node);
    env.def_fn("get-parent", builtin_get_parent);
    env.def_fn("get-siblings", builtin_get_siblings);
    env.def_fn("get-context", builtin_get_context);
    env.def_fn("annotate", builtin_annotate);

    // Mutations
    env.def_fn("replace-at", builtin_replace_at);
    env.def_fn("prune", builtin_prune);
    env.def_fn("insert-before", builtin_insert_before);
    env.def_fn("insert-after", builtin_insert_after);
    env.def_fn("append-child", builtin_append_child);
    env.def_fn("prepend-child", builtin_prepend_child);
    env.def_fn("hoist", builtin_hoist);
    env.def_fn("graft", builtin_graft);

    // Curation
    env.def_fn("wrap-in-callout", builtin_wrap_in_callout);
    env.def_fn("wrap-in-details", builtin_wrap_in_details);
    env.def_fn("normalize-headers", builtin_normalize_headers);
    env.def_fn("mark-deprecated", builtin_mark_deprecated);
    env.def_fn("generate-toc", builtin_generate_toc);

    // Links
    env.def_fn("scan-links", builtin_scan_links);
    env.def_fn("get-internal-links", builtin_get_internal_links);
    env.def_fn("get-external-links", builtin_get_external_links);
    env.def_fn("get-image-links", builtin_get_image_links);
    env.def_fn("scan-link-defs", builtin_scan_link_defs);
    env.def_fn("find-undef-refs", builtin_find_undef_refs);
    env.def_fn("update-link", builtin_update_link);
}

// Conversion functions

fn builtin_markdown_to_sexpr(args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 1 {
        return Err(SError::new("markdown-to-sexpr")
            .with_code("wrong-argument-count")
            .with_message("Requires exactly one string argument")
            .with_atom_field("received", args.len()));
    }
    let md = extract_string(&args[0]);
    markdown_to_sexpr(&md)
}

fn builtin_sexpr_to_markdown(args: &[SExpr]) -> SResult<SExpr> {
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

fn builtin_get_frontmatter(args: &[SExpr]) -> SResult<SExpr> {
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

fn builtin_get_frontmatter_content(args: &[SExpr]) -> SResult<SExpr> {
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

fn builtin_set_frontmatter(args: &[SExpr]) -> SResult<SExpr> {
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

fn builtin_remove_frontmatter(args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 1 {
        return Err(SError::new("remove-frontmatter")
            .with_code("wrong-argument-count")
            .with_message("Requires exactly one document argument")
            .with_atom_field("received", args.len()));
    }
    Ok(remove_frontmatter(&args[0]))
}

fn builtin_parse_yaml_frontmatter(args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 1 {
        return Err(SError::new("parse-yaml-frontmatter")
            .with_code("wrong-argument-count")
            .with_message("Requires exactly one string argument")
            .with_atom_field("received", args.len()));
    }
    let content = extract_string(&args[0]);
    Ok(parse_yaml_frontmatter(&content))
}

fn builtin_get_fm_field(args: &[SExpr]) -> SResult<SExpr> {
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

fn builtin_upsert_fm_field(args: &[SExpr]) -> SResult<SExpr> {
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

fn builtin_remove_fm_field(args: &[SExpr]) -> SResult<SExpr> {
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

fn builtin_get_by_path(args: &[SExpr]) -> SResult<SExpr> {
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

fn builtin_get_node(args: &[SExpr]) -> SResult<SExpr> {
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

fn builtin_get_parent(args: &[SExpr]) -> SResult<SExpr> {
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

fn builtin_get_siblings(args: &[SExpr]) -> SResult<SExpr> {
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

fn builtin_get_context(args: &[SExpr]) -> SResult<SExpr> {
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

fn builtin_annotate(args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 1 {
        return Err(SError::new("annotate")
            .with_code("wrong-argument-count")
            .with_message("Requires exactly one document argument")
            .with_atom_field("received", args.len()));
    }
    Ok(to_annotated_sexpr(&args[0]))
}

// Mutation functions

fn builtin_replace_at(args: &[SExpr]) -> SResult<SExpr> {
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

fn builtin_prune(args: &[SExpr]) -> SResult<SExpr> {
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

fn builtin_insert_before(args: &[SExpr]) -> SResult<SExpr> {
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

fn builtin_insert_after(args: &[SExpr]) -> SResult<SExpr> {
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

fn builtin_append_child(args: &[SExpr]) -> SResult<SExpr> {
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

fn builtin_prepend_child(args: &[SExpr]) -> SResult<SExpr> {
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

fn builtin_hoist(args: &[SExpr]) -> SResult<SExpr> {
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

fn builtin_graft(args: &[SExpr]) -> SResult<SExpr> {
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

fn builtin_wrap_in_callout(args: &[SExpr]) -> SResult<SExpr> {
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

fn builtin_wrap_in_details(args: &[SExpr]) -> SResult<SExpr> {
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

fn builtin_normalize_headers(args: &[SExpr]) -> SResult<SExpr> {
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

fn builtin_mark_deprecated(args: &[SExpr]) -> SResult<SExpr> {
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

fn builtin_generate_toc(args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 1 {
        return Err(SError::new("generate-toc")
            .with_code("wrong-argument-count")
            .with_message("Requires exactly one document argument")
            .with_atom_field("received", args.len()));
    }
    Ok(generate_toc(&args[0]))
}

// Link functions

fn builtin_scan_links(args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 1 {
        return Err(SError::new("scan-links")
            .with_code("wrong-argument-count")
            .with_message("Requires exactly one document argument")
            .with_atom_field("received", args.len()));
    }
    Ok(scan_links_to_sexpr(&args[0]))
}

fn builtin_get_internal_links(args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 1 {
        return Err(SError::new("get-internal-links")
            .with_code("wrong-argument-count")
            .with_message("Requires exactly one document argument")
            .with_atom_field("received", args.len()));
    }
    let links = get_internal_links(&args[0]);
    Ok(links_to_sexpr(&links))
}

fn builtin_get_external_links(args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 1 {
        return Err(SError::new("get-external-links")
            .with_code("wrong-argument-count")
            .with_message("Requires exactly one document argument")
            .with_atom_field("received", args.len()));
    }
    let links = get_external_links(&args[0]);
    Ok(links_to_sexpr(&links))
}

fn builtin_get_image_links(args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 1 {
        return Err(SError::new("get-image-links")
            .with_code("wrong-argument-count")
            .with_message("Requires exactly one document argument")
            .with_atom_field("received", args.len()));
    }
    let links = get_image_links(&args[0]);
    Ok(links_to_sexpr(&links))
}

fn builtin_scan_link_defs(args: &[SExpr]) -> SResult<SExpr> {
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

fn builtin_find_undef_refs(args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 1 {
        return Err(SError::new("find-undef-refs")
            .with_code("wrong-argument-count")
            .with_message("Requires exactly one document argument")
            .with_atom_field("received", args.len()));
    }
    let refs = find_undefined_references(&args[0]);
    Ok(links_to_sexpr(&refs))
}

fn builtin_update_link(args: &[SExpr]) -> SResult<SExpr> {
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
}

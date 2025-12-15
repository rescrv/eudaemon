//! Refactor a markdown document into a directory hierarchy based on header structure.
//!
//! This example demonstrates lispdown by running a Lisp program that:
//! 1. Loads lincoln.md
//! 2. Extracts sections based on header hierarchy  
//! 3. Writes each section to its own file in a directory tree
//!
//! Run with: cargo run --example lincoln

use std::fs;
use std::path::PathBuf;

use lispdown::{register_markdown_builtins, DirectoryFilesystem, Parser, Vm, VmState};

/// The Lisp program that does the actual work.
///
/// Uses these builtins:
/// - load: read markdown file and parse to s-expression
/// - extract-sections: parse document into section hierarchy  
/// - section-to-doc: convert a section back to a document s-expression
/// - save: convert s-expression to markdown and write to file
/// - get: access fields in section objects
/// - str: concatenate strings
///
/// This program is akin to the standard library call:
/// (splat (load "lincoln.md") "lincoln/")
const LISP_PROGRAM: &str = r#"
(begin
  (defun get-field (section field)
    (get field section))

  (defun get-children (section)
    (let ((children (get "children" section)))
      (if (list? children)
          (rest children)
          (list))))

  (defun write-section (section path)
    (save (section-to-doc section) path))

  (defun make-toc-entry (child has-children)
    (let ((title (get-field child "title"))
          (slug (get-field child "slug")))
      (if has-children
          (list (quote li) (list (quote link) (str slug "/index.md") "" title))
          (list (quote li) (list (quote link) (str slug ".md") "" title)))))

  (defun build-toc-items (children)
    (if (empty? children)
        (list)
        (let ((child (first children))
              (rest-children (rest children)))
          (let ((grandchildren (get-children child))
                (has-children (if (empty? grandchildren) #f #t)))
            (cons (make-toc-entry child has-children)
                  (build-toc-items rest-children))))))

  (defun make-index-doc (section)
    (let ((title (get-field section "title"))
          (children (get-children section)))
      (let ((toc-items (build-toc-items children)))
        (let ((header (list (quote h1) title))
              (ul-list (cons (quote ul) toc-items)))
          (list (quote doc) header ul-list)))))

  (defun write-index (section path)
    (save (make-index-doc section) path))

  (defun process-leaf (section prefix)
    (let ((slug (get-field section "slug")))
      (write-section section (str prefix slug ".md"))))

  (defun process-each-leaf (items prefix)
    (if (empty? items)
        "done"
        (begin
          (process-leaf (first items) prefix)
          (process-each-leaf (rest items) prefix))))

  (defun process-h3 (section parent-prefix)
    (let ((slug (get-field section "slug")))
      (let ((children (get-children section)))
        (let ((prefix (str parent-prefix slug "/")))
          (if (empty? children)
              (write-section section (str parent-prefix slug ".md"))
              (begin
                (write-index section (str parent-prefix slug "/index.md"))
                (process-each-leaf children prefix)))))))

  (defun process-each-h3 (items parent-prefix)
    (if (empty? items)
        "done"
        (begin
          (process-h3 (first items) parent-prefix)
          (process-each-h3 (rest items) parent-prefix))))

  (defun process-h2 (section parent-prefix)
    (let ((slug (get-field section "slug")))
      (let ((children (get-children section)))
        (let ((prefix (str parent-prefix slug "/")))
          (if (empty? children)
              (write-section section (str parent-prefix slug ".md"))
              (begin
                (write-index section (str parent-prefix slug "/index.md"))
                (process-each-h3 children prefix)))))))

  (defun process-each-h2 (items parent-prefix)
    (if (empty? items)
        "done"
        (begin
          (process-h2 (first items) parent-prefix)
          (process-each-h2 (rest items) parent-prefix))))

  (defun process-h1 (section)
    (let ((slug (get-field section "slug")))
      (let ((children (get-children section)))
        (let ((prefix (str slug "/")))
          (if (empty? children)
              (write-section section (str slug ".md"))
              (begin
                (write-index section (str slug "/index.md"))
                (process-each-h2 children prefix)))))))

  (let ((doc (load "lincoln.md")))
    (let ((sections (extract-sections doc)))
      (let ((top-sections (rest sections)))
        (if (empty? top-sections)
            "No sections found"
            (begin
              (process-h1 (first top-sections))
              "Refactoring complete!"))))))
"#;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let input_path = args
        .get(1)
        .map(String::as_str)
        .unwrap_or("examples/lincoln.md");

    // Determine output directory from input path
    let output_dir = {
        let path = PathBuf::from(input_path);
        let stem = path.file_stem().unwrap_or_default().to_string_lossy();
        path.parent()
            .unwrap_or(std::path::Path::new("."))
            .join(stem.as_ref())
    };

    // Create output directory
    fs::create_dir_all(&output_dir).expect("Failed to create output directory");
    eprintln!("Output directory: {}", output_dir.display());

    // Copy input file to output directory so the VM can load it
    let input_content = fs::read_to_string(input_path).expect("Failed to read input file");
    fs::write(output_dir.join("lincoln.md"), &input_content).expect("Failed to copy input file");

    // Create VM with all builtins
    let mut vm = Vm::new();
    vm.register_builtins();
    vm.register_json_builtins();
    vm.register_filesystem_builtins();
    register_markdown_builtins(&mut vm);

    // Attach filesystem rooted at output directory
    let filesystem =
        DirectoryFilesystem::new(&output_dir).expect("Failed to create directory filesystem");
    vm.set_filesystem(Box::new(filesystem));

    // Parse and run the Lisp program
    let program = Parser::new(LISP_PROGRAM)
        .parse()
        .expect("Failed to parse Lisp program");

    vm.load(program);

    match vm.run() {
        VmState::Finished(result) => {
            eprintln!("Result: {}", result);
        }
        VmState::Suspended(condition) => {
            eprintln!("Error: {:?}", condition);
            std::process::exit(1);
        }
        VmState::Running => unreachable!(),
    }

    // Clean up the copied input file
    let _ = fs::remove_file(output_dir.join("lincoln.md"));

    eprintln!("\nFiles written to: {}", output_dir.display());
}

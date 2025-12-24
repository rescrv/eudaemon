//! Summarize markdown headings across a knowledge base.
//!
//! Usage: summarize-knowledge-base <dir> [dir...]

use std::fs;
use std::path::{Path, PathBuf};

use lispdown::{SError, SExpr, markdown_to_sexpr, unescape_string};

struct Heading {
    level: usize,
    text: String,
}

fn heading_level(tag: &str) -> Option<usize> {
    match tag {
        "h1" => Some(1),
        "h2" => Some(2),
        "h3" => Some(3),
        "h4" => Some(4),
        "h5" => Some(5),
        "h6" => Some(6),
        _ => None,
    }
}

fn extract_text(expr: &SExpr) -> String {
    match expr {
        SExpr::Atom(s) => {
            if s.starts_with('"') && s.ends_with('"') && s.len() >= 2 {
                unescape_string(&s[1..s.len() - 1])
            } else {
                s.clone()
            }
        }
        SExpr::List(items) => {
            let mut text = String::new();
            for item in items.iter().skip(1) {
                text.push_str(&extract_text(item));
            }
            text
        }
    }
}

fn collect_headings(expr: &SExpr, headings: &mut Vec<Heading>) {
    if let SExpr::List(items) = expr {
        if let Some(SExpr::Atom(tag)) = items.first()
            && let Some(level) = heading_level(tag)
        {
            let text = extract_text(expr).trim().to_string();
            headings.push(Heading { level, text });
        }

        for child in items.iter().skip(1) {
            collect_headings(child, headings);
        }
    }
}

fn find_markdown_files(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    find_markdown_files_recursive(dir, &mut files);
    files.sort();
    files
}

fn find_markdown_files_recursive(dir: &Path, files: &mut Vec<PathBuf>) {
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                find_markdown_files_recursive(&path, files);
            } else if path.extension().is_some_and(|ext| ext == "md") {
                files.push(path);
            }
        }
    }
}

fn display_path(path: &Path, cwd: &Path) -> String {
    if let Ok(relative) = path.strip_prefix(cwd) {
        relative.to_string_lossy().to_string()
    } else {
        path.to_string_lossy().to_string()
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!("Usage: summarize-knowledge-base <dir> [dir...]");
        std::process::exit(1);
    }

    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let mut files = Vec::new();

    for arg in &args {
        let path = PathBuf::from(arg);
        if !path.exists() {
            let error = SError::new("summarize-knowledge-base")
                .with_code("path-not-found")
                .with_message("Path does not exist")
                .with_string_field("path", arg);
            eprintln!("{}", error.detail());
            std::process::exit(1);
        }

        if path.is_dir() {
            for file in find_markdown_files(&path) {
                let display = display_path(&file, &cwd);
                files.push((display, file));
            }
        } else if path.extension().is_some_and(|ext| ext == "md") {
            let display = display_path(&path, &cwd);
            files.push((display, path));
        } else {
            let error = SError::new("summarize-knowledge-base")
                .with_code("not-markdown")
                .with_message("Path is not a markdown file or directory")
                .with_string_field("path", arg);
            eprintln!("{}", error.detail());
            std::process::exit(1);
        }
    }

    files.sort_by(|a, b| a.0.cmp(&b.0));

    for (index, (display, path)) in files.iter().enumerate() {
        if index > 0 {
            println!();
        }
        println!("# {}", display);

        let content = match fs::read_to_string(path) {
            Ok(content) => content,
            Err(e) => {
                let error = SError::new("summarize-knowledge-base")
                    .with_code("io-error")
                    .with_message("Error reading file")
                    .with_string_field("path", &path.to_string_lossy())
                    .with_string_field("io_error", &e.to_string());
                eprintln!("{}", error.detail());
                std::process::exit(1);
            }
        };

        let doc = match markdown_to_sexpr(&content) {
            Ok(doc) => doc,
            Err(e) => {
                eprintln!("{}", e.detail());
                std::process::exit(1);
            }
        };

        let mut headings = Vec::new();
        collect_headings(&doc, &mut headings);
        for heading in headings {
            let indent = "\t".repeat(heading.level.saturating_sub(1));
            println!("{}- {}", indent, heading.text);
        }
    }
}

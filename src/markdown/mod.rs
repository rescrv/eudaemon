//! Markdown to S-expression conversion and vice versa.
//!
//! This module provides bidirectional conversion between markdown documents
//! and S-expressions, enabling programmatic manipulation of markdown content.
//!
//! # S-expression Representation
//!
//! ```text
//! Document:       (doc <block>...)
//! Paragraph:      (p <inline>...)
//! Heading:        (h1 <inline>...) through (h6 <inline>...)
//! List:           (ul <li>...) or (ol <li>...) or (ol start <li>...)
//! ListItem:       (li <block>...)
//! CodeBlock:      (code-block "language" "content")
//! Blockquote:     (blockquote <block>...)
//! ThematicBreak:  (hr)
//! Definition:     (def "label" "url" "title")
//!
//! Inline elements:
//! Text:           "literal text"
//! InlineCode:     (code "text")
//! Emphasis:       (em <inline>...)
//! Strong:         (strong <inline>...)
//! Link:           (link "url" "title" <inline>...)
//! Image:          (img "url" "alt" "title")
//! Break:          (br)
//! Html:           (html "raw html content")
//!
//! Frontmatter:
//! Yaml:           (yaml "content")
//! Toml:           (toml "content")
//! ```

pub mod curation;
pub mod invariants;
pub mod mutations;
pub mod test_runner;

use markdown::ParseOptions;
use markdown::mdast::Node;

use super::error::{SError, SResult};
use super::expr::SExpr;
use super::json::unescape_string;
use super::util::{extract_string, string_atom};

/// Parses markdown text into an S-expression AST.
pub fn markdown_to_sexpr(md: &str) -> SResult<SExpr> {
    let ast = markdown::to_mdast(md, &ParseOptions::default()).map_err(|e| {
        SError::new("markdown-parse")
            .with_code("parse-error")
            .with_message("Failed to parse markdown")
            .with_string_field("error", &e.to_string())
    })?;
    node_to_sexpr(&ast)
}

/// Converts an mdast Node to an S-expression.
fn node_to_sexpr(node: &Node) -> SResult<SExpr> {
    match node {
        Node::Root(root) => {
            let mut elements = vec![SExpr::Atom("doc".to_string())];
            for child in &root.children {
                elements.push(node_to_sexpr(child)?);
            }
            Ok(SExpr::List(elements))
        }

        Node::Paragraph(para) => {
            let mut elements = vec![SExpr::Atom("p".to_string())];
            for child in &para.children {
                elements.push(node_to_sexpr(child)?);
            }
            Ok(SExpr::List(elements))
        }

        Node::Heading(heading) => {
            let tag = format!("h{}", heading.depth);
            let mut elements = vec![SExpr::Atom(tag)];
            for child in &heading.children {
                elements.push(node_to_sexpr(child)?);
            }
            Ok(SExpr::List(elements))
        }

        Node::List(list) => {
            let tag = if list.ordered { "ol" } else { "ul" };
            let mut elements = vec![SExpr::Atom(tag.to_string())];
            // Include start number for ordered lists if not 1
            if list.ordered
                && let Some(start) = list.start
                && start != 1
            {
                elements.push(SExpr::Atom(start.to_string()));
            }
            for child in &list.children {
                elements.push(node_to_sexpr(child)?);
            }
            Ok(SExpr::List(elements))
        }

        Node::ListItem(item) => {
            let mut elements = vec![SExpr::Atom("li".to_string())];
            for child in &item.children {
                elements.push(node_to_sexpr(child)?);
            }
            Ok(SExpr::List(elements))
        }

        Node::Code(code) => {
            let lang = code.lang.as_deref().unwrap_or("");
            Ok(SExpr::List(vec![
                SExpr::Atom("code-block".to_string()),
                string_atom(lang),
                string_atom(&code.value),
            ]))
        }

        Node::Blockquote(bq) => {
            let mut elements = vec![SExpr::Atom("blockquote".to_string())];
            for child in &bq.children {
                elements.push(node_to_sexpr(child)?);
            }
            Ok(SExpr::List(elements))
        }

        Node::ThematicBreak(_) => Ok(SExpr::List(vec![SExpr::Atom("hr".to_string())])),

        Node::Text(text) => Ok(string_atom(&text.value)),

        Node::InlineCode(code) => Ok(SExpr::List(vec![
            SExpr::Atom("code".to_string()),
            string_atom(&code.value),
        ])),

        Node::Emphasis(em) => {
            let mut elements = vec![SExpr::Atom("em".to_string())];
            for child in &em.children {
                elements.push(node_to_sexpr(child)?);
            }
            Ok(SExpr::List(elements))
        }

        Node::Strong(strong) => {
            let mut elements = vec![SExpr::Atom("strong".to_string())];
            for child in &strong.children {
                elements.push(node_to_sexpr(child)?);
            }
            Ok(SExpr::List(elements))
        }

        Node::Link(link) => {
            let mut elements = vec![
                SExpr::Atom("link".to_string()),
                string_atom(&link.url),
                string_atom(link.title.as_deref().unwrap_or("")),
            ];
            for child in &link.children {
                elements.push(node_to_sexpr(child)?);
            }
            Ok(SExpr::List(elements))
        }

        Node::Image(img) => Ok(SExpr::List(vec![
            SExpr::Atom("img".to_string()),
            string_atom(&img.url),
            string_atom(&img.alt),
            string_atom(img.title.as_deref().unwrap_or("")),
        ])),

        Node::Break(_) => Ok(SExpr::List(vec![SExpr::Atom("br".to_string())])),

        Node::Html(html) => Ok(SExpr::List(vec![
            SExpr::Atom("html".to_string()),
            string_atom(&html.value),
        ])),

        Node::Definition(def) => Ok(SExpr::List(vec![
            SExpr::Atom("def".to_string()),
            string_atom(&def.identifier),
            string_atom(&def.url),
            string_atom(def.title.as_deref().unwrap_or("")),
        ])),

        Node::LinkReference(lr) => {
            let mut elements = vec![
                SExpr::Atom("link-ref".to_string()),
                string_atom(&lr.identifier),
            ];
            for child in &lr.children {
                elements.push(node_to_sexpr(child)?);
            }
            Ok(SExpr::List(elements))
        }

        Node::ImageReference(ir) => Ok(SExpr::List(vec![
            SExpr::Atom("img-ref".to_string()),
            string_atom(&ir.identifier),
            string_atom(&ir.alt),
        ])),

        Node::Yaml(yaml) => Ok(SExpr::List(vec![
            SExpr::Atom("yaml".to_string()),
            string_atom(&yaml.value),
        ])),

        Node::Toml(toml) => Ok(SExpr::List(vec![
            SExpr::Atom("toml".to_string()),
            string_atom(&toml.value),
        ])),

        // Table support (basic)
        Node::Table(table) => {
            let mut elements = vec![SExpr::Atom("table".to_string())];
            for child in &table.children {
                elements.push(node_to_sexpr(child)?);
            }
            Ok(SExpr::List(elements))
        }

        Node::TableRow(row) => {
            let mut elements = vec![SExpr::Atom("tr".to_string())];
            for child in &row.children {
                elements.push(node_to_sexpr(child)?);
            }
            Ok(SExpr::List(elements))
        }

        Node::TableCell(cell) => {
            let mut elements = vec![SExpr::Atom("td".to_string())];
            for child in &cell.children {
                elements.push(node_to_sexpr(child)?);
            }
            Ok(SExpr::List(elements))
        }

        // Footnotes
        Node::FootnoteDefinition(fd) => {
            let mut elements = vec![
                SExpr::Atom("footnote-def".to_string()),
                string_atom(&fd.identifier),
            ];
            for child in &fd.children {
                elements.push(node_to_sexpr(child)?);
            }
            Ok(SExpr::List(elements))
        }

        Node::FootnoteReference(fr) => Ok(SExpr::List(vec![
            SExpr::Atom("footnote-ref".to_string()),
            string_atom(&fr.identifier),
        ])),

        // Delete (strikethrough)
        Node::Delete(del) => {
            let mut elements = vec![SExpr::Atom("del".to_string())];
            for child in &del.children {
                elements.push(node_to_sexpr(child)?);
            }
            Ok(SExpr::List(elements))
        }

        // Math
        Node::Math(math) => Ok(SExpr::List(vec![
            SExpr::Atom("math-block".to_string()),
            string_atom(&math.value),
        ])),

        Node::InlineMath(math) => Ok(SExpr::List(vec![
            SExpr::Atom("math".to_string()),
            string_atom(&math.value),
        ])),

        // MDX elements - convert to generic representation
        Node::MdxjsEsm(esm) => Ok(SExpr::List(vec![
            SExpr::Atom("mdx-esm".to_string()),
            string_atom(&esm.value),
        ])),

        Node::MdxFlowExpression(expr) => Ok(SExpr::List(vec![
            SExpr::Atom("mdx-expr".to_string()),
            string_atom(&expr.value),
        ])),

        Node::MdxTextExpression(expr) => Ok(SExpr::List(vec![
            SExpr::Atom("mdx-text-expr".to_string()),
            string_atom(&expr.value),
        ])),

        Node::MdxJsxFlowElement(el) => {
            let mut elements = vec![
                SExpr::Atom("mdx-jsx".to_string()),
                string_atom(el.name.as_deref().unwrap_or("")),
            ];
            for child in &el.children {
                elements.push(node_to_sexpr(child)?);
            }
            Ok(SExpr::List(elements))
        }

        Node::MdxJsxTextElement(el) => {
            let mut elements = vec![
                SExpr::Atom("mdx-jsx-inline".to_string()),
                string_atom(el.name.as_deref().unwrap_or("")),
            ];
            for child in &el.children {
                elements.push(node_to_sexpr(child)?);
            }
            Ok(SExpr::List(elements))
        }
    }
}

/// Converts an S-expression AST back to markdown text.
pub fn sexpr_to_markdown(expr: &SExpr) -> SResult<String> {
    let mut output = String::new();
    sexpr_to_markdown_impl(expr, &mut output, 0)?;
    // Trim trailing whitespace but preserve a single trailing newline
    let trimmed = output.trim_end();
    if trimmed.is_empty() {
        Ok(String::new())
    } else {
        Ok(format!("{}\n", trimmed))
    }
}

/// Internal implementation for markdown generation with context tracking.
fn sexpr_to_markdown_impl(expr: &SExpr, output: &mut String, depth: usize) -> SResult<()> {
    match expr {
        SExpr::Atom(s) => {
            // String atom - extract content
            if s.starts_with('"') && s.ends_with('"') && s.len() >= 2 {
                output.push_str(&unescape_string(&s[1..s.len() - 1]));
            } else {
                output.push_str(s);
            }
            Ok(())
        }
        SExpr::List(items) => {
            if items.is_empty() {
                return Ok(());
            }

            let tag = match &items[0] {
                SExpr::Atom(s) => s.as_str(),
                _ => {
                    return Err(SError::new("sexpr-to-markdown")
                        .with_code("invalid-tag")
                        .with_message("First element of list must be a tag atom")
                        .with_field("element", items[0].clone()));
                }
            };

            match tag {
                "doc" => {
                    for child in items.iter().skip(1) {
                        sexpr_to_markdown_impl(child, output, depth)?;
                    }
                }

                "p" => {
                    for child in items.iter().skip(1) {
                        sexpr_to_markdown_impl(child, output, depth)?;
                    }
                    output.push_str("\n\n");
                }

                "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                    let level = tag[1..].parse::<usize>().unwrap_or(1);
                    for _ in 0..level {
                        output.push('#');
                    }
                    output.push(' ');
                    for child in items.iter().skip(1) {
                        sexpr_to_markdown_impl(child, output, depth)?;
                    }
                    output.push_str("\n\n");
                }

                "ul" => {
                    for child in items.iter().skip(1) {
                        render_list_item(child, output, depth, None)?;
                    }
                    if depth == 0 {
                        output.push('\n');
                    }
                }

                "ol" => {
                    // Check if second element is a start number
                    let (start, children_start) = if items.len() > 1 {
                        if let SExpr::Atom(s) = &items[1] {
                            if let Ok(n) = s.parse::<u32>() {
                                (n, 2)
                            } else {
                                (1, 1)
                            }
                        } else {
                            (1, 1)
                        }
                    } else {
                        (1, 1)
                    };

                    for (i, child) in items.iter().skip(children_start).enumerate() {
                        render_list_item(child, output, depth, Some(start + i as u32))?;
                    }
                    if depth == 0 {
                        output.push('\n');
                    }
                }

                "li" => {
                    // This shouldn't be called directly, but handle it
                    for child in items.iter().skip(1) {
                        sexpr_to_markdown_impl(child, output, depth)?;
                    }
                }

                "code-block" => {
                    if items.len() >= 3 {
                        let lang = extract_string(&items[1]);
                        let content = extract_string(&items[2]);
                        output.push_str("```");
                        output.push_str(&lang);
                        output.push('\n');
                        output.push_str(&content);
                        if !content.ends_with('\n') {
                            output.push('\n');
                        }
                        output.push_str("```\n\n");
                    }
                }

                "blockquote" => {
                    let mut bq_content = String::new();
                    for child in items.iter().skip(1) {
                        sexpr_to_markdown_impl(child, &mut bq_content, depth)?;
                    }
                    // Prefix each line with >
                    for line in bq_content.lines() {
                        output.push_str("> ");
                        output.push_str(line);
                        output.push('\n');
                    }
                    output.push('\n');
                }

                "hr" => {
                    output.push_str("---\n\n");
                }

                "code" => {
                    if items.len() >= 2 {
                        let content = extract_string(&items[1]);
                        output.push('`');
                        output.push_str(&content);
                        output.push('`');
                    }
                }

                "em" => {
                    output.push('*');
                    for child in items.iter().skip(1) {
                        sexpr_to_markdown_impl(child, output, depth)?;
                    }
                    output.push('*');
                }

                "strong" => {
                    output.push_str("**");
                    for child in items.iter().skip(1) {
                        sexpr_to_markdown_impl(child, output, depth)?;
                    }
                    output.push_str("**");
                }

                "link" => {
                    if items.len() >= 3 {
                        let url = extract_string(&items[1]);
                        let title = extract_string(&items[2]);
                        output.push('[');
                        for child in items.iter().skip(3) {
                            sexpr_to_markdown_impl(child, output, depth)?;
                        }
                        output.push_str("](");
                        output.push_str(&url);
                        if !title.is_empty() {
                            output.push_str(" \"");
                            output.push_str(&title);
                            output.push('"');
                        }
                        output.push(')');
                    }
                }

                "img" => {
                    if items.len() >= 4 {
                        let url = extract_string(&items[1]);
                        let alt = extract_string(&items[2]);
                        let title = extract_string(&items[3]);
                        output.push_str("![");
                        output.push_str(&alt);
                        output.push_str("](");
                        output.push_str(&url);
                        if !title.is_empty() {
                            output.push_str(" \"");
                            output.push_str(&title);
                            output.push('"');
                        }
                        output.push(')');
                    }
                }

                "br" => {
                    output.push_str("  \n");
                }

                "html" => {
                    if items.len() >= 2 {
                        let content = extract_string(&items[1]);
                        output.push_str(&content);
                    }
                }

                "def" => {
                    if items.len() >= 4 {
                        let label = extract_string(&items[1]);
                        let url = extract_string(&items[2]);
                        let title = extract_string(&items[3]);
                        output.push('[');
                        output.push_str(&label);
                        output.push_str("]: ");
                        output.push_str(&url);
                        if !title.is_empty() {
                            output.push_str(" \"");
                            output.push_str(&title);
                            output.push('"');
                        }
                        output.push_str("\n\n");
                    }
                }

                "link-ref" => {
                    if items.len() >= 2 {
                        let identifier = extract_string(&items[1]);
                        output.push('[');
                        for child in items.iter().skip(2) {
                            sexpr_to_markdown_impl(child, output, depth)?;
                        }
                        output.push_str("][");
                        output.push_str(&identifier);
                        output.push(']');
                    }
                }

                "img-ref" => {
                    if items.len() >= 3 {
                        let identifier = extract_string(&items[1]);
                        let alt = extract_string(&items[2]);
                        output.push_str("![");
                        output.push_str(&alt);
                        output.push_str("][");
                        output.push_str(&identifier);
                        output.push(']');
                    }
                }

                "yaml" => {
                    if items.len() >= 2 {
                        let content = extract_string(&items[1]);
                        output.push_str("---\n");
                        output.push_str(&content);
                        if !content.ends_with('\n') {
                            output.push('\n');
                        }
                        output.push_str("---\n\n");
                    }
                }

                "toml" => {
                    if items.len() >= 2 {
                        let content = extract_string(&items[1]);
                        output.push_str("+++\n");
                        output.push_str(&content);
                        if !content.ends_with('\n') {
                            output.push('\n');
                        }
                        output.push_str("+++\n\n");
                    }
                }

                "table" => {
                    // Basic table rendering
                    let rows: Vec<_> = items.iter().skip(1).collect();
                    if !rows.is_empty() {
                        // Render header row
                        if let Some(first_row) = rows.first() {
                            render_table_row(first_row, output, depth)?;
                            // Render separator
                            if let SExpr::List(cells) = first_row {
                                let cell_count = cells.len().saturating_sub(1);
                                output.push('|');
                                for _ in 0..cell_count {
                                    output.push_str(" --- |");
                                }
                                output.push('\n');
                            }
                        }
                        // Render remaining rows
                        for row in rows.iter().skip(1) {
                            render_table_row(row, output, depth)?;
                        }
                        output.push('\n');
                    }
                }

                "tr" => {
                    render_table_row(expr, output, depth)?;
                }

                "td" | "th" => {
                    for child in items.iter().skip(1) {
                        sexpr_to_markdown_impl(child, output, depth)?;
                    }
                }

                "footnote-def" => {
                    if items.len() >= 2 {
                        let identifier = extract_string(&items[1]);
                        output.push_str("[^");
                        output.push_str(&identifier);
                        output.push_str("]: ");
                        for child in items.iter().skip(2) {
                            sexpr_to_markdown_impl(child, output, depth)?;
                        }
                    }
                }

                "footnote-ref" => {
                    if items.len() >= 2 {
                        let identifier = extract_string(&items[1]);
                        output.push_str("[^");
                        output.push_str(&identifier);
                        output.push(']');
                    }
                }

                "del" => {
                    output.push_str("~~");
                    for child in items.iter().skip(1) {
                        sexpr_to_markdown_impl(child, output, depth)?;
                    }
                    output.push_str("~~");
                }

                "math-block" => {
                    if items.len() >= 2 {
                        let content = extract_string(&items[1]);
                        output.push_str("$$\n");
                        output.push_str(&content);
                        if !content.ends_with('\n') {
                            output.push('\n');
                        }
                        output.push_str("$$\n\n");
                    }
                }

                "math" => {
                    if items.len() >= 2 {
                        let content = extract_string(&items[1]);
                        output.push('$');
                        output.push_str(&content);
                        output.push('$');
                    }
                }

                _ => {
                    return Err(SError::new("sexpr-to-markdown")
                        .with_code("unknown-tag")
                        .with_message("Unknown markdown element tag")
                        .with_string_field("tag", tag));
                }
            }

            Ok(())
        }
    }
}

/// Renders a list item with proper indentation and marker.
fn render_list_item(
    item: &SExpr,
    output: &mut String,
    depth: usize,
    number: Option<u32>,
) -> SResult<()> {
    let indent = "  ".repeat(depth);

    if let SExpr::List(items) = item {
        if items.is_empty() {
            return Ok(());
        }

        // Check if first element is "li" tag
        if let SExpr::Atom(tag) = &items[0]
            && tag == "li"
        {
            // Render marker
            output.push_str(&indent);
            if let Some(n) = number {
                output.push_str(&n.to_string());
                output.push_str(". ");
            } else {
                output.push_str("- ");
            }

            // Render children
            let mut first = true;
            for child in items.iter().skip(1) {
                if !first && is_list(child) {
                    output.push('\n');
                    sexpr_to_markdown_impl(child, output, depth + 1)?;
                    continue;
                }
                first = false;

                // Handle nested content
                if is_list(child) {
                    output.push('\n');
                    sexpr_to_markdown_impl(child, output, depth + 1)?;
                } else {
                    let mut item_content = String::new();
                    sexpr_to_markdown_impl(child, &mut item_content, depth + 1)?;
                    // Remove trailing newlines from inline content
                    let trimmed = item_content.trim_end_matches('\n');
                    output.push_str(trimmed);
                }
            }
            output.push('\n');
        }
    }

    Ok(())
}

/// Checks if an expression represents a list (ul or ol).
fn is_list(expr: &SExpr) -> bool {
    if let SExpr::List(items) = expr
        && let Some(SExpr::Atom(tag)) = items.first()
    {
        return tag == "ul" || tag == "ol";
    }
    false
}

/// Renders a table row.
fn render_table_row(row: &SExpr, output: &mut String, depth: usize) -> SResult<()> {
    if let SExpr::List(items) = row {
        output.push('|');
        for item in items.iter().skip(1) {
            output.push(' ');
            sexpr_to_markdown_impl(item, output, depth)?;
            output.push_str(" |");
        }
        output.push('\n');
    }
    Ok(())
}

// ============================================================================
// Frontmatter utilities
// ============================================================================

/// Returns the frontmatter node from a document if present.
/// Frontmatter is expected to be the first child after the doc tag,
/// with tag "yaml" or "toml".
pub fn get_frontmatter(doc: &SExpr) -> Option<SExpr> {
    match doc {
        SExpr::List(items) if items.len() >= 2 => {
            let first_child = &items[1];
            if let SExpr::List(child_items) = first_child
                && let Some(SExpr::Atom(tag)) = child_items.first()
                && (tag == "yaml" || tag == "toml")
            {
                return Some(first_child.clone());
            }
            None
        }
        _ => None,
    }
}

/// Returns the raw frontmatter content as a string.
pub fn get_frontmatter_content(doc: &SExpr) -> Option<String> {
    let fm = get_frontmatter(doc)?;
    if let SExpr::List(items) = fm
        && items.len() >= 2
    {
        return Some(extract_string(&items[1]));
    }
    None
}

/// Sets or replaces the frontmatter in a document.
/// If frontmatter exists, it is replaced. Otherwise, it is inserted at the beginning.
/// `format` should be "yaml" or "toml".
pub fn set_frontmatter(doc: &SExpr, format: &str, content: &str) -> SResult<SExpr> {
    if format != "yaml" && format != "toml" {
        return Err(SError::new("frontmatter")
            .with_code("invalid-format")
            .with_message("Frontmatter format must be 'yaml' or 'toml'")
            .with_string_field("format", format));
    }

    let new_fm = SExpr::List(vec![SExpr::Atom(format.to_string()), string_atom(content)]);

    match doc {
        SExpr::List(items) if !items.is_empty() => {
            let mut new_items = items.clone();

            // Check if first child is frontmatter
            if items.len() >= 2
                && let SExpr::List(child_items) = &items[1]
                && let Some(SExpr::Atom(tag)) = child_items.first()
                && (tag == "yaml" || tag == "toml")
            {
                // Replace existing frontmatter
                new_items[1] = new_fm;
                return Ok(SExpr::List(new_items));
            }

            // No existing frontmatter, insert after doc tag
            new_items.insert(1, new_fm);
            Ok(SExpr::List(new_items))
        }
        _ => Err(SError::new("frontmatter")
            .with_code("invalid-document")
            .with_message("Cannot set frontmatter on non-document expression")),
    }
}

/// Removes frontmatter from a document if present.
pub fn remove_frontmatter(doc: &SExpr) -> SExpr {
    match doc {
        SExpr::List(items) if items.len() >= 2 => {
            if let SExpr::List(child_items) = &items[1]
                && let Some(SExpr::Atom(tag)) = child_items.first()
                && (tag == "yaml" || tag == "toml")
            {
                let mut new_items = items.clone();
                new_items.remove(1);
                return SExpr::List(new_items);
            }
            doc.clone()
        }
        _ => doc.clone(),
    }
}

/// Parses YAML frontmatter into key-value pairs.
/// This is a simple parser that handles basic `key: value` format.
/// Returns an s-expression object: (obj ("key1" "value1") ("key2" "value2") ...)
pub fn parse_yaml_frontmatter(content: &str) -> SExpr {
    let mut pairs = vec![SExpr::Atom("obj".to_string())];

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        if let Some(colon_pos) = line.find(':') {
            let key = line[..colon_pos].trim();
            let value = line[colon_pos + 1..].trim();

            // Handle quoted values
            let value = if (value.starts_with('"') && value.ends_with('"'))
                || (value.starts_with('\'') && value.ends_with('\''))
            {
                &value[1..value.len() - 1]
            } else {
                value
            };

            pairs.push(SExpr::List(vec![string_atom(key), string_atom(value)]));
        }
    }

    SExpr::List(pairs)
}

/// Gets a specific field from parsed frontmatter.
pub fn get_frontmatter_field(frontmatter_obj: &SExpr, key: &str) -> Option<String> {
    if let SExpr::List(items) = frontmatter_obj {
        for item in items.iter().skip(1) {
            if let SExpr::List(pair) = item
                && pair.len() == 2
            {
                let k = extract_string(&pair[0]);
                if k == key {
                    return Some(extract_string(&pair[1]));
                }
            }
        }
    }
    None
}

/// Updates or inserts a field in YAML frontmatter content.
/// Returns the new frontmatter content string.
pub fn upsert_frontmatter_field(content: &str, key: &str, value: &str) -> String {
    let mut lines: Vec<String> = Vec::new();
    let mut found = false;

    for line in content.lines() {
        let trimmed = line.trim();
        if !trimmed.is_empty()
            && !trimmed.starts_with('#')
            && let Some(colon_pos) = trimmed.find(':')
        {
            let line_key = trimmed[..colon_pos].trim();
            if line_key == key {
                lines.push(format!("{}: {}", key, value));
                found = true;
                continue;
            }
        }
        lines.push(line.to_string());
    }

    if !found {
        lines.push(format!("{}: {}", key, value));
    }

    lines.join("\n")
}

/// Removes a field from YAML frontmatter content.
/// Returns the new frontmatter content string.
pub fn remove_frontmatter_field(content: &str, key: &str) -> String {
    let mut lines: Vec<String> = Vec::new();

    for line in content.lines() {
        let trimmed = line.trim();
        if !trimmed.is_empty()
            && !trimmed.starts_with('#')
            && let Some(colon_pos) = trimmed.find(':')
        {
            let line_key = trimmed[..colon_pos].trim();
            if line_key == key {
                continue;
            }
        }
        lines.push(line.to_string());
    }

    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::escape_string;

    #[test]
    fn parse_simple_paragraph() {
        let md = "Hello, world!";
        let sexpr = markdown_to_sexpr(md).unwrap();
        assert_eq!(sexpr.to_string(), r#"(doc (p "Hello, world!"))"#);
    }

    #[test]
    fn parse_heading() {
        let md = "# Title";
        let sexpr = markdown_to_sexpr(md).unwrap();
        assert_eq!(sexpr.to_string(), r#"(doc (h1 "Title"))"#);
    }

    #[test]
    fn parse_multiple_heading_levels() {
        let md = "## Level 2\n\n### Level 3";
        let sexpr = markdown_to_sexpr(md).unwrap();
        assert!(sexpr.to_string().contains("(h2 \"Level 2\")"));
        assert!(sexpr.to_string().contains("(h3 \"Level 3\")"));
    }

    #[test]
    fn parse_emphasis() {
        let md = "This is *emphasized* text.";
        let sexpr = markdown_to_sexpr(md).unwrap();
        assert!(sexpr.to_string().contains("(em \"emphasized\")"));
    }

    #[test]
    fn parse_strong() {
        let md = "This is **strong** text.";
        let sexpr = markdown_to_sexpr(md).unwrap();
        assert!(sexpr.to_string().contains("(strong \"strong\")"));
    }

    #[test]
    fn parse_link() {
        let md = "Click [here](https://example.com) for more.";
        let sexpr = markdown_to_sexpr(md).unwrap();
        assert!(
            sexpr
                .to_string()
                .contains("(link \"https://example.com\" \"\" \"here\")")
        );
    }

    #[test]
    fn parse_link_with_title() {
        let md = r#"Click [here](https://example.com "Example") for more."#;
        let sexpr = markdown_to_sexpr(md).unwrap();
        assert!(
            sexpr
                .to_string()
                .contains("(link \"https://example.com\" \"Example\" \"here\")")
        );
    }

    #[test]
    fn parse_unordered_list() {
        let md = "- Item 1\n- Item 2\n- Item 3";
        let sexpr = markdown_to_sexpr(md).unwrap();
        let s = sexpr.to_string();
        assert!(s.contains("(ul"));
        assert!(s.contains("(li"));
    }

    #[test]
    fn parse_ordered_list() {
        let md = "1. First\n2. Second\n3. Third";
        let sexpr = markdown_to_sexpr(md).unwrap();
        let s = sexpr.to_string();
        assert!(s.contains("(ol"));
        assert!(s.contains("(li"));
    }

    #[test]
    fn parse_code_block() {
        let md = "```rust\nfn main() {}\n```";
        let sexpr = markdown_to_sexpr(md).unwrap();
        let s = sexpr.to_string();
        assert!(s.contains("(code-block \"rust\""));
        assert!(s.contains("fn main()"));
    }

    #[test]
    fn parse_inline_code() {
        let md = "Use the `println!` macro.";
        let sexpr = markdown_to_sexpr(md).unwrap();
        assert!(sexpr.to_string().contains("(code \"println!\")"));
    }

    #[test]
    fn parse_blockquote() {
        let md = "> This is a quote.";
        let sexpr = markdown_to_sexpr(md).unwrap();
        assert!(sexpr.to_string().contains("(blockquote"));
    }

    #[test]
    fn parse_image() {
        let md = "![Alt text](image.png)";
        let sexpr = markdown_to_sexpr(md).unwrap();
        assert!(
            sexpr
                .to_string()
                .contains("(img \"image.png\" \"Alt text\" \"\")")
        );
    }

    #[test]
    fn parse_thematic_break() {
        let md = "Above\n\n---\n\nBelow";
        let sexpr = markdown_to_sexpr(md).unwrap();
        assert!(sexpr.to_string().contains("(hr)"));
    }

    #[test]
    fn roundtrip_simple_paragraph() {
        let md = "Hello, world!\n";
        let sexpr = markdown_to_sexpr(md).unwrap();
        let output = sexpr_to_markdown(&sexpr).unwrap();
        // Normalize: both should parse to same AST
        let reparsed = markdown_to_sexpr(&output).unwrap();
        assert_eq!(sexpr.to_string(), reparsed.to_string());
    }

    #[test]
    fn roundtrip_heading() {
        let md = "# My Title\n";
        let sexpr = markdown_to_sexpr(md).unwrap();
        let output = sexpr_to_markdown(&sexpr).unwrap();
        assert!(output.contains("# My Title"));
    }

    #[test]
    fn roundtrip_list() {
        let md = "- One\n- Two\n- Three\n";
        let sexpr = markdown_to_sexpr(md).unwrap();
        let output = sexpr_to_markdown(&sexpr).unwrap();
        assert!(output.contains("- One"));
        assert!(output.contains("- Two"));
        assert!(output.contains("- Three"));
    }

    #[test]
    fn roundtrip_code_block() {
        let md = "```python\nprint('hello')\n```\n";
        let sexpr = markdown_to_sexpr(md).unwrap();
        let output = sexpr_to_markdown(&sexpr).unwrap();
        assert!(output.contains("```python"));
        assert!(output.contains("print('hello')"));
    }

    #[test]
    fn roundtrip_link() {
        let md = "See [example](https://example.com) for details.\n";
        let sexpr = markdown_to_sexpr(md).unwrap();
        let output = sexpr_to_markdown(&sexpr).unwrap();
        assert!(output.contains("[example](https://example.com)"));
    }

    #[test]
    fn render_ordered_list_with_start() {
        let md = "5. Fifth\n6. Sixth\n";
        let sexpr = markdown_to_sexpr(md).unwrap();
        println!("DEBUG sexpr: {}", sexpr);
        let output = sexpr_to_markdown(&sexpr).unwrap();
        println!("DEBUG output: {}", output);
        assert!(output.contains("5. "));
        assert!(output.contains("6. "));
    }

    // Frontmatter tests

    #[test]
    fn get_frontmatter_none() {
        let doc = parse("(doc (h1 \"Title\"))");
        assert!(get_frontmatter(&doc).is_none());
    }

    #[test]
    fn get_frontmatter_yaml() {
        let doc = parse(r#"(doc (yaml "title: Hello") (h1 "Title"))"#);
        let fm = get_frontmatter(&doc);
        assert!(fm.is_some());
        assert!(fm.unwrap().to_string().contains("yaml"));
    }

    #[test]
    fn set_frontmatter_new() {
        let doc = parse("(doc (h1 \"Title\"))");
        let result = set_frontmatter(&doc, "yaml", "title: Hello").unwrap();
        assert!(result.to_string().contains("(yaml"));
        assert!(result.to_string().contains("title: Hello"));
    }

    #[test]
    fn set_frontmatter_replace() {
        let doc = parse(r#"(doc (yaml "title: Old") (h1 "Title"))"#);
        let result = set_frontmatter(&doc, "yaml", "title: New").unwrap();
        assert!(result.to_string().contains("title: New"));
        assert!(!result.to_string().contains("title: Old"));
    }

    #[test]
    fn remove_frontmatter_present() {
        let doc = parse(r#"(doc (yaml "title: Hello") (h1 "Title"))"#);
        let result = remove_frontmatter(&doc);
        assert!(!result.to_string().contains("yaml"));
        assert!(result.to_string().contains("(h1"));
    }

    #[test]
    fn remove_frontmatter_absent() {
        let doc = parse("(doc (h1 \"Title\"))");
        let result = remove_frontmatter(&doc);
        assert_eq!(result, doc);
    }

    #[test]
    fn parse_yaml_frontmatter_basic() {
        let content = "title: Hello\nauthor: Alice\n";
        let obj = parse_yaml_frontmatter(content);
        assert_eq!(
            get_frontmatter_field(&obj, "title"),
            Some("Hello".to_string())
        );
        assert_eq!(
            get_frontmatter_field(&obj, "author"),
            Some("Alice".to_string())
        );
    }

    #[test]
    fn upsert_frontmatter_field_new() {
        let content = "title: Hello";
        let result = upsert_frontmatter_field(content, "author", "Alice");
        assert!(result.contains("title: Hello"));
        assert!(result.contains("author: Alice"));
    }

    #[test]
    fn upsert_frontmatter_field_update() {
        let content = "title: Old\nauthor: Alice";
        let result = upsert_frontmatter_field(content, "title", "New");
        assert!(result.contains("title: New"));
        assert!(!result.contains("title: Old"));
        assert!(result.contains("author: Alice"));
    }

    #[test]
    fn remove_frontmatter_field_basic() {
        let content = "title: Hello\nauthor: Alice\ntags: rust";
        let result = remove_frontmatter_field(content, "author");
        assert!(result.contains("title: Hello"));
        assert!(!result.contains("author"));
        assert!(result.contains("tags: rust"));
    }

    fn parse(s: &str) -> SExpr {
        super::super::expr::Parser::new(s).parse().unwrap()
    }

    #[test]
    fn set_frontmatter_invalid_format_error() {
        let doc = parse("(doc (h1 \"Title\"))");
        let err = set_frontmatter(&doc, "invalid", "content").unwrap_err();
        assert_eq!(
            *err.detail(),
            SExpr::List(vec![
                SExpr::Atom("error".to_string()),
                SExpr::List(vec![
                    SExpr::Atom("phase".to_string()),
                    SExpr::Atom("frontmatter".to_string()),
                ]),
                SExpr::List(vec![
                    SExpr::Atom("code".to_string()),
                    SExpr::Atom("invalid-format".to_string()),
                ]),
                SExpr::List(vec![
                    SExpr::Atom("message".to_string()),
                    SExpr::Atom("\"Frontmatter format must be 'yaml' or 'toml'\"".to_string()),
                ]),
                SExpr::List(vec![
                    SExpr::Atom("format".to_string()),
                    SExpr::Atom("\"invalid\"".to_string()),
                ]),
            ])
        );
    }

    #[test]
    fn set_frontmatter_on_atom_error() {
        let doc = SExpr::Atom("atom".to_string());
        let err = set_frontmatter(&doc, "yaml", "content").unwrap_err();
        assert_eq!(
            *err.detail(),
            SExpr::List(vec![
                SExpr::Atom("error".to_string()),
                SExpr::List(vec![
                    SExpr::Atom("phase".to_string()),
                    SExpr::Atom("frontmatter".to_string()),
                ]),
                SExpr::List(vec![
                    SExpr::Atom("code".to_string()),
                    SExpr::Atom("invalid-document".to_string()),
                ]),
                SExpr::List(vec![
                    SExpr::Atom("message".to_string()),
                    SExpr::Atom(
                        "\"Cannot set frontmatter on non-document expression\"".to_string()
                    ),
                ]),
            ])
        );
    }

    #[test]
    fn set_frontmatter_toml() {
        let doc = parse("(doc (h1 \"Title\"))");
        let result = set_frontmatter(&doc, "toml", "title = \"Hello\"").unwrap();
        assert!(result.to_string().contains("(toml"));
    }

    #[test]
    fn get_frontmatter_toml() {
        let doc = parse(r#"(doc (toml "title = \"Hello\"") (h1 "Title"))"#);
        let fm = get_frontmatter(&doc);
        assert!(fm.is_some());
        assert!(fm.unwrap().to_string().contains("toml"));
    }

    #[test]
    fn get_frontmatter_content_none() {
        let doc = parse("(doc (h1 \"Title\"))");
        assert!(get_frontmatter_content(&doc).is_none());
    }

    #[test]
    fn get_frontmatter_content_yaml() {
        let doc = parse(r#"(doc (yaml "title: Hello") (h1 "Title"))"#);
        let content = get_frontmatter_content(&doc);
        assert_eq!(content, Some("title: Hello".to_string()));
    }

    #[test]
    fn parse_yaml_frontmatter_with_comments() {
        let content = "# comment\ntitle: Hello\n# another comment\nauthor: Alice";
        let obj = parse_yaml_frontmatter(content);
        assert_eq!(
            get_frontmatter_field(&obj, "title"),
            Some("Hello".to_string())
        );
        assert_eq!(
            get_frontmatter_field(&obj, "author"),
            Some("Alice".to_string())
        );
    }

    #[test]
    fn parse_yaml_frontmatter_quoted_values() {
        let content = "title: \"Hello World\"\nauthor: 'Alice'";
        let obj = parse_yaml_frontmatter(content);
        assert_eq!(
            get_frontmatter_field(&obj, "title"),
            Some("Hello World".to_string())
        );
        assert_eq!(
            get_frontmatter_field(&obj, "author"),
            Some("Alice".to_string())
        );
    }

    #[test]
    fn parse_yaml_frontmatter_empty() {
        let obj = parse_yaml_frontmatter("");
        assert_eq!(obj, SExpr::List(vec![SExpr::Atom("obj".to_string())]));
    }

    #[test]
    fn get_frontmatter_field_not_found() {
        let obj = parse_yaml_frontmatter("title: Hello");
        assert!(get_frontmatter_field(&obj, "nonexistent").is_none());
    }

    #[test]
    fn sexpr_to_markdown_unknown_tag_error() {
        let expr = parse("(unknown-tag \"content\")");
        let err = sexpr_to_markdown(&expr).unwrap_err();
        eprintln!("DEBUG unknown tag error: {}", err.detail());
        assert_eq!(
            *err.detail(),
            SExpr::List(vec![
                SExpr::Atom("error".to_string()),
                SExpr::List(vec![
                    SExpr::Atom("phase".to_string()),
                    SExpr::Atom("sexpr-to-markdown".to_string()),
                ]),
                SExpr::List(vec![
                    SExpr::Atom("code".to_string()),
                    SExpr::Atom("unknown-tag".to_string()),
                ]),
                SExpr::List(vec![
                    SExpr::Atom("message".to_string()),
                    SExpr::Atom("\"Unknown markdown element tag\"".to_string()),
                ]),
                SExpr::List(vec![
                    SExpr::Atom("tag".to_string()),
                    SExpr::Atom("\"unknown-tag\"".to_string()),
                ]),
            ])
        );
    }

    #[test]
    fn sexpr_to_markdown_empty_list() {
        let expr = SExpr::List(vec![]);
        let result = sexpr_to_markdown(&expr).unwrap();
        assert_eq!(result, "");
    }

    #[test]
    fn roundtrip_blockquote() {
        let md = "> This is quoted.\n";
        let sexpr = markdown_to_sexpr(md).unwrap();
        let output = sexpr_to_markdown(&sexpr).unwrap();
        assert!(output.contains("> "));
    }

    #[test]
    fn roundtrip_image() {
        let md = "![alt](url.png)\n";
        let sexpr = markdown_to_sexpr(md).unwrap();
        let output = sexpr_to_markdown(&sexpr).unwrap();
        assert!(output.contains("![alt](url.png)"));
    }

    #[test]
    fn roundtrip_thematic_break() {
        let md = "Above\n\n---\n\nBelow\n";
        let sexpr = markdown_to_sexpr(md).unwrap();
        let output = sexpr_to_markdown(&sexpr).unwrap();
        assert!(output.contains("---"));
    }

    #[test]
    fn roundtrip_inline_code() {
        let md = "Use `code` here.\n";
        let sexpr = markdown_to_sexpr(md).unwrap();
        let output = sexpr_to_markdown(&sexpr).unwrap();
        assert!(output.contains("`code`"));
    }

    #[test]
    fn roundtrip_emphasis_and_strong() {
        let md = "This is *emphasized* and **strong**.\n";
        let sexpr = markdown_to_sexpr(md).unwrap();
        let output = sexpr_to_markdown(&sexpr).unwrap();
        assert!(output.contains("*emphasized*"));
        assert!(output.contains("**strong**"));
    }

    #[test]
    fn render_link_with_title() {
        let expr = parse(r#"(doc (p (link "url" "Title" "text")))"#);
        let output = sexpr_to_markdown(&expr).unwrap();
        assert!(output.contains("[text](url \"Title\")"));
    }

    #[test]
    fn render_image_with_title() {
        let expr = parse(r#"(doc (p (img "url.png" "alt" "Title")))"#);
        let output = sexpr_to_markdown(&expr).unwrap();
        assert!(output.contains("![alt](url.png \"Title\")"));
    }

    #[test]
    fn render_definition() {
        let expr = parse(r#"(doc (def "ref" "https://example.com" "Example"))"#);
        let output = sexpr_to_markdown(&expr).unwrap();
        assert!(output.contains("[ref]: https://example.com \"Example\""));
    }

    #[test]
    fn render_link_ref() {
        let expr = parse(r#"(doc (p (link-ref "ref" "text")))"#);
        let output = sexpr_to_markdown(&expr).unwrap();
        assert!(output.contains("[text][ref]"));
    }

    #[test]
    fn render_img_ref() {
        let expr = parse(r#"(doc (p (img-ref "ref" "alt")))"#);
        let output = sexpr_to_markdown(&expr).unwrap();
        assert!(output.contains("![alt][ref]"));
    }

    #[test]
    fn render_yaml_frontmatter() {
        let expr = parse(r#"(doc (yaml "title: Hello"))"#);
        let output = sexpr_to_markdown(&expr).unwrap();
        assert!(output.contains("---\ntitle: Hello\n---"));
    }

    #[test]
    fn render_toml_frontmatter() {
        let expr = parse(r#"(doc (toml "title = \"Hello\""))"#);
        let output = sexpr_to_markdown(&expr).unwrap();
        assert!(output.contains("+++\n"));
    }

    #[test]
    fn render_html() {
        let expr = parse(r#"(doc (p (html "<span>test</span>")))"#);
        let output = sexpr_to_markdown(&expr).unwrap();
        assert!(output.contains("<span>test</span>"));
    }

    #[test]
    fn render_break() {
        let expr = parse(r#"(doc (p "line1" (br) "line2"))"#);
        let output = sexpr_to_markdown(&expr).unwrap();
        assert!(output.contains("line1  \nline2"));
    }

    #[test]
    fn escape_string_all_chars() {
        assert_eq!(escape_string("a\\b"), "a\\\\b");
        assert_eq!(escape_string("a\"b"), "a\\\"b");
        assert_eq!(escape_string("a\nb"), "a\\nb");
        assert_eq!(escape_string("a\rb"), "a\\rb");
        assert_eq!(escape_string("a\tb"), "a\\tb");
    }

    #[test]
    fn extract_string_unquoted() {
        let atom = SExpr::Atom("unquoted".to_string());
        assert_eq!(extract_string(&atom), "unquoted");
    }

    #[test]
    fn extract_string_from_list() {
        let list = SExpr::List(vec![SExpr::Atom("tag".to_string())]);
        assert_eq!(extract_string(&list), "");
    }

    #[test]
    fn is_list_true_ul() {
        let expr = parse("(ul (li \"item\"))");
        assert!(is_list(&expr));
    }

    #[test]
    fn is_list_true_ol() {
        let expr = parse("(ol (li \"item\"))");
        assert!(is_list(&expr));
    }

    #[test]
    fn is_list_false() {
        let expr = parse("(p \"not a list\")");
        assert!(!is_list(&expr));
    }
}

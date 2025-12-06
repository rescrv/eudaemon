//! High-level curation operations for markdown documents.
//!
//! These functions encapsulate common multi-step operations that agents
//! frequently need when curating knowledge bases.

use crate::s::error::{SError, SResult};
use crate::s::expr::SExpr;
use crate::s::nodeid::{PathId, get_by_path};

use super::mutations::{insert_after, prepend_child, prune, replace_at};

/// Result of extracting content to a separate document.
#[derive(Debug, Clone)]
pub struct ExtractResult {
    /// The modified source document with a link/summary replacing the extracted content.
    pub source_doc: SExpr,
    /// The content for the new document.
    pub target_doc: SExpr,
    /// Suggested filename for the new document.
    pub suggested_filename: String,
}

/// Wraps nodes in a callout/admonition blockquote.
///
/// Creates a blockquote with the specified type and optional title:
/// ```markdown
/// > [!type] Title
/// > content...
/// ```
///
/// The callout format follows the Obsidian/GitHub convention.
pub fn wrap_in_callout(
    doc: &SExpr,
    paths: &[PathId],
    callout_type: &str,
    title: Option<&str>,
) -> SResult<SExpr> {
    if paths.is_empty() {
        return Err(SError::new("curation")
            .with_code("empty-paths")
            .with_message("No paths provided to wrap_in_callout"));
    }

    // Collect the nodes to wrap
    let mut nodes_to_wrap = Vec::new();
    for path in paths {
        let node = get_by_path(doc, path).ok_or_else(|| {
            SError::new("curation")
                .with_code("node-not-found")
                .with_message("Node not found at path")
                .with_string_field("path", &path.to_string())
        })?;
        nodes_to_wrap.push((path.clone(), node));
    }

    // Build the callout header
    let header_text = match title {
        Some(t) => format!("[!{}] {}", callout_type, t),
        None => format!("[!{}]", callout_type),
    };

    // Build the blockquote content
    let mut blockquote_children = vec![SExpr::Atom("blockquote".to_string())];

    // Add header paragraph
    blockquote_children.push(SExpr::List(vec![
        SExpr::Atom("p".to_string()),
        string_atom(&header_text),
    ]));

    // Add the wrapped nodes (stripping outer paragraph wrappers if they're paragraphs)
    for (_, node) in &nodes_to_wrap {
        blockquote_children.push(node.clone());
    }

    let blockquote = SExpr::List(blockquote_children);

    // Remove the original nodes (in reverse order to maintain path validity)
    let mut result = doc.clone();
    let mut sorted_paths: Vec<_> = paths.iter().collect();
    sorted_paths.sort_by(|a, b| b.indices().cmp(a.indices())); // Reverse order

    for (i, path) in sorted_paths.iter().enumerate() {
        if i == sorted_paths.len() - 1 {
            // Replace the first (last in reverse order) node with the blockquote
            result = replace_at(&result, path, blockquote.clone())?;
        } else {
            result = prune(&result, path)?;
        }
    }

    Ok(result)
}

/// Normalizes heading levels in a document according to a depth map.
///
/// The depth_map specifies the target level for each current level.
/// For example, `[(2, 1), (3, 2), (4, 3)]` would promote all h2->h1, h3->h2, h4->h3.
pub fn normalize_headers(doc: &SExpr, depth_map: &[(u8, u8)]) -> SResult<SExpr> {
    normalize_headers_impl(doc, depth_map)
}

fn normalize_headers_impl(expr: &SExpr, depth_map: &[(u8, u8)]) -> SResult<SExpr> {
    match expr {
        SExpr::Atom(_) => Ok(expr.clone()),
        SExpr::List(items) => {
            if items.is_empty() {
                return Ok(expr.clone());
            }

            // Check if this is a heading
            if let SExpr::Atom(tag) = &items[0]
                && tag.len() == 2
                && tag.starts_with('h')
                && let Ok(level) = tag[1..].parse::<u8>()
            {
                // Look for mapping
                for (from, to) in depth_map {
                    if level == *from {
                        let new_tag = format!("h{}", to);
                        let mut new_items = items.clone();
                        new_items[0] = SExpr::Atom(new_tag);
                        // Recursively process children
                        for item in new_items.iter_mut().skip(1) {
                            *item = normalize_headers_impl(item, depth_map)?;
                        }
                        return Ok(SExpr::List(new_items));
                    }
                }
            }

            // Recursively process all children
            let new_items: SResult<Vec<SExpr>> = items
                .iter()
                .map(|item| normalize_headers_impl(item, depth_map))
                .collect();
            Ok(SExpr::List(new_items?))
        }
    }
}

/// Extracts content from a document to a new document, leaving a link behind.
///
/// Returns an `ExtractResult` containing:
/// - The modified source document with a link/summary
/// - The new document containing the extracted content
/// - A suggested filename
pub fn extract_to_ref(
    doc: &SExpr,
    paths: &[PathId],
    target_filename: &str,
    summary: Option<&str>,
) -> SResult<ExtractResult> {
    if paths.is_empty() {
        return Err(SError::new("curation")
            .with_code("empty-paths")
            .with_message("No paths provided to extract_to_ref"));
    }

    // Collect nodes to extract
    let mut nodes_to_extract = Vec::new();
    for path in paths {
        let node = get_by_path(doc, path).ok_or_else(|| {
            SError::new("curation")
                .with_code("node-not-found")
                .with_message("Node not found at path")
                .with_string_field("path", &path.to_string())
        })?;
        nodes_to_extract.push((path.clone(), node));
    }

    // Build the target document
    let mut target_children = vec![SExpr::Atom("doc".to_string())];
    for (_, node) in &nodes_to_extract {
        target_children.push(node.clone());
    }
    let target_doc = SExpr::List(target_children);

    // Build the link/reference to replace the extracted content
    let link_text = summary.unwrap_or("See extracted content");
    let link = SExpr::List(vec![
        SExpr::Atom("p".to_string()),
        SExpr::List(vec![
            SExpr::Atom("link".to_string()),
            string_atom(target_filename),
            string_atom(""),
            string_atom(link_text),
        ]),
    ]);

    // Remove nodes from source and insert link
    let mut result = doc.clone();
    let mut sorted_paths: Vec<_> = paths.iter().collect();
    sorted_paths.sort_by(|a, b| b.indices().cmp(a.indices()));

    for (i, path) in sorted_paths.iter().enumerate() {
        if i == sorted_paths.len() - 1 {
            result = replace_at(&result, path, link.clone())?;
        } else {
            result = prune(&result, path)?;
        }
    }

    Ok(ExtractResult {
        source_doc: result,
        target_doc,
        suggested_filename: target_filename.to_string(),
    })
}

/// Merges content from a source section into a target section.
///
/// Strategies:
/// - `"append"`: Add source content after target content
/// - `"prepend"`: Add source content before target content
/// - `"replace"`: Replace target content with source content
pub fn merge_sections(
    doc: &SExpr,
    source_path: &PathId,
    target_path: &PathId,
    strategy: &str,
) -> SResult<SExpr> {
    let source_node = get_by_path(doc, source_path).ok_or_else(|| {
        SError::new("curation")
            .with_code("source-not-found")
            .with_message("Source node not found")
            .with_string_field("path", &source_path.to_string())
    })?;

    let target_node = get_by_path(doc, target_path).ok_or_else(|| {
        SError::new("curation")
            .with_code("target-not-found")
            .with_message("Target node not found")
            .with_string_field("path", &target_path.to_string())
    })?;

    // Get children of both nodes (skip tag)
    let source_children = match &source_node {
        SExpr::List(items) if items.len() > 1 => items[1..].to_vec(),
        _ => vec![source_node.clone()],
    };

    let (target_tag, target_children) = match &target_node {
        SExpr::List(items) if !items.is_empty() => {
            let tag = items[0].clone();
            let children = if items.len() > 1 {
                items[1..].to_vec()
            } else {
                vec![]
            };
            (tag, children)
        }
        _ => {
            return Err(SError::new("curation")
                .with_code("invalid-target")
                .with_message("Target must be a list node"));
        }
    };

    // Build merged node
    let merged_children = match strategy {
        "append" => {
            let mut c = vec![target_tag];
            c.extend(target_children);
            c.extend(source_children);
            c
        }
        "prepend" => {
            let mut c = vec![target_tag];
            c.extend(source_children);
            c.extend(target_children);
            c
        }
        "replace" => {
            let mut c = vec![target_tag];
            c.extend(source_children);
            c
        }
        _ => {
            return Err(SError::new("curation")
                .with_code("invalid-strategy")
                .with_message("Strategy must be 'append', 'prepend', or 'replace'")
                .with_string_field("strategy", strategy));
        }
    };

    let merged_node = SExpr::List(merged_children);

    // Replace target with merged, then remove source
    let result = replace_at(doc, target_path, merged_node)?;

    // Adjust source path if needed (if target was before source)
    // For simplicity, we prune after replace
    prune(&result, source_path)
}

/// Wraps content in a details/summary disclosure element.
///
/// Creates:
/// ```markdown
/// <details>
/// <summary>Title</summary>
/// content...
/// </details>
/// ```
pub fn wrap_in_details(doc: &SExpr, paths: &[PathId], summary_text: &str) -> SResult<SExpr> {
    if paths.is_empty() {
        return Err(SError::new("curation")
            .with_code("empty-paths")
            .with_message("No paths provided to wrap_in_details"));
    }

    // Collect nodes
    let mut nodes_to_wrap = Vec::new();
    for path in paths {
        let node = get_by_path(doc, path).ok_or_else(|| {
            SError::new("curation")
                .with_code("node-not-found")
                .with_message("Node not found at path")
                .with_string_field("path", &path.to_string())
        })?;
        nodes_to_wrap.push((path.clone(), node));
    }

    // Build HTML wrapper
    let open_tag = format!("<details>\n<summary>{}</summary>\n", summary_text);
    let close_tag = "</details>";

    // Wrap with html tags before and after
    let html_open = SExpr::List(vec![
        SExpr::Atom("html".to_string()),
        string_atom(&open_tag),
    ]);

    let html_close = SExpr::List(vec![
        SExpr::Atom("html".to_string()),
        string_atom(close_tag),
    ]);

    // Strategy: insert html_open before first node, html_close after last node
    let mut result = doc.clone();

    // Sort paths to find first and last
    let mut sorted_paths: Vec<_> = paths.iter().collect();
    sorted_paths.sort_by(|a, b| a.indices().cmp(b.indices()));

    let first_path = sorted_paths.first().unwrap();
    let last_path = sorted_paths.last().unwrap();

    // Insert close tag after last
    result = insert_after(&result, last_path, html_close)?;
    // Insert open tag before first
    result = super::mutations::insert_before(&result, first_path, html_open)?;

    Ok(result)
}

/// Adds a deprecation notice to content.
///
/// Wraps the content in a warning callout and optionally adds frontmatter.
pub fn mark_deprecated(
    doc: &SExpr,
    paths: &[PathId],
    reason: Option<&str>,
    replacement_link: Option<&str>,
) -> SResult<SExpr> {
    let mut notice_parts = vec!["This content is deprecated.".to_string()];

    if let Some(r) = reason {
        notice_parts.push(format!("Reason: {}", r));
    }

    if let Some(link) = replacement_link {
        notice_parts.push(format!("See: {}", link));
    }

    let notice_text = notice_parts.join(" ");

    // Create a warning paragraph
    let warning = SExpr::List(vec![
        SExpr::Atom("blockquote".to_string()),
        SExpr::List(vec![
            SExpr::Atom("p".to_string()),
            string_atom("[!warning] Deprecated"),
        ]),
        SExpr::List(vec![
            SExpr::Atom("p".to_string()),
            string_atom(&notice_text),
        ]),
    ]);

    // Insert warning before the first deprecated element
    if paths.is_empty() {
        // Insert at document level
        prepend_child(doc, &PathId::root(), warning)
    } else {
        let mut sorted_paths: Vec<_> = paths.iter().collect();
        sorted_paths.sort_by(|a, b| a.indices().cmp(b.indices()));
        let first_path = sorted_paths.first().unwrap();
        super::mutations::insert_before(doc, first_path, warning)
    }
}

/// Creates a table of contents from headings in the document.
///
/// Returns an s-expression representing the ToC as a nested list.
pub fn generate_toc(doc: &SExpr) -> SExpr {
    let mut toc_items = vec![SExpr::Atom("ul".to_string())];
    collect_headings_for_toc(doc, &mut toc_items);
    SExpr::List(toc_items)
}

fn collect_headings_for_toc(expr: &SExpr, toc_items: &mut Vec<SExpr>) {
    match expr {
        SExpr::List(items) if !items.is_empty() => {
            if let SExpr::Atom(tag) = &items[0]
                && tag.len() == 2
                && tag.starts_with('h')
                && tag[1..].parse::<u8>().is_ok()
            {
                // Extract heading text
                let text = extract_text_content(&SExpr::List(items.clone()));
                let slug = slugify(&text);

                // Create link to heading
                let link = SExpr::List(vec![
                    SExpr::Atom("link".to_string()),
                    string_atom(&format!("#{}", slug)),
                    string_atom(""),
                    string_atom(&text),
                ]);

                let li = SExpr::List(vec![
                    SExpr::Atom("li".to_string()),
                    SExpr::List(vec![SExpr::Atom("p".to_string()), link]),
                ]);

                toc_items.push(li);
            }

            // Recurse into children
            for child in items.iter().skip(1) {
                collect_headings_for_toc(child, toc_items);
            }
        }
        _ => {}
    }
}

/// Extracts plain text content from an s-expression node.
fn extract_text_content(expr: &SExpr) -> String {
    match expr {
        SExpr::Atom(s) => {
            if s.starts_with('"') && s.ends_with('"') && s.len() >= 2 {
                super::super::json::unescape_string(&s[1..s.len() - 1])
            } else {
                s.clone()
            }
        }
        SExpr::List(items) => {
            let mut text = String::new();
            for item in items.iter().skip(1) {
                text.push_str(&extract_text_content(item));
            }
            text
        }
    }
}

/// Converts text to a URL-friendly slug.
fn slugify(text: &str) -> String {
    text.to_lowercase()
        .chars()
        .map(|c| {
            if c.is_alphanumeric() {
                c
            } else if c.is_whitespace() || c == '-' {
                '-'
            } else {
                '_'
            }
        })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

/// Helper to create a quoted string atom.
fn string_atom(s: &str) -> SExpr {
    SExpr::Atom(format!(
        "\"{}\"",
        s.replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('\n', "\\n")
    ))
}

// ============================================================================
// Link Analysis Functions
// ============================================================================

/// Information about a link found in a document.
#[derive(Debug, Clone)]
pub struct LinkInfo {
    /// Path to the link node.
    pub path: PathId,
    /// The URL/href of the link.
    pub url: String,
    /// The link text.
    pub text: String,
    /// Optional title attribute.
    pub title: String,
    /// Whether this is an internal link (starts with ./ or ../ or # or no protocol).
    pub is_internal: bool,
    /// Whether this is an image link.
    pub is_image: bool,
}

/// Scans a document for all links and returns information about each.
pub fn scan_links(doc: &SExpr) -> Vec<LinkInfo> {
    let mut links = Vec::new();
    scan_links_impl(doc, &PathId::root(), &mut links);
    links
}

fn scan_links_impl(expr: &SExpr, path: &PathId, links: &mut Vec<LinkInfo>) {
    match expr {
        SExpr::List(items) if !items.is_empty() => {
            if let SExpr::Atom(tag) = &items[0] {
                match tag.as_str() {
                    "link" if items.len() >= 4 => {
                        let url = extract_string_content(&items[1]);
                        let title = extract_string_content(&items[2]);
                        let text = items
                            .iter()
                            .skip(3)
                            .map(extract_text_content)
                            .collect::<Vec<_>>()
                            .join("");

                        links.push(LinkInfo {
                            path: path.clone(),
                            url: url.clone(),
                            text,
                            title,
                            is_internal: is_internal_link(&url),
                            is_image: false,
                        });
                    }
                    "img" if items.len() >= 4 => {
                        let url = extract_string_content(&items[1]);
                        let alt = extract_string_content(&items[2]);
                        let title = extract_string_content(&items[3]);

                        links.push(LinkInfo {
                            path: path.clone(),
                            url: url.clone(),
                            text: alt,
                            title,
                            is_internal: is_internal_link(&url),
                            is_image: true,
                        });
                    }
                    "link-ref" if items.len() >= 2 => {
                        let identifier = extract_string_content(&items[1]);
                        let text = items
                            .iter()
                            .skip(2)
                            .map(extract_text_content)
                            .collect::<Vec<_>>()
                            .join("");

                        links.push(LinkInfo {
                            path: path.clone(),
                            url: format!("[{}]", identifier),
                            text,
                            title: String::new(),
                            is_internal: true, // Reference links are typically internal
                            is_image: false,
                        });
                    }
                    "img-ref" if items.len() >= 3 => {
                        let identifier = extract_string_content(&items[1]);
                        let alt = extract_string_content(&items[2]);

                        links.push(LinkInfo {
                            path: path.clone(),
                            url: format!("[{}]", identifier),
                            text: alt,
                            title: String::new(),
                            is_internal: true,
                            is_image: true,
                        });
                    }
                    _ => {}
                }
            }

            // Recurse into children
            for (i, child) in items.iter().enumerate().skip(1) {
                scan_links_impl(child, &path.child(i), links);
            }
        }
        _ => {}
    }
}

/// Determines if a URL is an internal link.
fn is_internal_link(url: &str) -> bool {
    if url.is_empty() || url.starts_with('#') {
        return true;
    }
    if url.starts_with("./") || url.starts_with("../") {
        return true;
    }
    // Check for protocol
    !url.contains("://")
}

/// Extracts string content from a quoted atom.
fn extract_string_content(expr: &SExpr) -> String {
    match expr {
        SExpr::Atom(s) => {
            if s.starts_with('"') && s.ends_with('"') && s.len() >= 2 {
                super::super::json::unescape_string(&s[1..s.len() - 1])
            } else {
                s.clone()
            }
        }
        _ => String::new(),
    }
}

/// Returns all internal links from a document.
pub fn get_internal_links(doc: &SExpr) -> Vec<LinkInfo> {
    scan_links(doc)
        .into_iter()
        .filter(|l| l.is_internal)
        .collect()
}

/// Returns all external links from a document.
pub fn get_external_links(doc: &SExpr) -> Vec<LinkInfo> {
    scan_links(doc)
        .into_iter()
        .filter(|l| !l.is_internal)
        .collect()
}

/// Returns all image links from a document.
pub fn get_image_links(doc: &SExpr) -> Vec<LinkInfo> {
    scan_links(doc).into_iter().filter(|l| l.is_image).collect()
}

/// Collects all link definitions (reference-style link targets) from a document.
#[derive(Debug, Clone)]
pub struct LinkDefinition {
    /// The identifier/label.
    pub identifier: String,
    /// The URL.
    pub url: String,
    /// Optional title.
    pub title: String,
    /// Path to the definition node.
    pub path: PathId,
}

/// Scans for link definitions (def nodes).
pub fn scan_link_definitions(doc: &SExpr) -> Vec<LinkDefinition> {
    let mut defs = Vec::new();
    scan_definitions_impl(doc, &PathId::root(), &mut defs);
    defs
}

fn scan_definitions_impl(expr: &SExpr, path: &PathId, defs: &mut Vec<LinkDefinition>) {
    match expr {
        SExpr::List(items) if !items.is_empty() => {
            if let SExpr::Atom(tag) = &items[0]
                && tag == "def"
                && items.len() >= 4
            {
                defs.push(LinkDefinition {
                    identifier: extract_string_content(&items[1]),
                    url: extract_string_content(&items[2]),
                    title: extract_string_content(&items[3]),
                    path: path.clone(),
                });
            }

            // Recurse
            for (i, child) in items.iter().enumerate().skip(1) {
                scan_definitions_impl(child, &path.child(i), defs);
            }
        }
        _ => {}
    }
}

/// Finds links that reference undefined definitions.
pub fn find_undefined_references(doc: &SExpr) -> Vec<LinkInfo> {
    let definitions: std::collections::HashSet<String> = scan_link_definitions(doc)
        .into_iter()
        .map(|d| d.identifier)
        .collect();

    scan_links(doc)
        .into_iter()
        .filter(|link| {
            // Check if it's a reference link
            if link.url.starts_with('[') && link.url.ends_with(']') {
                let identifier = &link.url[1..link.url.len() - 1];
                !definitions.contains(identifier)
            } else {
                false
            }
        })
        .collect()
}

/// Updates a link's URL in the document.
pub fn update_link(doc: &SExpr, path: &PathId, new_url: &str) -> SResult<SExpr> {
    let node = get_by_path(doc, path).ok_or_else(|| {
        SError::new("link-analysis")
            .with_code("node-not-found")
            .with_message("No node at the specified path")
            .with_string_field("path", &path.to_string())
    })?;

    // Verify it's a link or image
    let new_node = match &node {
        SExpr::List(items) if items.len() >= 2 => {
            if let SExpr::Atom(tag) = &items[0] {
                match tag.as_str() {
                    "link" if items.len() >= 4 => {
                        let mut new_items = items.clone();
                        new_items[1] = string_atom(new_url);
                        SExpr::List(new_items)
                    }
                    "img" if items.len() >= 4 => {
                        let mut new_items = items.clone();
                        new_items[1] = string_atom(new_url);
                        SExpr::List(new_items)
                    }
                    _ => {
                        return Err(SError::new("link-analysis")
                            .with_code("not-a-link")
                            .with_message("Node is not a link or image")
                            .with_string_field("tag", tag));
                    }
                }
            } else {
                return Err(SError::new("link-analysis")
                    .with_code("invalid-node")
                    .with_message("Node tag is not an atom"));
            }
        }
        _ => {
            return Err(SError::new("link-analysis")
                .with_code("invalid-node")
                .with_message("Node is not a valid link structure"));
        }
    };

    replace_at(doc, path, new_node)
}

/// Creates an s-expression representation of link info for programmatic access.
pub fn link_info_to_sexpr(info: &LinkInfo) -> SExpr {
    SExpr::List(vec![
        SExpr::Atom("link-info".to_string()),
        SExpr::List(vec![
            SExpr::Atom("\"path\"".to_string()),
            string_atom(&info.path.to_string()),
        ]),
        SExpr::List(vec![
            SExpr::Atom("\"url\"".to_string()),
            string_atom(&info.url),
        ]),
        SExpr::List(vec![
            SExpr::Atom("\"text\"".to_string()),
            string_atom(&info.text),
        ]),
        SExpr::List(vec![
            SExpr::Atom("\"internal\"".to_string()),
            SExpr::Atom(if info.is_internal { "#t" } else { "#f" }.to_string()),
        ]),
        SExpr::List(vec![
            SExpr::Atom("\"image\"".to_string()),
            SExpr::Atom(if info.is_image { "#t" } else { "#f" }.to_string()),
        ]),
    ])
}

/// Returns all links as an s-expression array for easy processing.
pub fn scan_links_to_sexpr(doc: &SExpr) -> SExpr {
    let links = scan_links(doc);
    let mut result = vec![SExpr::Atom("arr".to_string())];
    for link in links {
        result.push(link_info_to_sexpr(&link));
    }
    SExpr::List(result)
}

// ============================================================================
// Node Tagging (Stigmergic Annotation)
// ============================================================================

/// Tag format used in HTML comments: <!-- @tag:key=value -->
const TAG_PREFIX: &str = "@tag:";

/// Attaches a metadata tag to a node via an HTML comment.
///
/// The tag is inserted as an HTML comment immediately before the target node:
/// `<!-- @tag:key=value -->`
///
/// This allows agents to leave metadata for future passes without polluting
/// the visible document content.
pub fn tag_node(doc: &SExpr, path: &PathId, key: &str, value: &str) -> SResult<SExpr> {
    // Verify the target path exists.
    get_by_path(doc, path).ok_or_else(|| {
        SError::new("tagging")
            .with_code("node-not-found")
            .with_message("No node at the specified path")
            .with_string_field("path", &path.to_string())
    })?;

    // Build the tag comment.
    let comment = format!("<!-- {}{}={} -->", TAG_PREFIX, key, value);
    let html_node = SExpr::List(vec![SExpr::Atom("html".to_string()), string_atom(&comment)]);

    // Insert before the target node.
    super::mutations::insert_before(doc, path, html_node)
}

/// Removes a tag from a node.
///
/// Looks for an HTML comment tag immediately before the node and removes it
/// if it matches the specified key.
pub fn remove_tag(doc: &SExpr, path: &PathId, key: &str) -> SResult<SExpr> {
    // Check if there's a tag comment before this node.
    let prev_path = get_previous_sibling_path(path).ok_or_else(|| {
        SError::new("tagging")
            .with_code("no-previous-sibling")
            .with_message("No previous sibling to check for tag")
    })?;

    let prev_node = get_by_path(doc, &prev_path).ok_or_else(|| {
        SError::new("tagging")
            .with_code("node-not-found")
            .with_message("Previous sibling not found")
    })?;

    // Check if it's an HTML tag comment with the specified key.
    if !is_tag_comment_for_key(&prev_node, key) {
        return Err(SError::new("tagging")
            .with_code("tag-not-found")
            .with_message("No matching tag found before node")
            .with_string_field("key", key));
    }

    // Remove the tag comment.
    prune(doc, &prev_path)
}

/// Information about a tagged node.
#[derive(Debug, Clone)]
pub struct TagInfo {
    /// Path to the tagged node (the node after the tag comment).
    pub path: PathId,
    /// The tag key.
    pub key: String,
    /// The tag value.
    pub value: String,
}

/// Finds all nodes with a specific tag key.
pub fn get_tagged_nodes(doc: &SExpr, key: &str) -> Vec<TagInfo> {
    let mut results = Vec::new();
    find_tags_recursive(doc, &PathId::root(), key, &mut results);
    results
}

/// Recursively searches for tag comments.
fn find_tags_recursive(expr: &SExpr, path: &PathId, key: &str, results: &mut Vec<TagInfo>) {
    if let SExpr::List(items) = expr {
        // Look for HTML tag comments followed by the tagged node.
        let mut i = 1; // Skip the tag element at index 0.
        while i < items.len() {
            let child_path = path.child(i);

            if let Some((k, v)) = extract_tag_from_node(&items[i])
                && k == key
                && i + 1 < items.len()
            {
                // The next node is the tagged node.
                results.push(TagInfo {
                    path: path.child(i + 1),
                    key: k,
                    value: v,
                });
            }

            // Recurse into children.
            find_tags_recursive(&items[i], &child_path, key, results);
            i += 1;
        }
    }
}

/// Extracts tag key and value from an HTML comment node.
fn extract_tag_from_node(node: &SExpr) -> Option<(String, String)> {
    if let SExpr::List(items) = node
        && items.len() >= 2
        && let SExpr::Atom(tag) = &items[0]
        && tag == "html"
    {
        let content = extract_string_content(&items[1]);
        return parse_tag_content(&content);
    }
    None
}

/// Parses tag content from an HTML comment.
/// Format: <!-- @tag:key=value -->
fn parse_tag_content(content: &str) -> Option<(String, String)> {
    let content = content.trim();
    if !content.starts_with("<!--") || !content.ends_with("-->") {
        return None;
    }
    let inner = content[4..content.len() - 3].trim();
    if !inner.starts_with(TAG_PREFIX) {
        return None;
    }
    let tag_content = &inner[TAG_PREFIX.len()..];

    let eq_pos = tag_content.find('=')?;
    let key = tag_content[..eq_pos].to_string();
    let value = tag_content[eq_pos + 1..].to_string();
    Some((key, value))
}

/// Checks if a node is a tag comment for a specific key.
fn is_tag_comment_for_key(node: &SExpr, key: &str) -> bool {
    if let Some((k, _)) = extract_tag_from_node(node) {
        return k == key;
    }
    false
}

/// Gets the previous sibling path.
fn get_previous_sibling_path(path: &PathId) -> Option<PathId> {
    let indices = path.indices();
    if indices.is_empty() {
        return None;
    }
    let last = *indices.last()?;
    if last == 0 {
        return None;
    }
    let mut new_indices = indices.to_vec();
    *new_indices.last_mut()? = last - 1;
    Some(PathId::new(new_indices))
}

// ============================================================================
// Topological Refactoring
// ============================================================================

/// Updates all links pointing to a source URL to point to a target URL.
///
/// This is useful when moving content between documents or renaming files,
/// to ensure all internal references remain valid.
pub fn rehome_orphans(doc: &SExpr, source_url: &str, target_url: &str) -> SResult<SExpr> {
    rehome_orphans_recursive(doc, source_url, target_url)
}

/// Recursively updates links.
fn rehome_orphans_recursive(expr: &SExpr, source_url: &str, target_url: &str) -> SResult<SExpr> {
    match expr {
        SExpr::Atom(_) => Ok(expr.clone()),
        SExpr::List(items) if items.is_empty() => Ok(expr.clone()),
        SExpr::List(items) => {
            // Check if this is a link or image with the source URL.
            if let SExpr::Atom(tag) = &items[0] {
                match tag.as_str() {
                    "link" | "img" if items.len() >= 2 => {
                        let url = extract_string_content(&items[1]);
                        if url == source_url {
                            let mut new_items = items.clone();
                            new_items[1] = string_atom(target_url);
                            // Recurse into remaining children.
                            for item in new_items.iter_mut().skip(2) {
                                *item = rehome_orphans_recursive(item, source_url, target_url)?;
                            }
                            return Ok(SExpr::List(new_items));
                        }
                    }
                    _ => {}
                }
            }

            // Recurse into all children.
            let new_items: SResult<Vec<SExpr>> = items
                .iter()
                .map(|item| rehome_orphans_recursive(item, source_url, target_url))
                .collect();
            Ok(SExpr::List(new_items?))
        }
    }
}

/// Strategy for extracting content to a new document.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtractStrategy {
    /// Replace extracted content with a link to the new document.
    LeaveLink,
    /// Replace with a transclusion directive (platform-specific).
    Transclude,
    /// Replace with a redirect notice in frontmatter style.
    Redirect,
}

/// Extracts content with a specified strategy.
///
/// This is an enhanced version of `extract_to_ref` that supports multiple
/// replacement strategies.
pub fn extract_with_strategy(
    doc: &SExpr,
    paths: &[PathId],
    target_filename: &str,
    strategy: ExtractStrategy,
    summary: Option<&str>,
) -> SResult<ExtractResult> {
    if paths.is_empty() {
        return Err(SError::new("curation")
            .with_code("empty-paths")
            .with_message("No paths provided to extract"));
    }

    // Collect nodes to extract.
    let mut nodes_to_extract = Vec::new();
    for path in paths {
        let node = get_by_path(doc, path).ok_or_else(|| {
            SError::new("curation")
                .with_code("node-not-found")
                .with_message("Node not found at path")
                .with_string_field("path", &path.to_string())
        })?;
        nodes_to_extract.push((path.clone(), node));
    }

    // Build the target document.
    let mut target_children = vec![SExpr::Atom("doc".to_string())];
    for (_, node) in &nodes_to_extract {
        target_children.push(node.clone());
    }
    let target_doc = SExpr::List(target_children);

    // Build the replacement node based on strategy.
    let replacement = match strategy {
        ExtractStrategy::LeaveLink => {
            let link_text = summary.unwrap_or("See extracted content");
            SExpr::List(vec![
                SExpr::Atom("p".to_string()),
                SExpr::List(vec![
                    SExpr::Atom("link".to_string()),
                    string_atom(target_filename),
                    string_atom(""),
                    string_atom(link_text),
                ]),
            ])
        }
        ExtractStrategy::Transclude => {
            // Use a common transclusion syntax: ![[filename]]
            let transclude_text = format!("![[{}]]", target_filename);
            SExpr::List(vec![
                SExpr::Atom("p".to_string()),
                string_atom(&transclude_text),
            ])
        }
        ExtractStrategy::Redirect => {
            // Leave a redirect notice.
            let notice = format!(
                "This content has been moved to [{}]({}).",
                target_filename, target_filename
            );
            SExpr::List(vec![
                SExpr::Atom("blockquote".to_string()),
                SExpr::List(vec![
                    SExpr::Atom("p".to_string()),
                    string_atom("[!info] Content Moved"),
                ]),
                SExpr::List(vec![SExpr::Atom("p".to_string()), string_atom(&notice)]),
            ])
        }
    };

    // Remove nodes from source and insert replacement.
    let mut result = doc.clone();
    let mut sorted_paths: Vec<_> = paths.iter().collect();
    sorted_paths.sort_by(|a, b| b.indices().cmp(a.indices()));

    for (i, path) in sorted_paths.iter().enumerate() {
        if i == sorted_paths.len() - 1 {
            result = replace_at(&result, path, replacement.clone())?;
        } else {
            result = prune(&result, path)?;
        }
    }

    Ok(ExtractResult {
        source_doc: result,
        target_doc,
        suggested_filename: target_filename.to_string(),
    })
}

// ============================================================================
// Token Economy (Lens Functions)
// ============================================================================

/// Returns a pruned AST containing only nodes at the specified paths
/// and their ancestors.
///
/// This is useful for "surgical" edits where you want to focus on specific
/// nodes without loading the entire document context.
pub fn focus_context(doc: &SExpr, paths: &[PathId]) -> SExpr {
    if paths.is_empty() {
        return SExpr::List(vec![SExpr::Atom("doc".to_string())]);
    }

    // Collect all ancestor paths that need to be included.
    let mut required_paths: std::collections::HashSet<Vec<usize>> =
        std::collections::HashSet::new();

    for path in paths {
        // Include the path itself and all ancestors.
        let indices = path.indices();
        for len in 0..=indices.len() {
            required_paths.insert(indices[..len].to_vec());
        }
    }

    // Build the pruned tree.
    focus_context_recursive(doc, &PathId::root(), &required_paths)
}

/// Recursively builds the focused tree.
fn focus_context_recursive(
    expr: &SExpr,
    path: &PathId,
    required: &std::collections::HashSet<Vec<usize>>,
) -> SExpr {
    match expr {
        SExpr::Atom(_) => expr.clone(),
        SExpr::List(items) if items.is_empty() => expr.clone(),
        SExpr::List(items) => {
            let mut new_items = vec![items[0].clone()]; // Keep the tag.

            for (i, child) in items.iter().enumerate().skip(1) {
                let child_path = path.child(i);
                if required.contains(child_path.indices()) {
                    new_items.push(focus_context_recursive(child, &child_path, required));
                }
            }

            SExpr::List(new_items)
        }
    }
}

/// Returns a skeleton of the document structure without prose content.
///
/// The skeleton shows headers, list structure, and placeholders for content.
/// This allows an agent to decide where to work based on structure before
/// requesting the full text.
pub fn skeletonize(doc: &SExpr, max_depth: Option<u8>) -> SExpr {
    skeletonize_recursive(doc, 0, max_depth.unwrap_or(u8::MAX))
}

/// Recursively builds the skeleton.
fn skeletonize_recursive(expr: &SExpr, current_depth: u8, max_depth: u8) -> SExpr {
    if current_depth > max_depth {
        return SExpr::Atom("\"...\"".to_string());
    }

    match expr {
        SExpr::Atom(s) => {
            // Replace string content with placeholder.
            if s.starts_with('"') && s.ends_with('"') {
                SExpr::Atom("\"...\"".to_string())
            } else {
                expr.clone()
            }
        }
        SExpr::List(items) if items.is_empty() => expr.clone(),
        SExpr::List(items) => {
            let tag = match &items[0] {
                SExpr::Atom(t) => t.as_str(),
                _ => return expr.clone(),
            };

            match tag {
                // Keep structure nodes but skeletonize children.
                "doc" | "ul" | "ol" | "li" | "blockquote" | "table" | "tr" | "td" | "th" => {
                    let new_items: Vec<SExpr> = items
                        .iter()
                        .enumerate()
                        .map(|(i, item)| {
                            if i == 0 {
                                item.clone()
                            } else {
                                skeletonize_recursive(item, current_depth + 1, max_depth)
                            }
                        })
                        .collect();
                    SExpr::List(new_items)
                }
                // Keep headers with their text (important for navigation).
                "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => expr.clone(),
                // Replace paragraphs with placeholder.
                "p" => SExpr::List(vec![
                    SExpr::Atom("p".to_string()),
                    SExpr::Atom("\"...\"".to_string()),
                ]),
                // Replace code blocks with language only.
                "code-block" => {
                    if items.len() >= 2 {
                        SExpr::List(vec![
                            SExpr::Atom("code-block".to_string()),
                            items[1].clone(),
                            SExpr::Atom("\"...\"".to_string()),
                        ])
                    } else {
                        expr.clone()
                    }
                }
                // Keep other structural elements.
                _ => {
                    let new_items: Vec<SExpr> = items
                        .iter()
                        .enumerate()
                        .map(|(i, item)| {
                            if i == 0 {
                                item.clone()
                            } else {
                                skeletonize_recursive(item, current_depth + 1, max_depth)
                            }
                        })
                        .collect();
                    SExpr::List(new_items)
                }
            }
        }
    }
}

/// Summary statistics about a document's structure.
#[derive(Debug, Clone, Default)]
pub struct SkeletonSummary {
    /// Count of each heading level.
    pub heading_counts: [usize; 6],
    /// Total number of paragraphs.
    pub paragraph_count: usize,
    /// Total number of code blocks.
    pub code_block_count: usize,
    /// Total number of list items.
    pub list_item_count: usize,
    /// Total number of links.
    pub link_count: usize,
    /// Total number of images.
    pub image_count: usize,
    /// Maximum nesting depth.
    pub max_depth: usize,
    /// Languages used in code blocks.
    pub code_languages: Vec<String>,
}

/// Returns summary statistics about a document's structure.
pub fn skeleton_summary(doc: &SExpr) -> SkeletonSummary {
    let mut summary = SkeletonSummary::default();
    skeleton_summary_recursive(doc, 0, &mut summary);
    summary
}

/// Recursively collects summary statistics.
fn skeleton_summary_recursive(expr: &SExpr, depth: usize, summary: &mut SkeletonSummary) {
    summary.max_depth = summary.max_depth.max(depth);

    if let SExpr::List(items) = expr {
        if items.is_empty() {
            return;
        }

        if let SExpr::Atom(tag) = &items[0] {
            match tag.as_str() {
                "h1" => summary.heading_counts[0] += 1,
                "h2" => summary.heading_counts[1] += 1,
                "h3" => summary.heading_counts[2] += 1,
                "h4" => summary.heading_counts[3] += 1,
                "h5" => summary.heading_counts[4] += 1,
                "h6" => summary.heading_counts[5] += 1,
                "p" => summary.paragraph_count += 1,
                "code-block" => {
                    summary.code_block_count += 1;
                    if items.len() >= 2 {
                        let lang = extract_string_content(&items[1]);
                        if !lang.is_empty() && !summary.code_languages.contains(&lang) {
                            summary.code_languages.push(lang);
                        }
                    }
                }
                "li" => summary.list_item_count += 1,
                "link" | "link-ref" => summary.link_count += 1,
                "img" | "img-ref" => summary.image_count += 1,
                _ => {}
            }
        }

        // Recurse into children.
        for child in items.iter().skip(1) {
            skeleton_summary_recursive(child, depth + 1, summary);
        }
    }
}

/// Converts a SkeletonSummary to an s-expression for programmatic access.
pub fn skeleton_summary_to_sexpr(summary: &SkeletonSummary) -> SExpr {
    SExpr::List(vec![
        SExpr::Atom("summary".to_string()),
        SExpr::List(vec![
            SExpr::Atom("\"headings\"".to_string()),
            SExpr::List(vec![
                SExpr::Atom("arr".to_string()),
                SExpr::Atom(summary.heading_counts[0].to_string()),
                SExpr::Atom(summary.heading_counts[1].to_string()),
                SExpr::Atom(summary.heading_counts[2].to_string()),
                SExpr::Atom(summary.heading_counts[3].to_string()),
                SExpr::Atom(summary.heading_counts[4].to_string()),
                SExpr::Atom(summary.heading_counts[5].to_string()),
            ]),
        ]),
        SExpr::List(vec![
            SExpr::Atom("\"paragraphs\"".to_string()),
            SExpr::Atom(summary.paragraph_count.to_string()),
        ]),
        SExpr::List(vec![
            SExpr::Atom("\"code-blocks\"".to_string()),
            SExpr::Atom(summary.code_block_count.to_string()),
        ]),
        SExpr::List(vec![
            SExpr::Atom("\"list-items\"".to_string()),
            SExpr::Atom(summary.list_item_count.to_string()),
        ]),
        SExpr::List(vec![
            SExpr::Atom("\"links\"".to_string()),
            SExpr::Atom(summary.link_count.to_string()),
        ]),
        SExpr::List(vec![
            SExpr::Atom("\"images\"".to_string()),
            SExpr::Atom(summary.image_count.to_string()),
        ]),
        SExpr::List(vec![
            SExpr::Atom("\"max-depth\"".to_string()),
            SExpr::Atom(summary.max_depth.to_string()),
        ]),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::s::expr::Parser;

    fn parse(s: &str) -> SExpr {
        Parser::new(s).parse().unwrap()
    }

    #[test]
    fn wrap_in_callout_basic() {
        let doc = parse(r#"(doc (h1 "Title") (p "Content to wrap"))"#);
        let result =
            wrap_in_callout(&doc, &[PathId::new(vec![2])], "warning", Some("Caution")).unwrap();
        let s = result.to_string();
        println!("DEBUG wrap_in_callout: {}", s);
        assert!(s.contains("blockquote"));
        assert!(s.contains("[!warning] Caution"));
    }

    #[test]
    fn normalize_headers_basic() {
        let doc = parse(r#"(doc (h2 "A") (h3 "B") (h4 "C"))"#);
        let result = normalize_headers(&doc, &[(2, 1), (3, 2), (4, 3)]).unwrap();
        let s = result.to_string();
        println!("DEBUG normalize_headers: {}", s);
        assert!(s.contains("(h1 \"A\")"));
        assert!(s.contains("(h2 \"B\")"));
        assert!(s.contains("(h3 \"C\")"));
    }

    #[test]
    fn extract_to_ref_basic() {
        let doc = parse(r#"(doc (h1 "Title") (p "Extract me") (p "Keep me"))"#);
        let result = extract_to_ref(
            &doc,
            &[PathId::new(vec![2])],
            "extracted.md",
            Some("See extracted content"),
        )
        .unwrap();

        println!("DEBUG source: {}", result.source_doc);
        println!("DEBUG target: {}", result.target_doc);

        // Source should have link instead of extracted content
        assert!(result.source_doc.to_string().contains("extracted.md"));
        assert!(!result.source_doc.to_string().contains("Extract me"));

        // Target should have the extracted content
        assert!(result.target_doc.to_string().contains("Extract me"));
    }

    #[test]
    fn merge_sections_append() {
        let doc = parse(r#"(doc (ul (li "A") (li "B")) (ul (li "C") (li "D")))"#);
        let result =
            merge_sections(&doc, &PathId::new(vec![2]), &PathId::new(vec![1]), "append").unwrap();
        let s = result.to_string();
        println!("DEBUG merge_sections: {}", s);
        // Target should now have A, B, C, D
        assert!(s.contains("(li \"A\")"));
        assert!(s.contains("(li \"D\")"));
    }

    #[test]
    fn generate_toc_basic() {
        let doc = parse(r#"(doc (h1 "Introduction") (p "Text") (h2 "Details") (p "More"))"#);
        let toc = generate_toc(&doc);
        let s = toc.to_string();
        println!("DEBUG toc: {}", s);
        assert!(s.contains("ul"));
        assert!(s.contains("introduction"));
        assert!(s.contains("details"));
    }

    #[test]
    fn slugify_basic() {
        assert_eq!(slugify("Hello World"), "hello-world");
        assert_eq!(slugify("API v2.0"), "api-v2_0");
        assert_eq!(slugify("  Spaces  "), "spaces");
    }

    #[test]
    fn mark_deprecated_basic() {
        let doc = parse(r#"(doc (h1 "Old API") (p "Content"))"#);
        let result = mark_deprecated(
            &doc,
            &[PathId::new(vec![1])],
            Some("Use v2 instead"),
            Some("./v2.md"),
        )
        .unwrap();
        let s = result.to_string();
        println!("DEBUG deprecated: {}", s);
        assert!(s.contains("deprecated"));
        assert!(s.contains("v2"));
    }

    // Link analysis tests

    #[test]
    fn scan_links_basic() {
        let doc =
            parse(r#"(doc (p "See " (link "https://example.com" "Example" "here") " for more."))"#);
        let links = scan_links(&doc);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].url, "https://example.com");
        assert_eq!(links[0].text, "here");
        assert!(!links[0].is_internal);
        assert!(!links[0].is_image);
    }

    #[test]
    fn scan_links_internal() {
        let doc = parse(r#"(doc (p (link "./other.md" "" "Other page")))"#);
        let links = scan_links(&doc);
        assert_eq!(links.len(), 1);
        assert!(links[0].is_internal);
    }

    #[test]
    fn scan_links_images() {
        let doc = parse(r#"(doc (p (img "image.png" "Alt text" "Title")))"#);
        let links = scan_links(&doc);
        assert_eq!(links.len(), 1);
        assert!(links[0].is_image);
        assert_eq!(links[0].text, "Alt text");
    }

    #[test]
    fn scan_links_multiple() {
        let doc = parse(
            r#"(doc 
                (p (link "https://a.com" "" "A"))
                (p (link "./local.md" "" "Local") (img "pic.jpg" "Pic" ""))
            )"#,
        );
        let links = scan_links(&doc);
        assert_eq!(links.len(), 3);
    }

    #[test]
    fn get_internal_external_links() {
        let doc = parse(
            r##"(doc 
                (p (link "https://external.com" "" "Ext"))
                (p (link "./internal.md" "" "Int"))
                (p (link "#anchor" "" "Anchor"))
            )"##,
        );
        let internal = get_internal_links(&doc);
        let external = get_external_links(&doc);

        assert_eq!(internal.len(), 2);
        assert_eq!(external.len(), 1);
    }

    #[test]
    fn update_link_basic() {
        let doc = parse(r#"(doc (p (link "old.md" "" "Link")))"#);
        // Path to the link: doc -> p -> link = [1, 1]
        let result = update_link(&doc, &PathId::new(vec![1, 1]), "new.md").unwrap();
        let s = result.to_string();
        println!("DEBUG updated link: {}", s);
        assert!(s.contains("new.md"));
        assert!(!s.contains("old.md"));
    }

    #[test]
    fn is_internal_link_tests() {
        assert!(is_internal_link("./page.md"));
        assert!(is_internal_link("../other/page.md"));
        assert!(is_internal_link("#section"));
        assert!(is_internal_link("page.md"));
        assert!(!is_internal_link("https://example.com"));
        assert!(!is_internal_link("http://example.com"));
        assert!(!is_internal_link("ftp://files.com/file"));
    }

    #[test]
    fn scan_links_to_sexpr_basic() {
        let doc = parse(r#"(doc (p (link "test.md" "" "Test")))"#);
        let result = scan_links_to_sexpr(&doc);
        let s = result.to_string();
        println!("DEBUG scan_links_to_sexpr: {}", s);
        assert!(s.contains("arr"));
        assert!(s.contains("link-info"));
        assert!(s.contains("test.md"));
    }

    // Tagging tests

    #[test]
    fn tag_node_basic() {
        let doc = parse(r#"(doc (h1 "Title") (p "Content"))"#);
        let result = tag_node(&doc, &PathId::new(vec![2]), "status", "draft").unwrap();
        let s = result.to_string();
        println!("DEBUG tagged: {}", s);
        assert!(s.contains("@tag:status=draft"));
        assert!(s.contains("html"));
    }

    #[test]
    fn get_tagged_nodes_basic() {
        let doc =
            parse(r#"(doc (h1 "Title") (html "<!-- @tag:needs-review=true -->") (p "Content"))"#);
        let tagged = get_tagged_nodes(&doc, "needs-review");
        println!("DEBUG tagged nodes: {:?}", tagged);
        assert_eq!(tagged.len(), 1);
        assert_eq!(tagged[0].path, PathId::new(vec![3]));
        assert_eq!(tagged[0].value, "true");
    }

    #[test]
    fn remove_tag_basic() {
        let doc = parse(r#"(doc (h1 "Title") (html "<!-- @tag:draft=true -->") (p "Content"))"#);
        let result = remove_tag(&doc, &PathId::new(vec![3]), "draft").unwrap();
        let s = result.to_string();
        println!("DEBUG after remove tag: {}", s);
        assert!(!s.contains("@tag:draft"));
        assert!(s.contains("(p \"Content\")"));
    }

    // Rehome orphans tests

    #[test]
    fn rehome_orphans_basic() {
        let doc = parse(
            r#"(doc (p (link "old.md" "" "Link1")) (p (link "old.md" "" "Link2")) (p (link "other.md" "" "Other")))"#,
        );
        let result = rehome_orphans(&doc, "old.md", "new.md").unwrap();
        let s = result.to_string();
        println!("DEBUG rehomed: {}", s);
        assert!(!s.contains("old.md"));
        assert!(s.contains("new.md"));
        assert!(s.contains("other.md")); // Unchanged
    }

    #[test]
    fn rehome_orphans_images() {
        let doc = parse(r#"(doc (p (img "old.png" "Alt" "")))"#);
        let result = rehome_orphans(&doc, "old.png", "new.png").unwrap();
        let s = result.to_string();
        assert!(s.contains("new.png"));
        assert!(!s.contains("old.png"));
    }

    // Extract with strategy tests

    #[test]
    fn extract_with_leave_link() {
        let doc = parse(r#"(doc (h1 "Title") (p "Extract me"))"#);
        let result = extract_with_strategy(
            &doc,
            &[PathId::new(vec![2])],
            "extracted.md",
            ExtractStrategy::LeaveLink,
            Some("See details"),
        )
        .unwrap();
        assert!(result.source_doc.to_string().contains("extracted.md"));
        assert!(result.source_doc.to_string().contains("link"));
    }

    #[test]
    fn extract_with_transclude() {
        let doc = parse(r#"(doc (h1 "Title") (p "Extract me"))"#);
        let result = extract_with_strategy(
            &doc,
            &[PathId::new(vec![2])],
            "extracted.md",
            ExtractStrategy::Transclude,
            None,
        )
        .unwrap();
        let s = result.source_doc.to_string();
        println!("DEBUG transclude: {}", s);
        assert!(s.contains("![[extracted.md]]"));
    }

    #[test]
    fn extract_with_redirect() {
        let doc = parse(r#"(doc (h1 "Title") (p "Extract me"))"#);
        let result = extract_with_strategy(
            &doc,
            &[PathId::new(vec![2])],
            "extracted.md",
            ExtractStrategy::Redirect,
            None,
        )
        .unwrap();
        let s = result.source_doc.to_string();
        println!("DEBUG redirect: {}", s);
        assert!(s.contains("Content Moved"));
        assert!(s.contains("blockquote"));
    }

    // Focus context tests

    #[test]
    fn focus_context_basic() {
        let doc = parse(r#"(doc (h1 "A") (p "B") (ul (li (p "C")) (li (p "D"))) (p "E"))"#);
        // Focus on the first list item's paragraph.
        let focused = focus_context(&doc, &[PathId::new(vec![3, 1, 1])]);
        let s = focused.to_string();
        println!("DEBUG focused: {}", s);
        // Should contain doc, ul, first li, and the p inside.
        assert!(s.contains("doc"));
        assert!(s.contains("ul"));
        assert!(s.contains("li"));
        // Should NOT contain h1, the standalone p's, or second li.
        assert!(!s.contains("(h1"));
        assert!(!s.contains("(p \"B\")"));
        assert!(!s.contains("(p \"E\")"));
    }

    #[test]
    fn focus_context_multiple_paths() {
        let doc = parse(r#"(doc (h1 "A") (p "B") (h2 "C"))"#);
        let focused = focus_context(&doc, &[PathId::new(vec![1]), PathId::new(vec![3])]);
        let s = focused.to_string();
        println!("DEBUG multi-focused: {}", s);
        // Should contain h1 and h2 but not p.
        assert!(s.contains("(h1"));
        assert!(s.contains("(h2"));
        assert!(!s.contains("(p \"B\")"));
    }

    // Skeletonize tests

    #[test]
    fn skeletonize_basic() {
        let doc = parse(
            r#"(doc (h1 "Title") (p "Long paragraph text") (code-block "rust" "fn main() {}"))"#,
        );
        let skeleton = skeletonize(&doc, None);
        let s = skeleton.to_string();
        println!("DEBUG skeleton: {}", s);
        // Headers preserved.
        assert!(s.contains("(h1 \"Title\")"));
        // Paragraphs replaced.
        assert!(s.contains("(p \"...\")"));
        // Code block language preserved but content replaced.
        assert!(s.contains("code-block"));
        assert!(s.contains("\"rust\""));
        assert!(!s.contains("fn main"));
    }

    #[test]
    fn skeletonize_with_depth() {
        let doc = parse(r#"(doc (ul (li (ul (li (p "Deep"))))))"#);
        let skeleton = skeletonize(&doc, Some(2));
        let s = skeleton.to_string();
        println!("DEBUG skeleton depth 2: {}", s);
        assert!(s.contains("\"...\""));
    }

    #[test]
    fn skeleton_summary_basic() {
        let doc = parse(
            r#"(doc 
                (h1 "Title") 
                (p "Intro") 
                (h2 "Section") 
                (p "Content") 
                (code-block "rust" "code") 
                (ul (li "A") (li "B"))
                (p (link "test.md" "" "Link"))
            )"#,
        );
        let summary = skeleton_summary(&doc);
        println!("DEBUG summary: {:?}", summary);
        assert_eq!(summary.heading_counts[0], 1); // h1
        assert_eq!(summary.heading_counts[1], 1); // h2
        assert_eq!(summary.paragraph_count, 3);
        assert_eq!(summary.code_block_count, 1);
        assert_eq!(summary.list_item_count, 2);
        assert_eq!(summary.link_count, 1);
        assert!(summary.code_languages.contains(&"rust".to_string()));
    }

    #[test]
    fn skeleton_summary_to_sexpr_basic() {
        let summary = SkeletonSummary {
            heading_counts: [1, 2, 0, 0, 0, 0],
            paragraph_count: 5,
            code_block_count: 2,
            list_item_count: 3,
            link_count: 4,
            image_count: 1,
            max_depth: 4,
            code_languages: vec!["rust".to_string()],
        };
        let sexpr = skeleton_summary_to_sexpr(&summary);
        let s = sexpr.to_string();
        eprintln!("DEBUG summary sexpr: {}", s);
        assert!(s.contains("summary"));
        assert!(s.contains("paragraphs"));
    }

    #[test]
    fn wrap_in_callout_empty_paths_error() {
        let doc = parse(r#"(doc (h1 "Title"))"#);
        let err = wrap_in_callout(&doc, &[], "warning", None).unwrap_err();
        assert_eq!(
            *err.detail(),
            SExpr::List(vec![
                SExpr::Atom("error".to_string()),
                SExpr::List(vec![
                    SExpr::Atom("phase".to_string()),
                    SExpr::Atom("curation".to_string()),
                ]),
                SExpr::List(vec![
                    SExpr::Atom("code".to_string()),
                    SExpr::Atom("empty-paths".to_string()),
                ]),
                SExpr::List(vec![
                    SExpr::Atom("message".to_string()),
                    SExpr::Atom("\"No paths provided to wrap_in_callout\"".to_string()),
                ]),
            ])
        );
    }

    #[test]
    fn wrap_in_details_empty_paths_error() {
        let doc = parse(r#"(doc (h1 "Title"))"#);
        let err = wrap_in_details(&doc, &[], "Summary").unwrap_err();
        assert_eq!(
            *err.detail(),
            SExpr::List(vec![
                SExpr::Atom("error".to_string()),
                SExpr::List(vec![
                    SExpr::Atom("phase".to_string()),
                    SExpr::Atom("curation".to_string()),
                ]),
                SExpr::List(vec![
                    SExpr::Atom("code".to_string()),
                    SExpr::Atom("empty-paths".to_string()),
                ]),
                SExpr::List(vec![
                    SExpr::Atom("message".to_string()),
                    SExpr::Atom("\"No paths provided to wrap_in_details\"".to_string()),
                ]),
            ])
        );
    }

    #[test]
    fn wrap_in_details_basic() {
        let doc = parse(r#"(doc (p "Content"))"#);
        let result = wrap_in_details(&doc, &[PathId::new(vec![1])], "Click to expand").unwrap();
        let s = result.to_string();
        eprintln!("DEBUG wrap_in_details: {}", s);
        assert!(s.contains("<details>"));
        assert!(s.contains("<summary>Click to expand</summary>"));
        assert!(s.contains("</details>"));
    }

    #[test]
    fn extract_to_ref_empty_paths_error() {
        let doc = parse(r#"(doc (h1 "Title"))"#);
        let err = extract_to_ref(&doc, &[], "file.md", None).unwrap_err();
        assert_eq!(
            *err.detail(),
            SExpr::List(vec![
                SExpr::Atom("error".to_string()),
                SExpr::List(vec![
                    SExpr::Atom("phase".to_string()),
                    SExpr::Atom("curation".to_string()),
                ]),
                SExpr::List(vec![
                    SExpr::Atom("code".to_string()),
                    SExpr::Atom("empty-paths".to_string()),
                ]),
                SExpr::List(vec![
                    SExpr::Atom("message".to_string()),
                    SExpr::Atom("\"No paths provided to extract_to_ref\"".to_string()),
                ]),
            ])
        );
    }

    #[test]
    fn merge_sections_invalid_strategy_error() {
        let doc = parse(r#"(doc (ul (li "A")) (ul (li "B")))"#);
        let err = merge_sections(
            &doc,
            &PathId::new(vec![2]),
            &PathId::new(vec![1]),
            "invalid",
        )
        .unwrap_err();
        eprintln!("DEBUG merge_sections error: {}", err.detail());
        assert_eq!(
            *err.detail(),
            SExpr::List(vec![
                SExpr::Atom("error".to_string()),
                SExpr::List(vec![
                    SExpr::Atom("phase".to_string()),
                    SExpr::Atom("curation".to_string()),
                ]),
                SExpr::List(vec![
                    SExpr::Atom("code".to_string()),
                    SExpr::Atom("invalid-strategy".to_string()),
                ]),
                SExpr::List(vec![
                    SExpr::Atom("message".to_string()),
                    SExpr::Atom(
                        "\"Strategy must be 'append', 'prepend', or 'replace'\"".to_string()
                    ),
                ]),
                SExpr::List(vec![
                    SExpr::Atom("strategy".to_string()),
                    SExpr::Atom("\"invalid\"".to_string()),
                ]),
            ])
        );
    }

    #[test]
    fn merge_sections_prepend() {
        let doc = parse(r#"(doc (ul (li "A")) (ul (li "B")))"#);
        let result = merge_sections(
            &doc,
            &PathId::new(vec![2]),
            &PathId::new(vec![1]),
            "prepend",
        )
        .unwrap();
        let s = result.to_string();
        eprintln!("DEBUG merge prepend: {}", s);
        assert!(s.contains("(li \"B\")"));
        assert!(s.contains("(li \"A\")"));
    }

    #[test]
    fn merge_sections_replace() {
        let doc = parse(r#"(doc (ul (li "A")) (ul (li "B")))"#);
        let result = merge_sections(
            &doc,
            &PathId::new(vec![2]),
            &PathId::new(vec![1]),
            "replace",
        )
        .unwrap();
        let s = result.to_string();
        eprintln!("DEBUG merge replace: {}", s);
        assert!(s.contains("(li \"B\")"));
        assert!(!s.contains("(li \"A\")"));
    }

    #[test]
    fn generate_toc_empty_doc() {
        let doc = parse(r#"(doc)"#);
        let toc = generate_toc(&doc);
        assert_eq!(toc, SExpr::List(vec![SExpr::Atom("ul".to_string())]));
    }

    #[test]
    fn scan_links_link_ref() {
        let doc = parse(r#"(doc (p (link-ref "ref1" "Link text")))"#);
        let links = scan_links(&doc);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].url, "[ref1]");
        assert!(links[0].is_internal);
    }

    #[test]
    fn scan_links_img_ref() {
        let doc = parse(r#"(doc (p (img-ref "img1" "Alt text")))"#);
        let links = scan_links(&doc);
        assert_eq!(links.len(), 1);
        assert!(links[0].is_image);
        assert!(links[0].is_internal);
    }

    #[test]
    fn scan_link_definitions_basic() {
        let doc = parse(r#"(doc (def "ref1" "https://example.com" "Example"))"#);
        let defs = scan_link_definitions(&doc);
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].identifier, "ref1");
        assert_eq!(defs[0].url, "https://example.com");
    }

    #[test]
    fn find_undefined_references_found() {
        let doc = parse(r#"(doc (p (link-ref "undefined" "Text")))"#);
        let undefined = find_undefined_references(&doc);
        assert_eq!(undefined.len(), 1);
    }

    #[test]
    fn find_undefined_references_defined() {
        let doc = parse(r#"(doc (p (link-ref "defined" "Text")) (def "defined" "url" ""))"#);
        let undefined = find_undefined_references(&doc);
        assert_eq!(undefined.len(), 0);
    }

    #[test]
    fn update_link_image() {
        let doc = parse(r#"(doc (p (img "old.png" "Alt" "")))"#);
        let result = update_link(&doc, &PathId::new(vec![1, 1]), "new.png").unwrap();
        let s = result.to_string();
        assert!(s.contains("new.png"));
        assert!(!s.contains("old.png"));
    }

    #[test]
    fn get_tagged_nodes_empty() {
        let doc = parse(r#"(doc (h1 "Title"))"#);
        let tagged = get_tagged_nodes(&doc, "status");
        assert!(tagged.is_empty());
    }

    #[test]
    fn focus_context_empty_paths() {
        let doc = parse(r#"(doc (h1 "Title"))"#);
        let focused = focus_context(&doc, &[]);
        assert_eq!(focused, SExpr::List(vec![SExpr::Atom("doc".to_string())]));
    }

    #[test]
    fn skeleton_summary_empty_doc() {
        let doc = parse(r#"(doc)"#);
        let summary = skeleton_summary(&doc);
        assert_eq!(summary.paragraph_count, 0);
        assert_eq!(summary.heading_counts, [0, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn slugify_empty() {
        assert_eq!(slugify(""), "");
    }

    #[test]
    fn get_image_links_basic() {
        let doc = parse(r#"(doc (p (img "pic.png" "Alt" "")) (p (link "page.md" "" "Link")))"#);
        let images = get_image_links(&doc);
        assert_eq!(images.len(), 1);
        assert!(images[0].is_image);
    }

    #[test]
    fn normalize_headers_no_mappings() {
        let doc = parse(r#"(doc (h1 "Title") (h2 "Sub"))"#);
        let result = normalize_headers(&doc, &[]).unwrap();
        assert_eq!(result, doc);
    }

    #[test]
    fn normalize_headers_nested() {
        let doc = parse(r#"(doc (blockquote (h3 "Nested")))"#);
        let result = normalize_headers(&doc, &[(3, 1)]).unwrap();
        let s = result.to_string();
        assert!(s.contains("(h1 \"Nested\")"));
    }
}

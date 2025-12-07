//! Text chunker for RAG applications.
//!
//! Splits markdown text into semantically meaningful chunks using the parsed AST.
//! Chunks are created at paragraph and list item boundaries, representing complete
//! units of meaning suitable for retrieval-augmented generation.
//!
//! # Example
//!
//! ```
//! use agentkb::chunker::{Chunk, chunk_markdown, ChunkerConfig};
//!
//! let text = "First paragraph.\n\nSecond paragraph.\n\n- Item one\n- Item two";
//! let chunks = chunk_markdown(text, &ChunkerConfig::default()).unwrap();
//!
//! assert_eq!(chunks.len(), 4);
//! assert_eq!(chunks[0].text, "First paragraph.");
//! ```

use crate::s::error::{SError, SResult};
use crate::s::expr::SExpr;
use crate::s::markdown::{markdown_to_sexpr, sexpr_to_markdown};

/// Configuration for the text chunker.
#[derive(Debug, Clone)]
pub struct ChunkerConfig {
    /// Maximum size of a chunk in bytes. Chunks exceeding this will be split.
    pub max_chunk_size: usize,
    /// Whether to include frontmatter as a separate chunk.
    pub include_frontmatter: bool,
}

impl Default for ChunkerConfig {
    fn default() -> Self {
        ChunkerConfig {
            max_chunk_size: 1024,
            include_frontmatter: true,
        }
    }
}

/// A chunk of text with metadata about its type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chunk {
    /// The text content of this chunk.
    pub text: String,
    /// The type of content in this chunk.
    pub chunk_type: ChunkType,
    /// Path within the document AST (e.g., "1", "2.1").
    pub path: String,
}

/// The type of content in a chunk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChunkType {
    /// A paragraph.
    Paragraph,
    /// A heading (h1-h6).
    Heading,
    /// A bullet list item.
    BulletItem,
    /// A numbered list item.
    NumberedItem,
    /// A code block.
    CodeBlock,
    /// A blockquote.
    Blockquote,
    /// Frontmatter (yaml/toml).
    Frontmatter,
    /// A thematic break.
    ThematicBreak,
    /// A table.
    Table,
    /// Unknown or other content.
    Other,
}

/// Splits markdown text into chunks at paragraph and list item boundaries.
///
/// The chunker recognizes:
/// - Paragraphs
/// - Headings (h1-h6)
/// - Bullet list items (ul > li)
/// - Numbered list items (ol > li)
/// - Code blocks
/// - Blockquotes
/// - Frontmatter (yaml/toml)
///
/// # Arguments
///
/// * `markdown` - The input markdown text to chunk
/// * `config` - Configuration for chunking behavior
///
/// # Returns
///
/// A vector of chunks representing the split text, or an error if parsing fails.
///
/// # Errors
///
/// Returns an error if the markdown cannot be parsed.
pub fn chunk_markdown(markdown: &str, config: &ChunkerConfig) -> SResult<Vec<Chunk>> {
    let sexpr = markdown_to_sexpr(markdown)?;
    chunk_sexpr(&sexpr, config)
}

/// Splits an S-expression document into chunks.
///
/// # Arguments
///
/// * `doc` - The document S-expression (must have `doc` tag)
/// * `config` - Configuration for chunking behavior
///
/// # Returns
///
/// A vector of chunks, or an error if the structure is invalid.
///
/// # Errors
///
/// Returns an error if the S-expression is not a valid document.
pub fn chunk_sexpr(doc: &SExpr, config: &ChunkerConfig) -> SResult<Vec<Chunk>> {
    let items = match doc {
        SExpr::List(items) if !items.is_empty() => items,
        _ => {
            return Err(SError::new("chunker")
                .with_code("invalid-document")
                .with_message("Expected a document S-expression"));
        }
    };

    let tag = match &items[0] {
        SExpr::Atom(s) if s == "doc" => s,
        _ => {
            return Err(SError::new("chunker")
                .with_code("invalid-document")
                .with_message("Expected document to start with 'doc' tag"));
        }
    };
    let _ = tag;

    let mut chunks = Vec::new();

    for (idx, child) in items.iter().skip(1).enumerate() {
        let path = format!("{}", idx + 1);
        collect_chunks(child, &path, config, &mut chunks)?;
    }

    // Apply size constraints
    apply_size_constraints(chunks, config)
}

/// Recursively collects chunks from an S-expression node.
fn collect_chunks(
    expr: &SExpr,
    path: &str,
    config: &ChunkerConfig,
    chunks: &mut Vec<Chunk>,
) -> SResult<()> {
    let items = match expr {
        SExpr::List(items) if !items.is_empty() => items,
        SExpr::Atom(_) => return Ok(()),
        SExpr::List(_) => return Ok(()),
    };

    let tag = match &items[0] {
        SExpr::Atom(s) => s.as_str(),
        _ => return Ok(()),
    };

    match tag {
        // Block elements that become chunks
        "p" => {
            let text = render_to_markdown(expr)?;
            let text = text.trim().to_string();
            if !text.is_empty() {
                chunks.push(Chunk {
                    text,
                    chunk_type: ChunkType::Paragraph,
                    path: path.to_string(),
                });
            }
        }

        "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
            let text = render_to_markdown(expr)?;
            let text = text.trim().to_string();
            if !text.is_empty() {
                chunks.push(Chunk {
                    text,
                    chunk_type: ChunkType::Heading,
                    path: path.to_string(),
                });
            }
        }

        "code-block" => {
            let text = render_to_markdown(expr)?;
            let text = text.trim().to_string();
            if !text.is_empty() {
                chunks.push(Chunk {
                    text,
                    chunk_type: ChunkType::CodeBlock,
                    path: path.to_string(),
                });
            }
        }

        "blockquote" => {
            let text = render_to_markdown(expr)?;
            let text = text.trim().to_string();
            if !text.is_empty() {
                chunks.push(Chunk {
                    text,
                    chunk_type: ChunkType::Blockquote,
                    path: path.to_string(),
                });
            }
        }

        "hr" => {
            chunks.push(Chunk {
                text: "---".to_string(),
                chunk_type: ChunkType::ThematicBreak,
                path: path.to_string(),
            });
        }

        "table" => {
            let text = render_to_markdown(expr)?;
            let text = text.trim().to_string();
            if !text.is_empty() {
                chunks.push(Chunk {
                    text,
                    chunk_type: ChunkType::Table,
                    path: path.to_string(),
                });
            }
        }

        "yaml" | "toml" => {
            if config.include_frontmatter {
                let text = render_to_markdown(expr)?;
                let text = text.trim().to_string();
                if !text.is_empty() {
                    chunks.push(Chunk {
                        text,
                        chunk_type: ChunkType::Frontmatter,
                        path: path.to_string(),
                    });
                }
            }
        }

        // Lists: chunk each item separately
        "ul" => {
            for (idx, child) in items.iter().skip(1).enumerate() {
                let child_path = format!("{}.{}", path, idx + 1);
                collect_list_item(child, &child_path, ChunkType::BulletItem, chunks)?;
            }
        }

        "ol" => {
            // Check if second element is a start number
            let children_start = if items.len() > 1 {
                if let SExpr::Atom(s) = &items[1] {
                    if s.parse::<u32>().is_ok() { 2 } else { 1 }
                } else {
                    1
                }
            } else {
                1
            };

            for (idx, child) in items.iter().skip(children_start).enumerate() {
                let child_path = format!("{}.{}", path, idx + 1);
                collect_list_item(child, &child_path, ChunkType::NumberedItem, chunks)?;
            }
        }

        // Other elements: try to render them
        _ => {
            let text = render_to_markdown(expr)?;
            let text = text.trim().to_string();
            if !text.is_empty() {
                chunks.push(Chunk {
                    text,
                    chunk_type: ChunkType::Other,
                    path: path.to_string(),
                });
            }
        }
    }

    Ok(())
}

/// Collects a list item as a chunk.
fn collect_list_item(
    expr: &SExpr,
    path: &str,
    chunk_type: ChunkType,
    chunks: &mut Vec<Chunk>,
) -> SResult<()> {
    let items = match expr {
        SExpr::List(items) if !items.is_empty() => items,
        _ => return Ok(()),
    };

    let tag = match &items[0] {
        SExpr::Atom(s) => s.as_str(),
        _ => return Ok(()),
    };

    if tag != "li" {
        return Ok(());
    }

    // Render just the content of this list item
    let text = render_list_item_content(expr)?;
    let text = text.trim().to_string();
    if !text.is_empty() {
        chunks.push(Chunk {
            text,
            chunk_type,
            path: path.to_string(),
        });
    }

    Ok(())
}

/// Renders list item content to text.
fn render_list_item_content(li: &SExpr) -> SResult<String> {
    let items = match li {
        SExpr::List(items) if items.len() > 1 => items,
        _ => return Ok(String::new()),
    };

    // Build a temporary doc with just this list item's content
    let mut doc_items = vec![SExpr::Atom("doc".to_string())];
    for child in items.iter().skip(1) {
        doc_items.push(child.clone());
    }
    let temp_doc = SExpr::List(doc_items);

    let text = sexpr_to_markdown(&temp_doc)?;
    Ok(text.trim().to_string())
}

/// Renders an S-expression to markdown text.
fn render_to_markdown(expr: &SExpr) -> SResult<String> {
    // Wrap in a doc for rendering
    let doc = SExpr::List(vec![SExpr::Atom("doc".to_string()), expr.clone()]);
    sexpr_to_markdown(&doc)
}

/// Applies size constraints by splitting large chunks.
fn apply_size_constraints(chunks: Vec<Chunk>, config: &ChunkerConfig) -> SResult<Vec<Chunk>> {
    let mut result = Vec::new();

    for chunk in chunks {
        if chunk.text.len() <= config.max_chunk_size {
            result.push(chunk);
        } else {
            let split_chunks = split_large_chunk(&chunk, config);
            result.extend(split_chunks);
        }
    }

    Ok(result)
}

/// Splits a chunk that exceeds the maximum size.
fn split_large_chunk(chunk: &Chunk, config: &ChunkerConfig) -> Vec<Chunk> {
    let mut result = Vec::new();
    let text = &chunk.text;
    let mut current_start = 0;
    let mut part_num = 0;

    while current_start < text.len() {
        let remaining = &text[current_start..];
        if remaining.len() <= config.max_chunk_size {
            result.push(Chunk {
                text: remaining.to_string(),
                chunk_type: chunk.chunk_type,
                path: if part_num == 0 {
                    chunk.path.clone()
                } else {
                    format!("{}#{}", chunk.path, part_num)
                },
            });
            break;
        }

        let max_end = current_start + config.max_chunk_size;
        let split_point = find_split_point(text, current_start, max_end);

        let chunk_text = text[current_start..split_point].trim_end().to_string();
        if !chunk_text.is_empty() {
            result.push(Chunk {
                text: chunk_text,
                chunk_type: chunk.chunk_type,
                path: if part_num == 0 {
                    chunk.path.clone()
                } else {
                    format!("{}#{}", chunk.path, part_num)
                },
            });
            part_num += 1;
        }

        current_start = split_point;
        while current_start < text.len() && text[current_start..].starts_with(char::is_whitespace) {
            current_start += text[current_start..].chars().next().unwrap().len_utf8();
        }
    }

    result
}

/// Finds a good split point within the given range.
fn find_split_point(text: &str, start: usize, max_end: usize) -> usize {
    let search_range = &text[start..max_end.min(text.len())];

    // Try to split at sentence boundaries (. ! ?)
    if let Some(pos) = search_range.rfind(['.', '!', '?']) {
        let split_pos = start + pos + 1;
        if split_pos > start + (max_end - start) / 2 {
            return split_pos;
        }
    }

    // Try to split at newlines
    if let Some(pos) = search_range.rfind('\n') {
        return start + pos + 1;
    }

    // Try to split at word boundaries (space)
    if let Some(pos) = search_range.rfind(' ') {
        return start + pos;
    }

    // Fall back to hard split
    max_end.min(text.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_document() {
        let chunks = chunk_markdown("", &ChunkerConfig::default()).unwrap();
        assert!(chunks.is_empty(), "Empty document should produce no chunks");
        println!("empty_document: {:?}", chunks);
    }

    #[test]
    fn single_paragraph() {
        let chunks = chunk_markdown("Hello, world!", &ChunkerConfig::default()).unwrap();
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].text, "Hello, world!");
        assert_eq!(chunks[0].chunk_type, ChunkType::Paragraph);
        assert_eq!(chunks[0].path, "1");
        println!("single_paragraph: {:?}", chunks);
    }

    #[test]
    fn two_paragraphs() {
        let text = "First paragraph.\n\nSecond paragraph.";
        let chunks = chunk_markdown(text, &ChunkerConfig::default()).unwrap();
        assert_eq!(chunks.len(), 2, "chunks: {:?}", chunks);
        assert_eq!(chunks[0].text, "First paragraph.");
        assert_eq!(chunks[0].chunk_type, ChunkType::Paragraph);
        assert_eq!(chunks[1].text, "Second paragraph.");
        assert_eq!(chunks[1].chunk_type, ChunkType::Paragraph);
        println!("two_paragraphs: {:?}", chunks);
    }

    #[test]
    fn heading() {
        let text = "# My Title";
        let chunks = chunk_markdown(text, &ChunkerConfig::default()).unwrap();
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].text, "# My Title");
        assert_eq!(chunks[0].chunk_type, ChunkType::Heading);
        println!("heading: {:?}", chunks);
    }

    #[test]
    fn bullet_list() {
        let text = "- Item one\n- Item two\n- Item three";
        let chunks = chunk_markdown(text, &ChunkerConfig::default()).unwrap();
        assert_eq!(chunks.len(), 3, "chunks: {:?}", chunks);
        assert_eq!(chunks[0].text, "Item one");
        assert_eq!(chunks[0].chunk_type, ChunkType::BulletItem);
        assert_eq!(chunks[0].path, "1.1");
        assert_eq!(chunks[1].text, "Item two");
        assert_eq!(chunks[2].text, "Item three");
        println!("bullet_list: {:?}", chunks);
    }

    #[test]
    fn numbered_list() {
        let text = "1. First item\n2. Second item\n3. Third item";
        let chunks = chunk_markdown(text, &ChunkerConfig::default()).unwrap();
        assert_eq!(chunks.len(), 3, "chunks: {:?}", chunks);
        assert_eq!(chunks[0].text, "First item");
        assert_eq!(chunks[0].chunk_type, ChunkType::NumberedItem);
        assert_eq!(chunks[1].text, "Second item");
        assert_eq!(chunks[2].text, "Third item");
        println!("numbered_list: {:?}", chunks);
    }

    #[test]
    fn mixed_content() {
        let text =
            "# Introduction\n\nA paragraph here.\n\n- Bullet one\n- Bullet two\n\nConclusion.";
        let chunks = chunk_markdown(text, &ChunkerConfig::default()).unwrap();
        assert_eq!(chunks.len(), 5, "chunks: {:?}", chunks);
        assert_eq!(chunks[0].chunk_type, ChunkType::Heading);
        assert_eq!(chunks[1].chunk_type, ChunkType::Paragraph);
        assert_eq!(chunks[2].chunk_type, ChunkType::BulletItem);
        assert_eq!(chunks[3].chunk_type, ChunkType::BulletItem);
        assert_eq!(chunks[4].chunk_type, ChunkType::Paragraph);
        println!("mixed_content: {:?}", chunks);
    }

    #[test]
    fn code_block() {
        let text = "```rust\nfn main() {}\n```";
        let chunks = chunk_markdown(text, &ChunkerConfig::default()).unwrap();
        assert_eq!(chunks.len(), 1, "chunks: {:?}", chunks);
        assert_eq!(chunks[0].chunk_type, ChunkType::CodeBlock);
        assert!(chunks[0].text.contains("fn main()"));
        println!("code_block: {:?}", chunks);
    }

    #[test]
    fn blockquote() {
        let text = "> This is a quote.";
        let chunks = chunk_markdown(text, &ChunkerConfig::default()).unwrap();
        assert_eq!(chunks.len(), 1, "chunks: {:?}", chunks);
        assert_eq!(chunks[0].chunk_type, ChunkType::Blockquote);
        assert!(chunks[0].text.contains("> This is a quote."));
        println!("blockquote: {:?}", chunks);
    }

    #[test]
    fn thematic_break() {
        let text = "Above\n\n---\n\nBelow";
        let chunks = chunk_markdown(text, &ChunkerConfig::default()).unwrap();
        assert_eq!(chunks.len(), 3, "chunks: {:?}", chunks);
        assert_eq!(chunks[1].chunk_type, ChunkType::ThematicBreak);
        assert_eq!(chunks[1].text, "---");
        println!("thematic_break: {:?}", chunks);
    }

    #[test]
    fn frontmatter_yaml() {
        let doc = crate::Parser::new(r#"(doc (yaml "title: Hello") (h1 "Title"))"#)
            .parse()
            .unwrap();
        let chunks = chunk_sexpr(&doc, &ChunkerConfig::default()).unwrap();
        assert_eq!(chunks.len(), 2, "chunks: {:?}", chunks);
        assert_eq!(chunks[0].chunk_type, ChunkType::Frontmatter);
        assert!(chunks[0].text.contains("title: Hello"));
        assert_eq!(chunks[1].chunk_type, ChunkType::Heading);
        println!("frontmatter_yaml: {:?}", chunks);
    }

    #[test]
    fn frontmatter_excluded() {
        let doc = crate::Parser::new(r#"(doc (yaml "title: Hello") (h1 "Title"))"#)
            .parse()
            .unwrap();
        let config = ChunkerConfig {
            include_frontmatter: false,
            ..Default::default()
        };
        let chunks = chunk_sexpr(&doc, &config).unwrap();
        assert_eq!(chunks.len(), 1, "chunks: {:?}", chunks);
        assert_eq!(chunks[0].chunk_type, ChunkType::Heading);
        println!("frontmatter_excluded: {:?}", chunks);
    }

    #[test]
    fn large_paragraph_splitting() {
        let text = "This is a sentence. ".repeat(100);
        let config = ChunkerConfig {
            max_chunk_size: 100,
            ..Default::default()
        };
        let chunks = chunk_markdown(&text, &config).unwrap();
        assert!(
            chunks.len() > 1,
            "Large text should be split into multiple chunks"
        );
        for chunk in &chunks {
            assert!(
                chunk.text.len() <= config.max_chunk_size,
                "Chunk exceeds max size: {} > {}",
                chunk.text.len(),
                config.max_chunk_size
            );
        }
        println!("large_paragraph_splitting: {} chunks", chunks.len());
    }

    #[test]
    fn path_tracking() {
        let text = "# Title\n\nPara 1\n\nPara 2";
        let chunks = chunk_markdown(text, &ChunkerConfig::default()).unwrap();
        assert_eq!(chunks.len(), 3, "chunks: {:?}", chunks);
        assert_eq!(chunks[0].path, "1");
        assert_eq!(chunks[1].path, "2");
        assert_eq!(chunks[2].path, "3");
        println!("path_tracking: {:?}", chunks);
    }

    #[test]
    fn nested_list_path() {
        let text = "- Item 1\n- Item 2";
        let chunks = chunk_markdown(text, &ChunkerConfig::default()).unwrap();
        assert_eq!(chunks.len(), 2, "chunks: {:?}", chunks);
        assert_eq!(chunks[0].path, "1.1");
        assert_eq!(chunks[1].path, "1.2");
        println!("nested_list_path: {:?}", chunks);
    }

    #[test]
    fn invalid_document_atom() {
        let doc = SExpr::Atom("not-a-doc".to_string());
        let err = chunk_sexpr(&doc, &ChunkerConfig::default()).unwrap_err();
        assert!(err.to_string().contains("invalid-document"));
        println!("invalid_document_atom: error as expected");
    }

    #[test]
    fn invalid_document_wrong_tag() {
        let doc = SExpr::List(vec![
            SExpr::Atom("not-doc".to_string()),
            SExpr::Atom("content".to_string()),
        ]);
        let err = chunk_sexpr(&doc, &ChunkerConfig::default()).unwrap_err();
        assert!(err.to_string().contains("invalid-document"));
        println!("invalid_document_wrong_tag: error as expected");
    }

    #[test]
    fn chunk_type_display() {
        assert_eq!(format!("{:?}", ChunkType::Paragraph), "Paragraph");
        assert_eq!(format!("{:?}", ChunkType::BulletItem), "BulletItem");
        assert_eq!(format!("{:?}", ChunkType::NumberedItem), "NumberedItem");
        println!("chunk_type_display: passed");
    }

    #[test]
    fn config_default() {
        let config = ChunkerConfig::default();
        assert_eq!(config.max_chunk_size, 1024);
        assert!(config.include_frontmatter);
        println!("config_default: {:?}", config);
    }

    #[test]
    fn split_path_annotation() {
        let text =
            "This is a very long sentence that will definitely need to be split. ".repeat(50);
        let config = ChunkerConfig {
            max_chunk_size: 100,
            ..Default::default()
        };
        let chunks = chunk_markdown(&text, &config).unwrap();
        assert!(chunks.len() > 2);
        assert_eq!(chunks[0].path, "1");
        assert!(chunks[1].path.starts_with("1#"));
        println!(
            "split_path_annotation: {} chunks, paths: {:?}",
            chunks.len(),
            chunks.iter().map(|c| &c.path).collect::<Vec<_>>()
        );
    }

    #[test]
    fn emphasis_in_paragraph() {
        let text = "This is *emphasized* and **strong** text.";
        let chunks = chunk_markdown(text, &ChunkerConfig::default()).unwrap();
        assert_eq!(chunks.len(), 1);
        assert!(chunks[0].text.contains("*emphasized*"));
        assert!(chunks[0].text.contains("**strong**"));
        println!("emphasis_in_paragraph: {:?}", chunks);
    }

    #[test]
    fn link_in_paragraph() {
        let text = "Click [here](https://example.com) for more.";
        let chunks = chunk_markdown(text, &ChunkerConfig::default()).unwrap();
        assert_eq!(chunks.len(), 1);
        assert!(chunks[0].text.contains("[here](https://example.com)"));
        println!("link_in_paragraph: {:?}", chunks);
    }

    #[test]
    fn inline_code_in_paragraph() {
        let text = "Use the `println!` macro.";
        let chunks = chunk_markdown(text, &ChunkerConfig::default()).unwrap();
        assert_eq!(chunks.len(), 1);
        assert!(chunks[0].text.contains("`println!`"));
        println!("inline_code_in_paragraph: {:?}", chunks);
    }

    #[test]
    fn multiple_headings() {
        let text = "# H1\n\n## H2\n\n### H3";
        let chunks = chunk_markdown(text, &ChunkerConfig::default()).unwrap();
        assert_eq!(chunks.len(), 3, "chunks: {:?}", chunks);
        assert!(chunks[0].text.contains("# H1"));
        assert!(chunks[1].text.contains("## H2"));
        assert!(chunks[2].text.contains("### H3"));
        println!("multiple_headings: {:?}", chunks);
    }
}

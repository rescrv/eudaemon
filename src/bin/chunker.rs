//! Chunker CLI tool for splitting markdown into JSONL chunks.
//!
//! Reads markdown from stdin or files and outputs JSONL chunks suitable for RAG applications.

use std::fs;
use std::io::{self, BufRead, Write};
use std::path::PathBuf;

use agentkb::chunker::{Chunk, ChunkType, ChunkerConfig, chunk_markdown};

use arrrg::CommandLine;

/// Command-line arguments for the chunker tool.
#[derive(arrrg_derive::CommandLine, Debug, Default, Eq, PartialEq)]
struct ChunkerArgs {
    /// Maximum chunk size in bytes.
    #[arrrg(optional, "Maximum chunk size in bytes (default: 1024)")]
    max_chunk_size: Option<usize>,
    /// Exclude frontmatter from output.
    #[arrrg(flag, "Exclude frontmatter chunks from output")]
    exclude_frontmatter: bool,
}

/// JSON-serializable chunk output.
#[derive(serde::Serialize)]
struct ChunkOutput {
    text: String,
    chunk_type: String,
    path: String,
    source: String,
}

impl ChunkOutput {
    fn from_chunk(chunk: Chunk, source: String) -> Self {
        ChunkOutput {
            text: chunk.text,
            chunk_type: chunk_type_to_string(chunk.chunk_type),
            path: chunk.path,
            source,
        }
    }
}

fn chunk_type_to_string(ct: ChunkType) -> String {
    match ct {
        ChunkType::Paragraph => "paragraph",
        ChunkType::Heading => "heading",
        ChunkType::BulletItem => "bullet_item",
        ChunkType::NumberedItem => "numbered_item",
        ChunkType::CodeBlock => "code_block",
        ChunkType::Blockquote => "blockquote",
        ChunkType::Frontmatter => "frontmatter",
        ChunkType::ThematicBreak => "thematic_break",
        ChunkType::Table => "table",
        ChunkType::Other => "other",
    }
    .to_string()
}

fn process_markdown(
    markdown: &str,
    config: &ChunkerConfig,
    source: &str,
    output: &mut impl Write,
) -> Result<(), Box<dyn std::error::Error>> {
    let chunks = chunk_markdown(markdown, config)?;

    for chunk in chunks {
        let output_chunk = ChunkOutput::from_chunk(chunk, source.to_string());
        let json = serde_json::to_string(&output_chunk)?;
        writeln!(output, "{}", json)?;
    }

    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let (args, free) = ChunkerArgs::from_command_line_relaxed(
        "Usage: chunker [OPTIONS] [FILES...]\n\nChunks markdown into JSONL for RAG applications.\nReads from stdin if no files provided.",
    );

    let config = ChunkerConfig {
        max_chunk_size: args.max_chunk_size.unwrap_or(1024),
        include_frontmatter: !args.exclude_frontmatter,
    };

    let mut stdout = io::stdout().lock();

    if free.is_empty() {
        let stdin = io::stdin().lock();
        let markdown: String = stdin.lines().collect::<Result<Vec<_>, _>>()?.join("\n");
        process_markdown(&markdown, &config, "<stdin>", &mut stdout)?;
    } else {
        for file_path in &free {
            let path = PathBuf::from(file_path);
            let markdown = fs::read_to_string(&path)?;
            process_markdown(&markdown, &config, file_path, &mut stdout)?;
        }
    }

    Ok(())
}

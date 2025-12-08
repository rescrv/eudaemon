//! Edit tool for applying transforms to markdown documents in place.
//!
//! This module provides a tool that allows the agent to edit markdown documents
//! by applying s-expression transforms. The document is read from disk, transformed,
//! and written back to disk.

use std::any::Any;
use std::fs;
use std::ops::ControlFlow;

use async_trait::async_trait;
use claudius::{
    Agent, Anthropic, IntermediateToolResult, Tool, ToolCallback, ToolParam, ToolResult,
    ToolResultBlock, ToolResultBlockContent, ToolUnionParam, ToolUseBlock,
};
use serde_json::json;
use utf8path::Path;

use crate::markdown_to_sexpr;
use crate::s::eval::{Env, eval, register_builtins};
use crate::s::expr::{Parser, SExpr};
use crate::s::markdown::sexpr_to_markdown;
use crate::s::repl::register_markdown_builtins;
use crate::s::util::find_markdown_files;

/// A tool for editing markdown documents by applying s-expression transforms.
///
/// The `ToolEdit` provides the agent with the ability to modify documents in place
/// by applying transforms. The document is read from disk, transformed, and written
/// back to disk.
pub struct ToolEdit {
    filesystem: Path<'static>,
}

impl ToolEdit {
    /// Creates a new edit tool rooted at the given filesystem path.
    pub fn new(filesystem: Path<'static>) -> Self {
        Self { filesystem }
    }

    /// Applies a transform to a document and writes it back to disk.
    ///
    /// The transform is an s-expression that takes the document as its first argument
    /// (thread-first style). For example, `(prune "1.2")` becomes `(prune doc "1.2")`.
    ///
    /// # Arguments
    ///
    /// * `filename` - The name of the document to edit (relative to filesystem root).
    /// * `transform` - The s-expression transform to apply.
    ///
    /// # Returns
    ///
    /// Returns Ok with a success message, or an error message if the operation fails.
    pub fn edit(&self, filename: &str, transform: &str) -> Result<String, String> {
        // Read the document from disk
        let full_path = self.filesystem.join(filename);
        let content = fs::read_to_string(full_path.as_str())
            .map_err(|e| format!("Failed to read {}: {}", filename, e))?;

        let doc = markdown_to_sexpr(&content)
            .map_err(|e| format!("Failed to parse {}: {}", filename, e))?;

        // Parse the transform expression
        let mut parser = Parser::new(transform);
        let transform_expr = parser.parse().map_err(|e| e.to_string())?;

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
            SExpr::Atom(func_name) => SExpr::List(vec![
                SExpr::Atom(func_name),
                SExpr::List(vec![SExpr::Atom("quote".to_string()), doc.clone()]),
            ]),
            _ => {
                return Err("Transform must be a function call or function name".to_string());
            }
        };

        // Set up evaluation environment
        let mut env = Env::new();
        register_builtins(&mut env);
        register_markdown_builtins(&mut env);

        // Load and bind all markdown documents from filesystem
        let doc_paths = find_markdown_files(std::path::Path::new(self.filesystem.as_str()));
        for path in &doc_paths {
            let path_full = self.filesystem.join(path);
            if let Ok(c) = fs::read_to_string(path_full.as_str())
                && let Ok(sexpr) = markdown_to_sexpr(&c)
            {
                let path_ref = Path::new(path);
                let name = path_ref.basename().to_string();
                env.bind(&name, sexpr);
            }
        }

        // Evaluate the transform
        let result = eval(&expr, &env).map_err(|e| e.to_string())?;

        // Convert back to markdown and write to disk
        let markdown = sexpr_to_markdown(&result).map_err(|e| e.to_string())?;
        fs::write(full_path.as_str(), markdown)
            .map_err(|e| format!("Failed to write {}: {}", filename, e))?;

        Ok(format!("Successfully edited {}", filename))
    }

    /// Returns a list of available document names from the filesystem.
    pub fn available_documents(&self) -> Vec<String> {
        let doc_paths = find_markdown_files(std::path::Path::new(self.filesystem.as_str()));
        doc_paths
            .iter()
            .map(|p| Path::new(p).basename().to_string())
            .collect()
    }
}

/// Unit intermediate result for edit tool.
struct EditUnit;

impl IntermediateToolResult for EditUnit {
    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Callback implementation for the edit tool.
struct EditCallback {
    filesystem: Path<'static>,
}

#[async_trait]
impl<A: Agent> ToolCallback<A> for EditCallback {
    async fn compute_tool_result(
        &self,
        _client: &Anthropic,
        _agent: &A,
        _tool_use: &ToolUseBlock,
    ) -> Box<dyn IntermediateToolResult> {
        Box::new(EditUnit)
    }

    async fn apply_tool_result(
        &self,
        _client: &Anthropic,
        _agent: &mut A,
        tool_use: &ToolUseBlock,
        _intermediate: Box<dyn IntermediateToolResult>,
    ) -> ToolResult {
        let filename = tool_use
            .input
            .get("filename")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        let transform = tool_use
            .input
            .get("transform")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        let tool = ToolEdit::new(self.filesystem.clone());

        match tool.edit(filename, transform) {
            Ok(result) => ControlFlow::Continue(Ok(ToolResultBlock {
                tool_use_id: tool_use.id.clone(),
                cache_control: None,
                content: Some(ToolResultBlockContent::String(result)),
                is_error: None,
            })),
            Err(e) => ControlFlow::Continue(Err(ToolResultBlock {
                tool_use_id: tool_use.id.clone(),
                cache_control: None,
                content: Some(ToolResultBlockContent::String(format!("Error: {}", e))),
                is_error: Some(true),
            })),
        }
    }
}

impl<A: Agent> Tool<A> for ToolEdit {
    fn name(&self) -> String {
        "edit".to_string()
    }

    fn callback(&self) -> Box<dyn ToolCallback<A> + '_> {
        Box::new(EditCallback {
            filesystem: self.filesystem.clone(),
        })
    }

    fn to_param(&self) -> ToolUnionParam {
        let doc_list = self.available_documents();
        let doc_description = if doc_list.is_empty() {
            "No markdown files found in filesystem.".to_string()
        } else {
            format!("Available documents: {}", doc_list.join(", "))
        };

        let tool = ToolParam::new(
            "edit".to_string(),
            json!({
                "type": "object",
                "properties": {
                    "filename": {
                        "type": "string",
                        "description": "The path to the document to edit (relative to filesystem root)."
                    },
                    "transform": {
                        "type": "string",
                        "description": "The s-expression transform to apply. The document is inserted as the first argument."
                    }
                },
                "required": ["filename", "transform"]
            }),
        )
        .with_description(format!(
            "Edit a markdown document by applying an s-expression transform. \
             Reads the document from disk, applies the transform, and writes back to disk. \
             The transform takes the document as its first argument (thread-first style). \
             For example, (prune \"1.2\") becomes (prune doc \"1.2\"). \
             {}\n\n\
             Examples:\n\
             - (prune \"1.2\") - Remove node at path 1.2\n\
             - (replace-at \"1\" (h1 \"New Title\")) - Replace first node\n\
             - (hoist \"2\" -1) - Decrease heading level at path 2",
            doc_description
        ));

        ToolUnionParam::CustomTool(tool)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    fn test_filesystem() -> Path<'static> {
        let dir = env::current_dir().unwrap();
        Path::new(dir.to_str().unwrap()).into_owned()
    }

    #[test]
    fn edit_prune_node() {
        // Create a temporary test file
        let test_file = "test_edit_prune.md";
        let content = "# Title\n\n## Section\n\nParagraph";
        fs::write(test_file, content).unwrap();

        let tool = ToolEdit::new(test_filesystem());

        // Prune the paragraph at path "3"
        let result = tool.edit(test_file, "(prune \"3\")");
        assert!(result.is_ok(), "Edit should succeed: {:?}", result);

        // Read back and verify
        let edited_content = fs::read_to_string(test_file).unwrap();
        assert!(
            !edited_content.contains("Paragraph"),
            "Paragraph should be removed: {}",
            edited_content
        );
        println!("DEBUG: edited document: {}", edited_content);

        // Cleanup
        fs::remove_file(test_file).unwrap();
    }

    #[test]
    fn edit_document_not_found() {
        let tool = ToolEdit::new(test_filesystem());

        let result = tool.edit("nonexistent_test_file.md", "(prune \"1\")");
        assert!(result.is_err());
        assert!(
            result.as_ref().unwrap_err().contains("Failed to read"),
            "Error should mention read failure: {:?}",
            result
        );
        println!(
            "DEBUG: expected error for nonexistent document: {:?}",
            result
        );
    }

    #[test]
    fn edit_invalid_transform() {
        // Create a temporary test file
        let test_file = "test_edit_invalid.md";
        fs::write(test_file, "# Title").unwrap();

        let tool = ToolEdit::new(test_filesystem());

        let result = tool.edit(test_file, "(undefined-function \"1\")");
        assert!(result.is_err());
        println!(
            "DEBUG: expected error for invalid transform: {:?}",
            result.unwrap_err()
        );

        // Cleanup
        fs::remove_file(test_file).unwrap();
    }

    #[test]
    fn edit_with_single_function() {
        // Create a temporary test file
        let test_file = "test_edit_single_fn.md";
        fs::write(test_file, "# Title\n\n## Section").unwrap();

        let tool = ToolEdit::new(test_filesystem());

        let result = tool.edit(test_file, "generate-toc");
        assert!(result.is_ok(), "Edit with single function should succeed");

        // Read back and verify
        let edited_content = fs::read_to_string(test_file).unwrap();
        // generate-toc produces a ul list
        println!("DEBUG: TOC generated: {}", edited_content);

        // Cleanup
        fs::remove_file(test_file).unwrap();
    }
}

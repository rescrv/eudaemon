//! Edit tool for applying transforms to markdown documents in place.
//!
//! This module provides a tool that allows the agent to edit markdown documents
//! by applying s-expression transforms. The document is modified in place and
//! the updated version is stored in the agent's document map.

use std::any::Any;
use std::collections::HashMap;
use std::ops::ControlFlow;
use std::sync::{Arc, RwLock};

use async_trait::async_trait;
use claudius::{
    Agent, Anthropic, IntermediateToolResult, Tool, ToolCallback, ToolParam, ToolResult,
    ToolResultBlock, ToolResultBlockContent, ToolUnionParam, ToolUseBlock,
};
use serde_json::json;

use crate::s::eval::{Env, eval, register_builtins};
use crate::s::expr::{Parser, SExpr};
use crate::s::repl::register_markdown_builtins;

/// A tool for editing markdown documents by applying s-expression transforms.
///
/// The `ToolEdit` provides the agent with the ability to modify documents in place
/// by applying transforms. The transform expression takes the document as its last
/// argument (thread-last style).
pub struct ToolEdit {
    documents: Arc<RwLock<HashMap<String, SExpr>>>,
}

impl ToolEdit {
    /// Creates a new edit tool with the given mutable documents map.
    pub fn new(documents: Arc<RwLock<HashMap<String, SExpr>>>) -> Self {
        Self { documents }
    }

    /// Applies a transform to a document and updates it in place.
    ///
    /// The transform is an s-expression that takes the document as its first argument
    /// (thread-first style). For example, `(prune "1.2")` becomes `(prune doc "1.2")`.
    ///
    /// # Arguments
    ///
    /// * `filename` - The name of the document to edit.
    /// * `transform` - The s-expression transform to apply.
    ///
    /// # Returns
    ///
    /// Returns Ok with a success message, or an error message if the operation fails.
    pub fn edit(&self, filename: &str, transform: &str) -> Result<String, String> {
        let mut docs = self
            .documents
            .write()
            .map_err(|e| format!("Failed to acquire write lock: {}", e))?;

        let doc = docs
            .get(filename)
            .ok_or_else(|| format!("Document not loaded: {}", filename))?
            .clone();

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

        // Bind all documents as variables
        for (name, d) in docs.iter() {
            env.bind(name, d.clone());
        }

        // Evaluate the transform
        let result = eval(&expr, &env).map_err(|e| e.to_string())?;

        // Update the document in place
        docs.insert(filename.to_string(), result);

        Ok(format!("Successfully edited {}", filename))
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
    documents: Arc<RwLock<HashMap<String, SExpr>>>,
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

        let tool = ToolEdit::new(Arc::clone(&self.documents));

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
            documents: Arc::clone(&self.documents),
        })
    }

    fn to_param(&self) -> ToolUnionParam {
        let docs = self.documents.read().ok();
        let doc_list: Vec<String> = docs
            .map(|d| d.keys().cloned().collect())
            .unwrap_or_default();
        let doc_description = if doc_list.is_empty() {
            "No documents currently loaded.".to_string()
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
                        "description": "The name of the document to edit (must be loaded)."
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
            "Edit a markdown document in place by applying an s-expression transform. \
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
    use crate::markdown_to_sexpr;

    #[test]
    fn edit_prune_node() {
        let mut docs = HashMap::new();
        // Document structure: (doc (h1 "Title") (h2 "Section") (p "Paragraph"))
        // Paths: root, 1 (h1), 2 (h2), 3 (p)
        let doc = markdown_to_sexpr("# Title\n\n## Section\n\nParagraph").unwrap();
        docs.insert("test.md".to_string(), doc);

        let documents = Arc::new(RwLock::new(docs));
        let tool = ToolEdit::new(Arc::clone(&documents));

        // Prune the paragraph at path "3"
        let result = tool.edit("test.md", "(prune \"3\")");
        assert!(result.is_ok(), "Edit should succeed: {:?}", result);

        let docs = documents.read().unwrap();
        let edited = docs.get("test.md").unwrap();
        let edited_str = edited.to_string();
        assert!(
            !edited_str.contains("Paragraph"),
            "Paragraph should be removed: {}",
            edited_str
        );
        println!("DEBUG: edited document: {}", edited_str);
    }

    #[test]
    fn edit_document_not_found() {
        let docs = HashMap::new();
        let documents = Arc::new(RwLock::new(docs));
        let tool = ToolEdit::new(documents);

        let result = tool.edit("nonexistent.md", "(prune \"1\")");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not loaded"));
        println!("DEBUG: expected error for nonexistent document");
    }

    #[test]
    fn edit_invalid_transform() {
        let mut docs = HashMap::new();
        let doc = markdown_to_sexpr("# Title").unwrap();
        docs.insert("test.md".to_string(), doc);

        let documents = Arc::new(RwLock::new(docs));
        let tool = ToolEdit::new(documents);

        let result = tool.edit("test.md", "(undefined-function \"1\")");
        assert!(result.is_err());
        println!(
            "DEBUG: expected error for invalid transform: {:?}",
            result.unwrap_err()
        );
    }

    #[test]
    fn edit_with_single_function() {
        let mut docs = HashMap::new();
        let doc = markdown_to_sexpr("# Title\n\n## Section").unwrap();
        docs.insert("test.md".to_string(), doc);

        let documents = Arc::new(RwLock::new(docs));
        let tool = ToolEdit::new(Arc::clone(&documents));

        let result = tool.edit("test.md", "generate-toc");
        assert!(result.is_ok(), "Edit with single function should succeed");

        let docs = documents.read().unwrap();
        let edited = docs.get("test.md").unwrap();
        let edited_str = edited.to_string();
        assert!(
            edited_str.contains("ul"),
            "TOC should be generated: {}",
            edited_str
        );
        println!("DEBUG: TOC generated: {}", edited_str);
    }
}

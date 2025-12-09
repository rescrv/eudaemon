//! S-expression evaluation tool for the AgentKB agent.
//!
//! This module provides a tool that allows the agent to evaluate s-expressions against
//! markdown documents. Documents are read from the filesystem on each evaluation,
//! enabling programmatic manipulation of the markdown wiki.
//!
//! # Example
//!
//! If documents `readme.md` and `guide.md` exist in the filesystem root, the agent can
//! evaluate expressions like:
//!
//! ```text
//! (generate-toc readme.md)
//! (get-frontmatter guide.md)
//! (->> readme.md (annotate) (get-by-path "1"))
//! ```

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
use crate::s::eval::{Env, register_builtins};
use crate::s::expr::Parser;
use crate::s::repl::register_markdown_builtins;
use crate::s::util::find_markdown_files;

/// A tool for evaluating s-expressions against markdown documents.
///
/// The `ToolEval` provides the agent with the ability to programmatically manipulate
/// markdown documents using a Lisp-like language. Documents are read from the filesystem
/// on each evaluation.
///
/// # Available Functions
///
/// The evaluation environment includes:
///
/// - **Core builtins**: `first`, `rest`, `cons`, `append`, `map`, `filter`, `reduce`, etc.
/// - **Markdown functions**: `markdown-to-sexpr`, `sexpr-to-markdown`, `get-frontmatter`,
///   `generate-toc`, `annotate`, `prune`, `graft`, etc.
/// - **JSON functions**: `get`, `keys`, `values`, `assoc`, `dissoc`, etc.
///
/// # Document Variables
///
/// Markdown files in the filesystem root are available as variables using their filename.
/// For example, if `readme.md` exists in the root, it's available as `readme.md`.
/// Files are read and parsed on each evaluation.
pub struct ToolEval {
    filesystem: Path<'static>,
}

impl ToolEval {
    /// Creates a new eval tool rooted at the given filesystem path.
    ///
    /// # Arguments
    ///
    /// * `filesystem` - The root path for filesystem operations.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use agentkb::agent::ToolEval;
    /// use utf8path::Path;
    ///
    /// let tool = ToolEval::new(Path::new("docs").into_owned());
    /// ```
    pub fn new(filesystem: Path<'static>) -> Self {
        Self { filesystem }
    }

    /// Evaluates an s-expression string, loading documents from the filesystem.
    ///
    /// Creates an evaluation environment with all builtins and markdown functions
    /// registered, then discovers and binds markdown documents from the filesystem
    /// before evaluating the expression.
    ///
    /// # Arguments
    ///
    /// * `expr` - The s-expression string to evaluate.
    ///
    /// # Returns
    ///
    /// Returns the result of evaluation as a string, or an error message if evaluation fails.
    pub fn evaluate(&self, expr: &str) -> Result<String, String> {
        let mut parser = Parser::new(expr);
        let parsed = parser.parse().map_err(|e| e.to_string())?;

        let mut env = Env::new();
        register_builtins(&mut env);
        register_markdown_builtins(&mut env);

        // Discover and load markdown files from filesystem
        let doc_paths = find_markdown_files(std::path::Path::new(self.filesystem.as_str()));
        for path in &doc_paths {
            let full_path = self.filesystem.join(path);
            if let Ok(content) = fs::read_to_string(full_path.as_str())
                && let Ok(sexpr) = markdown_to_sexpr(&content)
            {
                env.bind(path, sexpr);
            }
        }

        let result = crate::s::eval::eval(&parsed, &env).map_err(|e| e.to_string())?;
        Ok(result.to_string())
    }

    /// Returns a list of available document variable names from the filesystem.
    pub fn available_documents(&self) -> Vec<String> {
        let doc_paths = find_markdown_files(std::path::Path::new(self.filesystem.as_str()));
        doc_paths
            .iter()
            .map(|p| Path::new(p).basename().to_string())
            .collect()
    }
}

/// Unit intermediate result for eval tool (computation happens in apply phase).
struct EvalUnit;

impl IntermediateToolResult for EvalUnit {
    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Callback implementation for the eval tool.
struct EvalCallback {
    filesystem: Path<'static>,
}

#[async_trait]
impl<A: Agent> ToolCallback<A> for EvalCallback {
    async fn compute_tool_result(
        &self,
        _client: &Anthropic,
        _agent: &A,
        _tool_use: &ToolUseBlock,
    ) -> Box<dyn IntermediateToolResult> {
        Box::new(EvalUnit)
    }

    async fn apply_tool_result(
        &self,
        _client: &Anthropic,
        _agent: &mut A,
        tool_use: &ToolUseBlock,
        _intermediate: Box<dyn IntermediateToolResult>,
    ) -> ToolResult {
        let expr = tool_use
            .input
            .get("expression")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        let tool = ToolEval::new(self.filesystem.clone());

        match tool.evaluate(expr) {
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

impl<A: Agent> Tool<A> for ToolEval {
    fn name(&self) -> String {
        "eval".to_string()
    }

    fn callback(&self) -> Box<dyn ToolCallback<A> + '_> {
        Box::new(EvalCallback {
            filesystem: self.filesystem.clone(),
        })
    }

    fn to_param(&self) -> ToolUnionParam {
        let doc_list = self.available_documents();
        let doc_description = if doc_list.is_empty() {
            "No markdown files found in filesystem.".to_string()
        } else {
            format!("Available document variables: {}", doc_list.join(", "))
        };

        let tool = ToolParam::new(
            "eval".to_string(),
            json!({
                "type": "object",
                "properties": {
                    "expression": {
                        "type": "string",
                        "description": "The s-expression to evaluate. Documents are available as variables by filename."
                    }
                },
                "required": ["expression"]
            }),
        )
        .with_description(format!(
            "Evaluate an s-expression against markdown documents from the filesystem. \
             The expression can use any registered builtin or markdown function. \
             Documents are read fresh from disk on each evaluation. \
             {}\n\n\
             Examples:\n\
             - (generate-toc readme.md) - Generate table of contents\n\
             - (get-frontmatter guide.md) - Get YAML frontmatter\n\
             - (annotate readme.md) - Add path IDs to all nodes\n\
             - (->> readme.md (get-by-path \"1\")) - Get first child element",
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
    fn eval_simple_expression() {
        let tool = ToolEval::new(test_filesystem());
        let result = tool.evaluate("(list 1 2 3)");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "(1 2 3)");
        println!("DEBUG: (list 1 2 3) = (1 2 3)");
    }

    #[test]
    fn eval_with_filesystem_document() {
        // This test uses the actual filesystem - stdlib.md or extlib.md should exist
        let tool = ToolEval::new(test_filesystem());
        let available = tool.available_documents();
        println!("DEBUG: available documents: {:?}", available);

        // If there are markdown files, test with one
        if !available.is_empty() {
            let doc_name = &available[0];
            let result = tool.evaluate(&format!("(generate-toc {})", doc_name));
            assert!(result.is_ok(), "generate-toc should succeed: {:?}", result);
            println!(
                "DEBUG: generate-toc result for {}: {}",
                doc_name,
                result.unwrap()
            );
        }
    }

    #[test]
    fn eval_invalid_expression() {
        let tool = ToolEval::new(test_filesystem());
        let result = tool.evaluate("(undefined-function)");
        assert!(result.is_err());
        println!("DEBUG: expected error for undefined function: {:?}", result);
    }

    #[test]
    fn eval_pipeline_with_inline_markdown() {
        let tool = ToolEval::new(test_filesystem());
        // Use markdown-to-sexpr to create a document inline
        let result =
            tool.evaluate("(->> (markdown-to-sexpr \"# Title\\n\\n## Section\") (annotate))");
        assert!(result.is_ok());
        let output = result.unwrap();
        assert!(
            output.contains("(@"),
            "Annotated doc should have path markers"
        );
        println!("DEBUG: annotate pipeline result: {}", output);
    }

    #[test]
    fn available_documents_from_filesystem() {
        let tool = ToolEval::new(test_filesystem());
        let available = tool.available_documents();
        // Should find at least stdlib.md and extlib.md in the project root
        println!(
            "DEBUG: available documents from filesystem: {:?}",
            available
        );
    }
}

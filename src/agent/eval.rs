//! S-expression evaluation tool for the AgentKB agent.
//!
//! This module provides a tool that allows the agent to evaluate s-expressions against
//! loaded markdown documents. Documents are pre-loaded and bound as variables in the
//! evaluation environment, enabling programmatic manipulation of the markdown wiki.
//!
//! # Example
//!
//! If documents `readme.md` and `guide.md` are loaded, the agent can evaluate expressions like:
//!
//! ```text
//! (generate-toc readme.md)
//! (get-frontmatter guide.md)
//! (->> readme.md (annotate) (get-by-path "1"))
//! ```

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

use crate::s::eval::{Env, register_builtins};
use crate::s::expr::{Parser, SExpr};
use crate::s::repl::register_markdown_builtins;

/// A tool for evaluating s-expressions against loaded markdown documents.
///
/// The `ToolEval` provides the agent with the ability to programmatically manipulate
/// markdown documents using a Lisp-like language. Documents are pre-loaded and available
/// as variables in the evaluation environment.
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
/// Each document loaded via the command line is available as a variable using its filename.
/// For example, if `docs/readme.md` is loaded, it's available as `readme.md`.
pub struct ToolEval {
    documents: Arc<RwLock<HashMap<String, SExpr>>>,
}

impl ToolEval {
    /// Creates a new eval tool with the given pre-loaded documents.
    ///
    /// # Arguments
    ///
    /// * `documents` - A map of document names to their parsed s-expression representations.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use std::collections::HashMap;
    /// use std::sync::{Arc, RwLock};
    /// use agentkb::agent::ToolEval;
    /// use agentkb::SExpr;
    ///
    /// let mut docs = HashMap::new();
    /// docs.insert("readme.md".to_string(), SExpr::Atom("placeholder".to_string()));
    /// let tool = ToolEval::new(Arc::new(RwLock::new(docs)));
    /// ```
    pub fn new(documents: Arc<RwLock<HashMap<String, SExpr>>>) -> Self {
        Self { documents }
    }

    /// Evaluates an s-expression string in the context of loaded documents.
    ///
    /// Creates an evaluation environment with all builtins and markdown functions
    /// registered, then binds each loaded document as a variable before evaluating
    /// the expression.
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

        let docs = self
            .documents
            .read()
            .map_err(|e| format!("Failed to acquire read lock: {}", e))?;
        for (name, doc) in docs.iter() {
            env.bind(name, doc.clone());
        }

        let result = crate::s::eval::eval(&parsed, &env).map_err(|e| e.to_string())?;
        Ok(result.to_string())
    }

    /// Returns a list of available document variable names.
    pub fn available_documents(&self) -> Vec<String> {
        self.documents
            .read()
            .map(|docs| docs.keys().cloned().collect())
            .unwrap_or_default()
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
    documents: Arc<RwLock<HashMap<String, SExpr>>>,
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

        let tool = ToolEval::new(Arc::clone(&self.documents));

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
            documents: Arc::clone(&self.documents),
        })
    }

    fn to_param(&self) -> ToolUnionParam {
        let doc_list: Vec<String> = self
            .documents
            .read()
            .map(|docs| docs.keys().cloned().collect())
            .unwrap_or_default();
        let doc_description = if doc_list.is_empty() {
            "No documents currently loaded.".to_string()
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
            "Evaluate an s-expression against loaded markdown documents. \
             The expression can use any registered builtin or markdown function. \
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
    use crate::markdown_to_sexpr;

    #[test]
    fn eval_simple_expression() {
        let docs = Arc::new(RwLock::new(HashMap::new()));
        let tool = ToolEval::new(docs);
        let result = tool.evaluate("(list 1 2 3)");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "(1 2 3)");
        println!("DEBUG: (list 1 2 3) = (1 2 3)");
    }

    #[test]
    fn eval_with_document() {
        let mut docs = HashMap::new();
        let doc = markdown_to_sexpr("# Hello\n\nWorld").unwrap();
        docs.insert("test.md".to_string(), doc);

        let tool = ToolEval::new(Arc::new(RwLock::new(docs)));
        let result = tool.evaluate("(generate-toc test.md)");
        assert!(result.is_ok());
        let output = result.unwrap();
        assert!(output.contains("ul"), "TOC should contain ul element");
        println!("DEBUG: generate-toc result: {}", output);
    }

    #[test]
    fn eval_invalid_expression() {
        let docs = Arc::new(RwLock::new(HashMap::new()));
        let tool = ToolEval::new(docs);
        let result = tool.evaluate("(undefined-function)");
        assert!(result.is_err());
        println!("DEBUG: expected error for undefined function: {:?}", result);
    }

    #[test]
    fn eval_pipeline() {
        let mut docs = HashMap::new();
        let doc = markdown_to_sexpr("# Title\n\n## Section\n\nParagraph").unwrap();
        docs.insert("doc.md".to_string(), doc);

        let tool = ToolEval::new(Arc::new(RwLock::new(docs)));
        let result = tool.evaluate("(->> doc.md (annotate))");
        assert!(result.is_ok());
        let output = result.unwrap();
        assert!(
            output.contains("(@"),
            "Annotated doc should have path markers"
        );
        println!("DEBUG: annotate pipeline result: {}", output);
    }

    #[test]
    fn available_documents_lists_loaded_docs() {
        let mut docs = HashMap::new();
        docs.insert("a.md".to_string(), SExpr::Atom("a".to_string()));
        docs.insert("b.md".to_string(), SExpr::Atom("b".to_string()));

        let tool = ToolEval::new(Arc::new(RwLock::new(docs)));
        let available = tool.available_documents();
        assert_eq!(available.len(), 2);
        assert!(available.contains(&"a.md".to_string()));
        assert!(available.contains(&"b.md".to_string()));
        println!("DEBUG: available documents: {:?}", available);
    }
}

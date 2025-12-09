//! Help tool for querying s-expression function documentation.
//!
//! This module provides a tool that allows the agent to look up documentation
//! for any s-expression function by name.

use std::any::Any;
use std::ops::ControlFlow;

use async_trait::async_trait;
use claudius::{
    Agent, Anthropic, IntermediateToolResult, Tool, ToolCallback, ToolParam, ToolResult,
    ToolResultBlock, ToolResultBlockContent, ToolUnionParam, ToolUseBlock,
};
use serde_json::json;

use super::docs::{ALL_DOCS, BUILTINS_DOCS, JSON_DOCS, MARKDOWN_DOCS};

/// A tool for looking up s-expression function documentation.
///
/// The `ToolHelp` provides the agent with the ability to query documentation
/// for any available function by name or to list all available functions.
pub struct ToolHelp;

impl ToolHelp {
    /// Creates a new help tool.
    pub fn new() -> Self {
        Self
    }

    /// Look up documentation for a function by name.
    ///
    /// Returns the full documentation content if found, or an error message if not.
    pub fn lookup(&self, query: &str) -> Result<String, String> {
        let query = query.trim();

        if query.is_empty() || query == "list" {
            return Ok(self.list_all());
        }

        // Try to find documentation matching the query
        for (path, content) in ALL_DOCS.iter() {
            let filename = path.rsplit('/').next().unwrap_or(path);
            let func_name = filename.trim_end_matches(".md");

            if func_name == query || func_name == query.replace('_', "-") {
                return Ok(content.to_string());
            }
        }

        // Partial match - find functions containing the query
        let matches: Vec<&str> = ALL_DOCS
            .iter()
            .filter_map(|(path, _)| {
                let filename = path.rsplit('/').next().unwrap_or(path);
                let func_name = filename.trim_end_matches(".md");
                if func_name.contains(query) {
                    Some(func_name)
                } else {
                    None
                }
            })
            .collect();

        if matches.is_empty() {
            Err(format!(
                "No documentation found for '{}'. Use 'list' to see all available functions.",
                query
            ))
        } else {
            Ok(format!(
                "No exact match for '{}'. Did you mean one of these?\n\n{}",
                query,
                matches.join("\n")
            ))
        }
    }

    /// List all available functions grouped by category.
    fn list_all(&self) -> String {
        let mut result = String::from("# Available Functions\n\n");

        result.push_str("## Builtins (Core Lisp)\n\n");
        for (path, _) in BUILTINS_DOCS.iter() {
            let func_name = path
                .rsplit('/')
                .next()
                .unwrap_or(path)
                .trim_end_matches(".md");
            result.push_str(&format!("- {}\n", func_name));
        }

        result.push_str("\n## JSON Functions\n\n");
        for (path, _) in JSON_DOCS.iter() {
            let func_name = path
                .rsplit('/')
                .next()
                .unwrap_or(path)
                .trim_end_matches(".md");
            result.push_str(&format!("- {}\n", func_name));
        }

        result.push_str("\n## Markdown Functions\n\n");
        for (path, _) in MARKDOWN_DOCS.iter() {
            let func_name = path
                .rsplit('/')
                .next()
                .unwrap_or(path)
                .trim_end_matches(".md");
            result.push_str(&format!("- {}\n", func_name));
        }

        result
    }
}

impl Default for ToolHelp {
    fn default() -> Self {
        Self::new()
    }
}

/// Unit intermediate result for help tool.
struct HelpUnit;

impl IntermediateToolResult for HelpUnit {
    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Callback implementation for the help tool.
struct HelpCallback;

#[async_trait]
impl<A: Agent> ToolCallback<A> for HelpCallback {
    async fn compute_tool_result(
        &self,
        _client: &Anthropic,
        _agent: &A,
        _tool_use: &ToolUseBlock,
    ) -> Box<dyn IntermediateToolResult> {
        Box::new(HelpUnit)
    }

    async fn apply_tool_result(
        &self,
        _client: &Anthropic,
        _agent: &mut A,
        tool_use: &ToolUseBlock,
        _intermediate: Box<dyn IntermediateToolResult>,
    ) -> ToolResult {
        let query = tool_use
            .input
            .get("function")
            .and_then(|v| v.as_str())
            .unwrap_or("list");

        let tool = ToolHelp::new();

        match tool.lookup(query) {
            Ok(result) => ControlFlow::Continue(Ok(ToolResultBlock {
                tool_use_id: tool_use.id.clone(),
                cache_control: None,
                content: Some(ToolResultBlockContent::String(result)),
                is_error: None,
            })),
            Err(e) => ControlFlow::Continue(Err(ToolResultBlock {
                tool_use_id: tool_use.id.clone(),
                cache_control: None,
                content: Some(ToolResultBlockContent::String(e)),
                is_error: Some(true),
            })),
        }
    }
}

impl<A: Agent> Tool<A> for ToolHelp {
    fn name(&self) -> String {
        "help".to_string()
    }

    fn callback(&self) -> Box<dyn ToolCallback<A> + '_> {
        Box::new(HelpCallback)
    }

    fn to_param(&self) -> ToolUnionParam {
        let tool = ToolParam::new(
            "help".to_string(),
            json!({
                "type": "object",
                "properties": {
                    "function": {
                        "type": "string",
                        "description": "The function name to look up, or 'list' to see all functions."
                    }
                },
                "required": ["function"]
            }),
        )
        .with_description(
            "Look up documentation for an s-expression function. \
             Use 'list' or omit the function parameter to see all available functions. \
             Examples: 'prune', 'annotate', 'replace-at', 'get-frontmatter'."
                .to_string(),
        );

        ToolUnionParam::CustomTool(tool)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lookup_exact_match() {
        let tool = ToolHelp::new();
        let result = tool.lookup("prune");
        assert!(result.is_ok());
        let content = result.unwrap();
        assert!(content.contains("# prune"), "Should contain prune heading");
        assert!(
            content.contains("Remove a node"),
            "Should contain description"
        );
        println!("DEBUG: prune doc found with {} chars", content.len());
    }

    #[test]
    fn lookup_builtin() {
        let tool = ToolHelp::new();
        let result = tool.lookup("first");
        assert!(result.is_ok());
        let content = result.unwrap();
        assert!(content.contains("# first"), "Should contain first heading");
        println!("DEBUG: first doc found");
    }

    #[test]
    fn lookup_json_function() {
        let tool = ToolHelp::new();
        let result = tool.lookup("assoc");
        assert!(result.is_ok());
        let content = result.unwrap();
        assert!(content.contains("# assoc"), "Should contain assoc heading");
        println!("DEBUG: assoc doc found");
    }

    #[test]
    fn lookup_not_found() {
        let tool = ToolHelp::new();
        let result = tool.lookup("nonexistent-function");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.contains("No documentation found"),
            "Should indicate not found"
        );
        println!("DEBUG: expected error: {}", err);
    }

    #[test]
    fn lookup_partial_match() {
        let tool = ToolHelp::new();
        // Use a prefix that doesn't match exactly but matches multiple functions
        let result = tool.lookup("insert");
        assert!(result.is_ok());
        let content = result.unwrap();
        // Should suggest functions containing "insert"
        assert!(
            content.contains("insert-before") || content.contains("insert-after"),
            "Should suggest insert functions: {}",
            content
        );
        println!("DEBUG: partial match suggestions: {}", content);
    }

    #[test]
    fn list_all_functions() {
        let tool = ToolHelp::new();
        let result = tool.lookup("list");
        assert!(result.is_ok());
        let content = result.unwrap();
        assert!(
            content.contains("# Available Functions"),
            "Should have header"
        );
        assert!(
            content.contains("## Builtins"),
            "Should have builtins section"
        );
        assert!(content.contains("## JSON"), "Should have JSON section");
        assert!(
            content.contains("## Markdown"),
            "Should have markdown section"
        );
        assert!(content.contains("- prune"), "Should list prune");
        assert!(content.contains("- first"), "Should list first");
        println!("DEBUG: function list has {} lines", content.lines().count());
    }

    #[test]
    fn empty_query_lists_all() {
        let tool = ToolHelp::new();
        let result = tool.lookup("");
        assert!(result.is_ok());
        let content = result.unwrap();
        assert!(
            content.contains("# Available Functions"),
            "Empty should list all"
        );
        println!("DEBUG: empty query returned list");
    }
}

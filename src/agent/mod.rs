//! Agent harness for knowledge base operations.
//!
//! This module provides an agent implementation for interacting with markdown-based
//! knowledge bases through the Anthropic API.  The agent is chrooted to a filesystem
//! path and equipped with text editing tools for document manipulation.
//!
//! # Architecture
//!
//! The [`AgentKB`] struct implements the [`claudius::Agent`] trait, providing:
//!
//! - A chrooted filesystem view restricting operations to a knowledge base directory
//! - Text editor tooling for file manipulation
//! - S-expression evaluation for programmatic document manipulation
//! - Configurable model selection and token limits
//! - Request/response hooks for debugging and logging
//!
//! # Example
//!
//! ```rust,no_run
//! use std::sync::Arc;
//!
//! use agentkb::agent::AgentKB;
//! use claudius::{Agent, Anthropic, Budget, MessageParam, MessageParamContent, MessageRole, StopReason};
//! use utf8path::Path;
//!
//! #[tokio::main]
//! async fn main() {
//!     let client = Anthropic::new(None).unwrap();
//!     let mut agent = AgentKB::new(&Path::new("kb"));
//!     let budget = Arc::new(Budget::from_dollars_flat_rate(0.25, 1000));
//!     let mut messages = vec![MessageParam {
//!         role: MessageRole::User,
//!         content: MessageParamContent::String("List all markdown files.".to_string()),
//!     }];
//!
//!     loop {
//!         match agent.take_turn(&client, &mut messages, &budget).await.unwrap() {
//!             StopReason::EndTurn | StopReason::MaxTokens => break,
//!             _ => {}
//!         }
//!     }
//! }
//! ```

pub mod docs;
mod edit;
mod eval;
mod help;

pub use edit::ToolEdit;
pub use eval::ToolEval;
pub use help::ToolHelp;

use std::sync::Arc;

use claudius::{
    Agent, FileSystem, KnownModel, Message, MessageCreateParams, Model, SystemPrompt, Tool,
};
use utf8path::Path;

/// An agent for knowledge base operations over a chrooted filesystem.
///
/// `AgentKB` wraps a filesystem path and provides an [`Agent`] implementation
/// that restricts all file operations to that path.  The agent is equipped with
/// a text editor tool for reading and modifying markdown documents, as well as
/// an s-expression evaluation tool for programmatic document manipulation.
///
/// # Filesystem Isolation
///
/// The agent operates as if chrooted to its configured path.  All file paths
/// provided to tools are interpreted relative to this root, preventing access
/// to files outside the knowledge base.
///
/// # Tools
///
/// The agent is equipped with:
///
/// - [`ToolTextEditor20250728`]: A text editor for viewing, searching, and
///   modifying files within the knowledge base.
/// - [`ToolEval`]: An s-expression evaluator for programmatic manipulation
///   of markdown documents using Lisp-like expressions. Reads files from disk
///   on each evaluation.
/// - [`ToolEdit`]: An editor for applying s-expression transforms to documents
///   in place. Reads from disk, applies transform, writes back to disk.
pub struct AgentKB {
    tools: Vec<Arc<dyn Tool<Self>>>,
    filesystem: Path<'static>,
}

impl AgentKB {
    /// Create a new knowledge base agent rooted at the given filesystem path.
    ///
    /// The agent will have access to all files under `filesystem` and will be
    /// equipped with text editing tools for document manipulation. Markdown
    /// files are read from disk on each operation rather than being cached
    /// in memory.
    ///
    /// # Arguments
    ///
    /// * `filesystem` - The root path for filesystem operations.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use agentkb::agent::AgentKB;
    /// use utf8path::Path;
    ///
    /// // Agent rooted at docs/
    /// let agent = AgentKB::new(&Path::new("docs"));
    /// ```
    pub fn new(filesystem: &Path) -> Self {
        let filesystem = filesystem.clone().into_owned();
        let tools: Vec<Arc<dyn Tool<Self>>> = vec![
            Arc::new(ToolEval::new(filesystem.clone())),
            Arc::new(ToolEdit::new(filesystem.clone())),
            Arc::new(ToolHelp::new()),
        ];

        Self { tools, filesystem }
    }

    /// Returns the filesystem root path for this agent.
    pub fn filesystem_root(&self) -> &Path<'_> {
        &self.filesystem
    }

    /// Build the system prompt with documentation and available documents.
    fn build_system_prompt(&self) -> String {
        let doc_list = self.available_documents();
        let doc_section = if doc_list.is_empty() {
            "No markdown documents found.".to_string()
        } else {
            format!(
                "Available documents (use as variables in eval, or as filenames in edit):\n{}",
                doc_list
                    .iter()
                    .map(|d| format!("  - {}", d))
                    .collect::<Vec<_>>()
                    .join("\n")
            )
        };

        format!(
            r##"You are an agent for editing markdown documents in a knowledge base at /.

# Tools

You have three tools:

1. **eval** - Evaluate s-expressions to query documents (read-only)
2. **edit** - Apply s-expression transforms to modify documents in place
3. **help** - Look up documentation for any s-expression function

Use `eval` to explore document structure. Use `edit` to make changes.
Use `help` to learn about functions (e.g., help function="prune" or help function="list").

# S-Expression Syntax

Expressions use Lisp syntax: `(function arg1 arg2 ...)`.

## Core Functions

- `(first lst)` - First element
- `(rest lst)` - All but first
- `(cons x lst)` - Prepend x to list
- `(append lst1 lst2)` - Concatenate lists
- `(map fn lst)` - Apply fn to each element
- `(filter fn lst)` - Keep elements where fn is true
- `(quote expr)` - Return expr unevaluated

## Threading

`(->> initial form1 form2 ...)` threads a value through forms as the last argument:
```lisp
(->> "# Title" (markdown-to-sexpr) (annotate))
```

# Document Operations

## Querying (use with eval tool)

- `(annotate doc)` - Add path IDs to all nodes (e.g., @1, @1.2)
- `(get-by-path doc "1.2")` - Get node at path
- `(get-frontmatter doc)` - Get YAML frontmatter
- `(generate-toc doc)` - Generate table of contents
- `(get-internal-links doc)` - List internal links
- `(get-external-links doc)` - List external links

## Mutations (use with edit tool)

The edit tool takes a filename and a transform. The document is automatically
inserted as the first argument (thread-first style).

- `(prune "path")` - Remove node at path
- `(replace-at "path" (quote (h1 "New")))` - Replace node
- `(insert-before "path" (quote (p "text")))` - Insert before path
- `(insert-after "path" (quote (p "text")))` - Insert after path
- `(hoist "path" -1)` - Decrease heading level (h2→h1)
- `(hoist "path" 1)` - Increase heading level (h1→h2)
- `(graft "src" "dst")` - Move node from src to dst

## Path System

Paths identify nodes by position: "1" is first child, "1.2" is second child of first child.
Use `(annotate doc)` with the eval tool to see all paths in a document.

# Examples

## Print a document as markdown

```
eval: (sexpr-to-markdown readme.md)
```

## Print the first heading of every document

```
eval: (map (lambda (d) (get-by-path d "1")) (list readme.md guide.md api.md))
```

## Explore document structure

```
eval: (annotate readme.md)
```

## Edit workflow

1. First, explore structure with annotate to find paths
2. Identify the path you want to modify (e.g., "2.3")
3. Apply the edit:
   ```
   edit: filename="readme.md", transform="(prune \"2.3\")"
   ```

# {doc_section}
"##,
            doc_section = doc_section
        )
    }

    /// Returns available document names from the filesystem.
    fn available_documents(&self) -> Vec<String> {
        crate::s::util::find_markdown_files(std::path::Path::new(self.filesystem.as_str()))
            .iter()
            .map(|p| p.to_string())
            .collect()
    }
}

#[async_trait::async_trait]
impl Agent for AgentKB {
    async fn model(&self) -> Model {
        Model::Known(KnownModel::ClaudeOpus45)
    }

    async fn max_tokens(&self) -> u32 {
        10240
    }

    async fn tools(&self) -> Vec<Arc<dyn Tool<Self>>> {
        self.tools.clone()
    }

    async fn system(&self) -> Option<SystemPrompt> {
        Some(self.build_system_prompt().into())
    }

    async fn filesystem(&self) -> Option<&dyn FileSystem> {
        Some(&self.filesystem)
    }

    async fn hook_message_create_params(
        &self,
        req: &MessageCreateParams,
    ) -> Result<(), claudius::Error> {
        println!("request: {}", serde_json::to_string_pretty(&req).unwrap());
        Ok(())
    }

    async fn hook_message(&self, resp: &Message) -> Result<(), claudius::Error> {
        println!("response: {}", serde_json::to_string_pretty(&resp).unwrap());
        Ok(())
    }
}

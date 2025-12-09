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

use claudius::{Agent, FileSystem, KnownModel, Model, SystemPrompt, Tool};
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

        include_str!("system.md").to_string() + &format!("\n\n# {doc_section}")
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
}

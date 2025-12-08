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

pub use edit::ToolEdit;
pub use eval::ToolEval;

use std::collections::HashMap;
use std::fs;
use std::sync::{Arc, RwLock};

use claudius::{
    Agent, FileSystem, KnownModel, Message, MessageCreateParams, Model, SystemPrompt, Tool,
    ToolTextEditor20250728,
};
use utf8path::Path;

use crate::SExpr;
use crate::markdown_to_sexpr;

/// Recursively find all markdown files under a directory.
///
/// Returns a vector of relative paths (as strings) to all `.md` files found
/// under the given directory, recursively traversing subdirectories.
fn find_markdown_files(dir: &std::path::Path) -> Vec<String> {
    find_markdown_files_recursive(dir, dir)
}

/// Recursive helper for `find_markdown_files`.
fn find_markdown_files_recursive(root: &std::path::Path, dir: &std::path::Path) -> Vec<String> {
    let mut files = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                files.extend(find_markdown_files_recursive(root, &path));
            } else if path.extension().is_some_and(|ext| ext == "md")
                && let Some(relative) = path.strip_prefix(root).ok().and_then(|p| p.to_str())
            {
                files.push(relative.to_string());
            }
        }
    }
    files
}

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
///   of markdown documents using Lisp-like expressions.
/// - [`ToolEdit`]: An editor for applying s-expression transforms to documents
///   in place.
pub struct AgentKB {
    tools: Vec<Arc<dyn Tool<Self>>>,
    filesystem: Path<'static>,
    documents: Arc<RwLock<HashMap<String, SExpr>>>,
}

impl AgentKB {
    /// Create a new knowledge base agent rooted at the given filesystem path.
    ///
    /// The agent will have access to all files under `filesystem` and will be
    /// equipped with text editing tools for document manipulation. All markdown
    /// files (*.md) under the filesystem root are automatically discovered and
    /// loaded as variables in the eval tool.
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
    /// // Agent that auto-discovers all .md files under docs/
    /// let agent = AgentKB::new(&Path::new("docs"));
    /// ```
    pub fn new(filesystem: &Path) -> Self {
        let document_paths = find_markdown_files(std::path::Path::new(filesystem.as_str()));
        let mut documents = HashMap::new();

        for path in &document_paths {
            let full_path = filesystem.join(path);
            if let Ok(content) = fs::read_to_string(full_path.as_str())
                && let Ok(sexpr) = markdown_to_sexpr(&content)
            {
                // Use just the basename as the variable name
                let path_ref = Path::new(path);
                let name = path_ref.basename().to_string();
                documents.insert(name, sexpr);
            }
        }

        let documents = Arc::new(RwLock::new(documents));
        let tools: Vec<Arc<dyn Tool<Self>>> = vec![
            Arc::new(ToolTextEditor20250728::new()),
            Arc::new(ToolEval::new(Arc::clone(&documents))),
            Arc::new(ToolEdit::new(Arc::clone(&documents))),
        ];
        let filesystem = filesystem.clone().into_owned();

        Self {
            tools,
            filesystem,
            documents,
        }
    }

    /// Returns a reference to the loaded documents wrapped in RwLock.
    pub fn documents(&self) -> &RwLock<HashMap<String, SExpr>> {
        &self.documents
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
        Some(
            "You are chrooted in an extensive, cross-linked, markdown-based Wiki in /.  \
             You are a proof-reading agent.  Accomplish the user's task."
                .into(),
        )
    }

    async fn filesystem(&self) -> Option<&dyn FileSystem> {
        Some(&self.filesystem)
    }

    async fn hook_message_create_params(
        &self,
        req: &MessageCreateParams,
    ) -> Result<(), claudius::Error> {
        println!("request: {req:?}");
        Ok(())
    }

    async fn hook_message(&self, resp: &Message) -> Result<(), claudius::Error> {
        println!("response: {resp:?}");
        Ok(())
    }
}

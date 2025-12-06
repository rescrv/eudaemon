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
//! - Configurable model selection and token limits
//! - Request/response hooks for debugging and logging
//!
//! # Example
//!
//! ```rust,no_run
//! use std::sync::Arc;
//!
//! use agentkb::agent::AgentKB;
//! use claudius::{Anthropic, Budget, MessageParam, MessageParamContent, MessageRole, StopReason};
//! use utf8path::Path;
//!
//! #[tokio::main]
//! async fn main() {
//!     let client = Anthropic::new(None).unwrap();
//!     let mut agent = AgentKB::new(Path::new("kb"));
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

use std::sync::Arc;

use claudius::{Agent, FileSystem, KnownModel, Model, SystemPrompt, Tool, ToolTextEditor20250429};
use utf8path::Path;

/// An agent for knowledge base operations over a chrooted filesystem.
///
/// `AgentKB` wraps a filesystem path and provides an [`Agent`] implementation
/// that restricts all file operations to that path.  The agent is equipped with
/// a text editor tool for reading and modifying markdown documents.
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
/// - [`ToolTextEditor20250429`]: A text editor for viewing, searching, and
///   modifying files within the knowledge base.
pub struct AgentKB {
    tools: Vec<Arc<dyn Tool<Self>>>,
    filesystem: Path<'static>,
}

impl AgentKB {
    /// Create a new knowledge base agent rooted at the given filesystem path.
    ///
    /// The agent will have access to all files under `filesystem` and will be
    /// equipped with text editing tools for document manipulation.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use agentkb::agent::AgentKB;
    /// use utf8path::Path;
    ///
    /// let agent = AgentKB::new(Path::new("docs"));
    /// ```
    pub fn new(filesystem: &Path) -> Self {
        let tools = vec![Arc::new(ToolTextEditor20250429::new()) as _];
        let filesystem = filesystem.clone().into_owned();
        Self { tools, filesystem }
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
}

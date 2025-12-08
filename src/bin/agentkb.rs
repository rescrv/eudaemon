//! Run the AgentKB agent against a markdown knowledge base.
//!
//! This binary provides an interactive command-line interface for running the AgentKB
//! agent against a filesystem path.  The agent is equipped with text editing tools
//! and an s-expression evaluator, operating in a chrooted environment restricted to
//! the specified directory.  All markdown files (*.md) under the path are automatically
//! discovered and loaded as variables in the eval tool.
//!
//! # Usage
//!
//! ```text
//! agentkb [path]
//! ```
//!
//! If no path is specified, the current working directory is used.
//!
//! # Environment
//!
//! Requires `ANTHROPIC_API_KEY` to be set.
//!
//! # Examples
//!
//! ```text
//! # Run in current directory
//! agentkb
//!
//! # Run in a specific directory
//! agentkb /path/to/wiki
//! ```

use std::env;
use std::io::Write;
use std::process::ExitCode;
use std::sync::Arc;

use agentkb::agent::AgentKB;
use claudius::{
    Agent, Anthropic, Budget, ContentBlock, MessageParam, MessageParamContent, MessageRole,
    StopReason,
};
use utf8path::Path;

/// Run the AgentKB agent interactively.
///
/// Reads user prompts from stdin and passes them to the agent, printing
/// assistant responses to stdout.
async fn run_agent(path: &Path<'_>) -> Result<(), claudius::Error> {
    let client = Anthropic::new(None)?;
    let mut agent = AgentKB::new(path);
    let budget = Arc::new(Budget::from_dollars_with_rates(1.0, 500, 2500, 625, 50));
    let mut messages: Vec<MessageParam> = Vec::new();

    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();

    loop {
        print!("> ");
        stdout.flush().ok();

        let mut input = String::new();
        if stdin.read_line(&mut input).unwrap_or(0) == 0 {
            break;
        }

        let input = input.trim();
        if input.is_empty() {
            continue;
        }

        if input == "/quit" || input == "/exit" {
            break;
        }

        messages.push(MessageParam {
            role: MessageRole::User,
            content: MessageParamContent::String(input.to_string()),
        });

        loop {
            match agent.take_turn(&client, &mut messages, &budget).await? {
                StopReason::EndTurn | StopReason::MaxTokens => break,
                StopReason::ToolUse => {}
                StopReason::StopSequence | StopReason::Refusal | StopReason::PauseTurn => break,
            }
        }

        if let Some(last) = messages.last()
            && last.role == MessageRole::Assistant
        {
            print_assistant_message(&last.content);
        }
    }

    Ok(())
}

/// Print the assistant's message content to stdout.
fn print_assistant_message(content: &MessageParamContent) {
    match content {
        MessageParamContent::String(s) => {
            println!("{}", s);
        }
        MessageParamContent::Array(blocks) => {
            for block in blocks {
                if let ContentBlock::Text(text_block) = block {
                    println!("{}", text_block.text);
                }
            }
        }
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    let args: Vec<String> = env::args().collect();

    let dir = if args.len() > 1 {
        args[1].clone()
    } else {
        env::current_dir()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| ".".to_string())
    };

    let path = Path::new(&dir);

    if let Err(e) = run_agent(&path).await {
        eprintln!("Error: {}", e);
        return ExitCode::FAILURE;
    }

    ExitCode::SUCCESS
}

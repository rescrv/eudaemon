//! Eudaemon CLI: A command-line interface for running Claude agents on eudaemon images.
//!
//! This binary provides Claude with access to a rooted filesystem via the
//! text editor tool and a dedicated lispdown tool.

#![deny(missing_docs)]

mod lispdown_tool;

use std::io::{self, IsTerminal, Read};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;
use claudius::chat::{
    ChatAgent, ChatCommand, ChatConfig, ChatSession, PlainTextRenderer, help_text, parse_command,
};
use claudius::{
    Agent, Anthropic, CacheControlEphemeral, Error, FileSystem, KnownModel, Message, MessageParam,
    Model, OperatorLine, Renderer, StopReason, StreamContext, SystemPrompt, TextBlock,
    ThinkingConfig, ToolTextEditor20250728,
};
use lispdown_tool::LispdownTool;
use rustyline::DefaultEditor;
use rustyline::error::ReadlineError;
use utf8path::Path;

/// CLI usage string.
const USAGE: &str = "Usage: eudaemoncli <filesystem-root> [prompt]";

/// System prompt for knowledge-base manipulation with the available tools.
const SYSTEM_PROMPT_TEXT: &str = r#"You are maintaining a markdown knowledge base rooted at /.

You have access to the text editor tool and a dedicated lispdown tool. Use lispdown for
structured markdown queries and transformations when it is a better fit than direct text edits.
Inspect files before editing, keep paths inside the rooted knowledge base, explain substantive
changes, and avoid touching unrelated files."#;

/// Returns the default chat configuration for eudaemoncli.
fn default_chat_config() -> ChatConfig {
    ChatConfig::new()
        .with_model(Model::Known(KnownModel::ClaudeHaiku45))
        .with_system_prompt(SYSTEM_PROMPT_TEXT.to_string())
        .with_max_tokens(32768)
}

/// The Eudaemon agent that provides filesystem access to Claude.
struct EudaemonAgent {
    filesystem: Path<'static>,
    config: ChatConfig,
}

impl EudaemonAgent {
    /// Creates a new EudaemonAgent rooted at the provided filesystem path.
    fn new(filesystem: Path<'static>) -> Self {
        Self::with_config(filesystem, default_chat_config())
    }

    /// Creates a new EudaemonAgent rooted at the provided filesystem path and chat config.
    fn with_config(filesystem: Path<'static>, config: ChatConfig) -> Self {
        Self { filesystem, config }
    }
}

#[async_trait]
impl Agent for EudaemonAgent {
    async fn max_tokens(&self) -> u32 {
        self.config.max_tokens()
    }

    fn stream_label(&self) -> String {
        "eudaemoncli".to_string()
    }

    async fn model(&self) -> Model {
        self.config.model()
    }

    async fn stop_sequences(&self) -> Option<Vec<String>> {
        let sequences = self.config.stop_sequences();
        if sequences.is_empty() {
            None
        } else {
            Some(sequences.to_vec())
        }
    }

    async fn system(&self) -> Option<SystemPrompt> {
        let prompt = self.config.template.system.as_ref()?;

        if self.config.caching_enabled {
            let mut blocks = match prompt {
                SystemPrompt::String(text) => vec![TextBlock::new(text.clone())],
                SystemPrompt::Blocks(existing) => {
                    existing.iter().map(|block| block.block.clone()).collect()
                }
            };
            if let Some(last) = blocks.last_mut() {
                last.cache_control = Some(CacheControlEphemeral::new());
            }
            Some(SystemPrompt::from_blocks(blocks))
        } else {
            Some(prompt.clone())
        }
    }

    async fn temperature(&self) -> Option<f32> {
        self.config.template.temperature
    }

    async fn thinking(&self) -> Option<ThinkingConfig> {
        self.config.template.thinking
    }

    async fn top_k(&self) -> Option<u32> {
        self.config.template.top_k
    }

    async fn top_p(&self) -> Option<f32> {
        self.config.template.top_p
    }

    async fn tools(&self) -> Vec<Arc<dyn claudius::Tool<Self>>> {
        vec![
            Arc::new(ToolTextEditor20250728::new()) as _,
            Arc::new(LispdownTool) as _,
        ]
    }

    async fn tool_choice(&self) -> Option<claudius::ToolChoice> {
        Some(claudius::ToolChoice::Auto {
            disable_parallel_tool_use: None,
        })
    }

    async fn filesystem(&self) -> Option<&dyn FileSystem> {
        Some(&self.filesystem)
    }

    async fn hook_message(&self, _resp: &Message) -> Result<(), Error> {
        Ok(())
    }
}

impl ChatAgent for EudaemonAgent {
    fn config(&self) -> &ChatConfig {
        &self.config
    }

    fn config_mut(&mut self) -> &mut ChatConfig {
        &mut self.config
    }
}

/// An interactive terminal that matches claudius-chat's renderer-backed REPL model.
struct ChatTerminal {
    editor: DefaultEditor,
    renderer: PlainTextRenderer,
}

impl ChatTerminal {
    /// Create a new terminal with line editing and streaming renderer output.
    fn new(
        use_color: bool,
        interrupted: Arc<AtomicBool>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        Ok(Self {
            editor: DefaultEditor::new()?,
            renderer: PlainTextRenderer::with_color_and_interrupt(use_color, interrupted),
        })
    }

    /// Read a line from the terminal and normalize it into a renderer operator line.
    fn read_line(&mut self, prompt: &str) -> io::Result<OperatorLine> {
        match self.editor.readline(prompt) {
            Ok(line) => Ok(OperatorLine::Line(line)),
            Err(ReadlineError::Interrupted) => Ok(OperatorLine::Interrupted),
            Err(ReadlineError::Eof) => Ok(OperatorLine::Eof),
            Err(err) => Err(io::Error::other(err.to_string())),
        }
    }

    /// Add an entry to the line editor history.
    fn add_history_entry(&mut self, line: &str) {
        let _ = self.editor.add_history_entry(line);
    }
}

impl Renderer for ChatTerminal {
    fn start_agent(&mut self, context: &dyn StreamContext) {
        self.renderer.start_agent(context);
    }

    fn finish_agent(&mut self, context: &dyn StreamContext, stop_reason: Option<&StopReason>) {
        self.renderer.finish_agent(context, stop_reason);
    }

    fn print_text(&mut self, context: &dyn StreamContext, text: &str) {
        self.renderer.print_text(context, text);
    }

    fn print_thinking(&mut self, context: &dyn StreamContext, text: &str) {
        self.renderer.print_thinking(context, text);
    }

    fn print_error(&mut self, context: &dyn StreamContext, error: &str) {
        self.renderer.print_error(context, error);
    }

    fn print_info(&mut self, context: &dyn StreamContext, info: &str) {
        self.renderer.print_info(context, info);
    }

    fn start_tool_use(&mut self, context: &dyn StreamContext, name: &str, id: &str) {
        self.renderer.start_tool_use(context, name, id);
    }

    fn print_tool_input(&mut self, context: &dyn StreamContext, partial_json: &str) {
        self.renderer.print_tool_input(context, partial_json);
    }

    fn finish_tool_use(&mut self, context: &dyn StreamContext) {
        self.renderer.finish_tool_use(context);
    }

    fn start_tool_result(
        &mut self,
        context: &dyn StreamContext,
        tool_use_id: &str,
        is_error: bool,
    ) {
        self.renderer
            .start_tool_result(context, tool_use_id, is_error);
    }

    fn print_tool_result_text(&mut self, context: &dyn StreamContext, text: &str) {
        self.renderer.print_tool_result_text(context, text);
    }

    fn finish_tool_result(&mut self, context: &dyn StreamContext) {
        self.renderer.finish_tool_result(context);
    }

    fn finish_response(&mut self, context: &dyn StreamContext) {
        self.renderer.finish_response(context);
    }

    fn print_interrupted(&mut self, context: &dyn StreamContext) {
        self.renderer.print_interrupted(context);
    }

    fn should_interrupt(&self) -> bool {
        self.renderer.should_interrupt()
    }

    fn read_operator_line(&mut self, prompt: &str) -> io::Result<Option<OperatorLine>> {
        self.read_line(prompt).map(Some)
    }
}

/// Determine the initial user prompt from command-line arguments or stdin.
fn initial_prompt_from_inputs(
    args: &[String],
    stdin: &str,
    stdin_is_terminal: bool,
) -> Option<String> {
    let prompt = if args.len() > 2 {
        args[2..].join(" ")
    } else if stdin_is_terminal {
        String::new()
    } else {
        stdin.to_string()
    };
    let prompt = prompt.trim();
    if prompt.is_empty() {
        None
    } else {
        Some(prompt.to_string())
    }
}

/// Send a single user turn to the chat session.
async fn send_turn<A: ChatAgent, R: Renderer>(
    session: &mut ChatSession<A>,
    renderer: &mut R,
    prompt: &str,
) {
    if let Err(err) = session
        .send_message(MessageParam::user(prompt), renderer)
        .await
    {
        renderer.print_error(&(), &err.to_string());
    }
}

/// Run the interactive multi-turn REPL using claudius's chat command model.
async fn run_repl<A: ChatAgent>(
    session: &mut ChatSession<A>,
    terminal: &mut ChatTerminal,
    interrupted: &AtomicBool,
) -> Result<(), Box<dyn std::error::Error>> {
    let context = ();

    println!("eudaemoncli (model: {})", session.config().model());
    println!("Type /help for commands, /quit to exit\n");

    loop {
        interrupted.store(false, Ordering::Relaxed);

        match terminal.read_operator_line("You: ")? {
            Some(OperatorLine::Line(line)) => {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }

                terminal.add_history_entry(line);

                if let Some(cmd) = parse_command(line) {
                    match cmd {
                        ChatCommand::Quit => {
                            println!("Goodbye!");
                            break;
                        }
                        ChatCommand::Clear => {
                            session.clear();
                            terminal.print_info(&context, "Conversation cleared.");
                        }
                        ChatCommand::Help => {
                            for line in help_text().lines() {
                                println!("    {}", line);
                            }
                        }
                        ChatCommand::Model(model_name) => {
                            let model = model_name
                                .parse()
                                .unwrap_or_else(|_| Model::Custom(model_name.clone()));
                            session.template_mut().model = Some(model);
                            terminal
                                .print_info(&context, &format!("Model changed to: {}", model_name));
                        }
                        ChatCommand::System(prompt) => {
                            session.template_mut().system = prompt.clone().map(SystemPrompt::from);
                            match prompt {
                                Some(prompt) => terminal.print_info(
                                    &context,
                                    &format!("System prompt set to: {}", prompt),
                                ),
                                None => terminal.print_info(&context, "System prompt cleared."),
                            }
                        }
                        ChatCommand::MaxTokens(value) => {
                            session.template_mut().max_tokens = Some(value);
                            terminal.print_info(&context, &format!("max_tokens set to {value}"));
                        }
                        ChatCommand::Temperature(value) => {
                            session.template_mut().temperature = Some(value);
                            terminal
                                .print_info(&context, &format!("temperature set to {:.2}", value));
                        }
                        ChatCommand::ClearTemperature => {
                            session.template_mut().temperature = None;
                            terminal.print_info(&context, "temperature reset to model default");
                        }
                        ChatCommand::TopP(value) => {
                            session.template_mut().top_p = Some(value);
                            terminal.print_info(&context, &format!("top_p set to {:.2}", value));
                        }
                        ChatCommand::ClearTopP => {
                            session.template_mut().top_p = None;
                            terminal.print_info(&context, "top_p reset to model default");
                        }
                        ChatCommand::TopK(value) => {
                            session.template_mut().top_k = Some(value);
                            terminal.print_info(&context, &format!("top_k set to {value}"));
                        }
                        ChatCommand::ClearTopK => {
                            session.template_mut().top_k = None;
                            terminal.print_info(&context, "top_k reset to model default");
                        }
                        ChatCommand::AddStopSequence(sequence) => {
                            let stop_sequences = session
                                .template_mut()
                                .stop_sequences
                                .get_or_insert_with(Vec::new);
                            if !stop_sequences.iter().any(|s| s == &sequence) {
                                stop_sequences.push(sequence.clone());
                            }
                            terminal
                                .print_info(&context, &format!("Added stop sequence: {sequence}"));
                        }
                        ChatCommand::ClearStopSequences => {
                            session.template_mut().stop_sequences = None;
                            terminal.print_info(&context, "Stop sequences cleared.");
                        }
                        ChatCommand::ListStopSequences => {
                            let sequences =
                                session.template().stop_sequences.as_deref().unwrap_or(&[]);
                            print_stop_sequences(sequences);
                        }
                        ChatCommand::Thinking(budget) => {
                            session.template_mut().thinking = budget.map(ThinkingConfig::enabled);
                            match budget {
                                Some(tokens) => {
                                    terminal.print_info(
                                        &context,
                                        &format!(
                                            "Extended thinking enabled with {} token budget.",
                                            tokens
                                        ),
                                    );
                                }
                                None => {
                                    terminal.print_info(&context, "Extended thinking disabled.");
                                }
                            }
                        }
                        ChatCommand::Budget(_tokens) => {
                            terminal.print_error(&context, "budget not supported");
                        }
                        ChatCommand::ClearBudget => {
                            session.config_mut().session_budget = None;
                            terminal.print_info(&context, "Session budget cleared.");
                        }
                        ChatCommand::Caching(enabled) => {
                            session.config_mut().caching_enabled = enabled;
                            if enabled {
                                terminal.print_info(&context, "Prompt caching enabled.");
                            } else {
                                terminal.print_info(&context, "Prompt caching disabled.");
                            }
                        }
                        ChatCommand::TranscriptPath(path) => {
                            session.config_mut().transcript_path = Some(PathBuf::from(&path));
                            terminal.print_info(
                                &context,
                                &format!("Transcript auto-save set to {}", path),
                            );
                        }
                        ChatCommand::ClearTranscriptPath => {
                            session.config_mut().transcript_path = None;
                            terminal.print_info(&context, "Transcript auto-save disabled.");
                        }
                        ChatCommand::SaveTranscript(path) => {
                            match session.save_transcript_to(&path) {
                                Ok(_) => terminal
                                    .print_info(&context, &format!("Transcript saved to {}", path)),
                                Err(err) => terminal.print_error(
                                    &context,
                                    &format!("Failed to save transcript: {}", err),
                                ),
                            }
                        }
                        ChatCommand::LoadTranscript(path) => {
                            match session.load_transcript_from(&path) {
                                Ok(_) => terminal.print_info(
                                    &context,
                                    &format!("Transcript loaded from {}", path),
                                ),
                                Err(err) => terminal.print_error(
                                    &context,
                                    &format!("Failed to load transcript: {}", err),
                                ),
                            }
                        }
                        ChatCommand::Stats => {
                            print_stats(session);
                        }
                        ChatCommand::ShowConfig => {
                            print_config(session);
                        }
                        ChatCommand::Invalid(message) => {
                            terminal.print_error(&context, &message);
                        }
                    }
                    continue;
                }

                println!("Claude:");
                send_turn(session, terminal, line).await;
            }
            Some(OperatorLine::Interrupted) => {
                println!();
                continue;
            }
            Some(OperatorLine::Eof) => {
                println!("\nGoodbye!");
                break;
            }
            None => break,
        }
    }

    Ok(())
}

/// Print session statistics using claudius-chat formatting.
fn print_stats<A: ChatAgent>(session: &ChatSession<A>) {
    let stats = session.stats();
    println!("    Session Statistics:");
    println!("      Model: {}", stats.model);
    println!("      Messages: {}", stats.message_count);
    println!("      Max tokens: {}", stats.max_tokens);
    println!("      Temperature: {}", describe_float(stats.temperature));
    println!("      Top-p: {}", describe_float(stats.top_p));
    println!("      Top-k: {}", describe_top_k(stats.top_k));
    if let Some(prompt) = stats.system_prompt.as_deref() {
        println!("      System prompt: {}", prompt);
    } else {
        println!("      System prompt: (none)");
    }
    println!(
        "      Thinking: {}",
        match stats.thinking_budget {
            Some(budget) => format!("enabled ({} tokens)", budget),
            None => "disabled".to_string(),
        }
    );
    print_stop_sequences(&stats.stop_sequences);
    println!(
        "      Total tokens: {} in / {} out ({} requests)",
        stats.total_input_tokens, stats.total_output_tokens, stats.total_requests
    );
    if stats.caching_enabled {
        println!(
            "      Cache tokens: {} created / {} read",
            stats.total_cache_creation_tokens, stats.total_cache_read_tokens
        );
    }
    if let Some(input) = stats.last_turn_input_tokens {
        let output = stats.last_turn_output_tokens.unwrap_or(0);
        println!("      Last turn tokens: {input} in / {output} out");
    }
    if let Some(limit) = stats.session_budget_tokens {
        let remaining = limit.saturating_sub(stats.budget_spent_tokens);
        println!(
            "      Budget: {}/{} tokens ({} remaining)",
            stats.budget_spent_tokens, limit, remaining
        );
    } else {
        println!("      Budget: (not set)");
    }
    match stats.transcript_path {
        Some(ref path) => println!("      Transcript file: {}", path.display()),
        None => println!("      Transcript file: (disabled)"),
    }
}

/// Print current configuration using claudius-chat formatting.
fn print_config<A: ChatAgent>(session: &ChatSession<A>) {
    let stats = session.stats();
    println!("    Current Configuration:");
    println!("      Model: {}", stats.model);
    println!("      Max tokens: {}", stats.max_tokens);
    println!("      Temperature: {}", describe_float(stats.temperature));
    println!("      Top-p: {}", describe_float(stats.top_p));
    println!("      Top-k: {}", describe_top_k(stats.top_k));
    println!(
        "      Thinking: {}",
        match stats.thinking_budget {
            Some(budget) => format!("enabled ({} tokens)", budget),
            None => "disabled".to_string(),
        }
    );
    println!(
        "      Caching: {}",
        if stats.caching_enabled {
            "enabled"
        } else {
            "disabled"
        }
    );
    if let Some(prompt) = stats.system_prompt.as_deref() {
        println!("      System prompt: {}", prompt);
    } else {
        println!("      System prompt: (none)");
    }
    print_stop_sequences(&stats.stop_sequences);
    match stats.transcript_path {
        Some(ref path) => println!("      Transcript file: {}", path.display()),
        None => println!("      Transcript file: (disabled)"),
    }
}

/// Print stop sequences using claudius-chat formatting.
fn print_stop_sequences(stop_sequences: &[String]) {
    if stop_sequences.is_empty() {
        println!("      Stop sequences: (none)");
    } else {
        println!("      Stop sequences:");
        for seq in stop_sequences {
            println!("        - {}", seq);
        }
    }
}

/// Describe an optional float value for display.
fn describe_float(value: Option<f32>) -> String {
    value
        .map(|value| format!("{value:.2}"))
        .unwrap_or_else(|| "default".to_string())
}

/// Describe an optional top-k value for display.
fn describe_top_k(value: Option<u32>) -> String {
    value
        .map(|value| value.to_string())
        .unwrap_or_else(|| "default".to_string())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let Some(filesystem_root) = args.get(1).map(|arg| Path::from(arg.as_str()).into_owned()) else {
        eprintln!("{}", USAGE);
        std::process::exit(1);
    };

    let stdin_is_terminal = std::io::stdin().is_terminal();
    let mut stdin = String::new();
    if args.len() <= 2 && !stdin_is_terminal {
        std::io::stdin().read_to_string(&mut stdin)?;
    }
    let initial_prompt = initial_prompt_from_inputs(&args, &stdin, stdin_is_terminal);
    if !stdin_is_terminal && initial_prompt.is_none() {
        eprintln!("{}", USAGE);
        std::process::exit(1);
    }

    let client = Anthropic::new(None)?;
    let agent = EudaemonAgent::new(filesystem_root);
    let mut session = ChatSession::with_agent(client, agent);

    let interrupted = Arc::new(AtomicBool::new(false));
    let interrupted_clone = interrupted.clone();
    ctrlc::set_handler(move || {
        interrupted_clone.store(true, Ordering::Relaxed);
    })?;

    if stdin_is_terminal {
        let mut terminal = ChatTerminal::new(session.config().use_color, interrupted.clone())?;
        if let Some(prompt) = initial_prompt {
            println!("Claude:");
            interrupted.store(false, Ordering::Relaxed);
            send_turn(&mut session, &mut terminal, &prompt).await;
        }
        run_repl(&mut session, &mut terminal, interrupted.as_ref()).await?;
    } else {
        let mut renderer =
            PlainTextRenderer::with_color_and_interrupt(session.config().use_color, interrupted);
        if let Some(prompt) = initial_prompt {
            send_turn(&mut session, &mut renderer, &prompt).await;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initial_prompt_prefers_argv_tail() {
        let args = vec![
            "eudaemoncli".to_string(),
            "kb".to_string(),
            "summarize".to_string(),
            "this".to_string(),
        ];

        assert_eq!(
            initial_prompt_from_inputs(&args, "ignored\n", true),
            Some("summarize this".to_string())
        );
    }

    #[test]
    fn initial_prompt_uses_piped_stdin() {
        let args = vec!["eudaemoncli".to_string(), "kb".to_string()];
        assert_eq!(
            initial_prompt_from_inputs(&args, "  inspect links\n", false),
            Some("inspect links".to_string())
        );
    }

    #[test]
    fn interactive_mode_allows_empty_initial_prompt() {
        let args = vec!["eudaemoncli".to_string(), "kb".to_string()];
        assert_eq!(initial_prompt_from_inputs(&args, "", true), None);
    }

    #[test]
    fn blank_piped_stdin_is_not_a_prompt() {
        let args = vec!["eudaemoncli".to_string(), "kb".to_string()];
        assert_eq!(initial_prompt_from_inputs(&args, "  \n", false), None);
    }

    #[test]
    fn default_chat_config_matches_cli_defaults() {
        let config = default_chat_config();

        assert_eq!(config.model(), Model::Known(KnownModel::ClaudeHaiku45));
        assert_eq!(config.max_tokens(), 32768);
        assert_eq!(config.system_prompt_text(), Some(SYSTEM_PROMPT_TEXT));
    }

    #[tokio::test]
    async fn agent_exposes_text_editor_and_lispdown() {
        let agent = EudaemonAgent::new(Path::from("/").into_owned());
        let tools = agent.tools().await;
        let tool_names: Vec<String> = tools.iter().map(|tool| tool.name()).collect();

        assert_eq!(tools.len(), 2);
        assert!(
            tool_names
                .iter()
                .any(|name| name == "str_replace_based_edit_tool")
        );
        assert!(tool_names.iter().any(|name| name == "lispdown"));
    }

    #[tokio::test]
    async fn bash_is_not_supported() {
        let agent = EudaemonAgent::new(Path::from("/").into_owned());
        let err = Agent::bash(&agent, "pwd", false).await.unwrap_err();

        assert_eq!(err.kind(), std::io::ErrorKind::Unsupported);
    }
}

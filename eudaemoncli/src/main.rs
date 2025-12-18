//! Eudaemon CLI: A command-line interface for running Claude agents on eudaemon images.
//!
//! This binary provides Claude with access to a eudaemon filesystem and shell via the
//! TextEditor and Bash tool types from the claudius crate.

#![deny(missing_docs)]

use std::sync::Arc;
use std::sync::Mutex;

use async_trait::async_trait;
use claudius::Agent;
use claudius::Anthropic;
use claudius::Budget;
use claudius::ContentBlock;
use claudius::Error;
use claudius::FileSystem;
use claudius::Message;
use claudius::Model;
use claudius::ToolBash20250124;
use claudius::ToolTextEditor20250728;
use claudius::ToolUseBlock;

use std::collections::HashMap;
use std::sync::atomic::AtomicBool;

use eudaemonsh::DeviceId;
use eudaemonsh::Environment;
use eudaemonsh::EudaemonFilesystem;
use eudaemonsh::Filesystem;
use eudaemonsh::MemoryLfsExt;
use eudaemonsh::StringStderr;
use eudaemonsh::StringStdin;
use eudaemonsh::StringStdout;
use eudaemonsh::sh;

use utf8path::Path;

/// Persistent shell state that survives across bash invocations.
struct ShellState {
    /// Environment variables (exported, passed to child processes).
    env: HashMap<String, String>,
    /// Shell-local variables (not exported, not passed to child processes).
    vars: HashMap<String, String>,
    /// Current working directory.
    cwd: Path<'static>,
}

impl ShellState {
    /// Creates a new ShellState with default values.
    fn new() -> Self {
        Self {
            env: HashMap::from_iter([
                ("COLUMNS".to_string(), "120".to_string()),
                ("HOME".to_string(), "/home/assistant".to_string()),
                ("PATH".to_string(), "/usr/bin:/bin".to_string()),
                ("PWD".to_string(), "/".to_string()),
                ("SHELL".to_string(), "eudaemonsh".to_string()),
                ("TMPDIR".to_string(), "/tmp".to_string()),
                ("USER".to_string(), "assistant".to_string()),
            ]),
            vars: HashMap::new(),
            cwd: Path::from("/"),
        }
    }
}

/// Time source function returning current time in milliseconds since UNIX epoch.
fn current_time_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Type alias for the EudaemonFilesystem with real time.
type RealTimeFilesystem = EudaemonFilesystem<fn() -> i64>;

/// A FileSystem implementation backed by EudaemonFilesystem.
///
/// This struct wraps the eudaemonsh filesystem and implements the claudius FileSystem
/// trait to provide file operations for the TextEditor tool.
struct EudaemonFileSystem {
    fs: RealTimeFilesystem,
}

impl EudaemonFileSystem {
    /// Creates a new EudaemonFileSystem wrapping the given filesystem.
    fn new(fs: RealTimeFilesystem) -> Self {
        Self { fs }
    }
}

#[async_trait]
impl FileSystem for EudaemonFileSystem {
    async fn search(&self, search: &str) -> Result<String, std::io::Error> {
        // Search for files matching the query by listing directories recursively
        let mut results = Vec::new();
        search_recursive(&self.fs, "/", search, &mut results);
        Ok(results.join("\n"))
    }

    async fn view(
        &self,
        path: &str,
        view_range: Option<(u32, u32)>,
    ) -> Result<String, std::io::Error> {
        let contents = self
            .fs
            .read_to_string(path)
            .map_err(|e| std::io::Error::other(format!("{:?}", e)))?;

        match view_range {
            Some((start, end)) => {
                let lines: Vec<&str> = contents.lines().collect();
                let start = start.saturating_sub(1) as usize; // Convert to 0-indexed
                let end = end as usize;
                let end = end.min(lines.len());
                let selected: Vec<String> = lines
                    .iter()
                    .enumerate()
                    .skip(start)
                    .take(end - start)
                    .map(|(i, line)| format!("{:6}\t{}", i + 1, line))
                    .collect();
                Ok(selected.join("\n"))
            }
            None => {
                let numbered: Vec<String> = contents
                    .lines()
                    .enumerate()
                    .map(|(i, line)| format!("{:6}\t{}", i + 1, line))
                    .collect();
                Ok(numbered.join("\n"))
            }
        }
    }

    async fn str_replace(
        &self,
        path: &str,
        old_str: &str,
        new_str: &str,
    ) -> Result<String, std::io::Error> {
        let contents = self
            .fs
            .read_to_string(path)
            .map_err(|e| std::io::Error::other(format!("{:?}", e)))?;

        // Count occurrences
        let count = contents.matches(old_str).count();
        if count == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "oldString not found in content",
            ));
        }
        if count > 1 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "oldString found multiple times and requires more code context to uniquely identify the intended match",
            ));
        }

        let new_contents = contents.replacen(old_str, new_str, 1);
        self.fs
            .write_string(path, &new_contents)
            .map_err(|e| std::io::Error::other(format!("{:?}", e)))?;

        Ok("success".to_string())
    }

    async fn insert(
        &self,
        path: &str,
        insert_line: u32,
        new_str: &str,
    ) -> Result<String, std::io::Error> {
        let contents = self
            .fs
            .read_to_string(path)
            .map_err(|e| std::io::Error::other(format!("{:?}", e)))?;

        let mut lines: Vec<&str> = contents.lines().collect();
        let insert_idx = insert_line as usize;

        // If inserting beyond the end, pad with empty lines
        while lines.len() < insert_idx {
            lines.push("");
        }

        let new_lines: Vec<&str> = new_str.lines().collect();
        for (i, line) in new_lines.iter().enumerate() {
            lines.insert(insert_idx + i, line);
        }

        let new_contents = lines.join("\n");
        self.fs
            .write_string(path, &new_contents)
            .map_err(|e| std::io::Error::other(format!("{:?}", e)))?;

        Ok("success".to_string())
    }

    async fn create(&self, path: &str, file_text: &str) -> Result<String, std::io::Error> {
        // Check if file exists
        if self.fs.exists(path) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                format!("file already exists: {}", path),
            ));
        }

        // Create parent directories if needed
        let path_obj = Path::new(path);
        let parent = path_obj.dirname();
        let parent_str = parent.as_str();
        if !parent_str.is_empty() && parent_str != "/" && !self.fs.exists(parent_str) {
            self.fs
                .mkdir_all(parent_str)
                .map_err(|e| std::io::Error::other(format!("{:?}", e)))?;
        }

        self.fs
            .write_string(path, file_text)
            .map_err(|e| std::io::Error::other(format!("{:?}", e)))?;

        Ok("success".to_string())
    }
}

/// Recursively search for files matching the query.
fn search_recursive<FS: Filesystem>(fs: &FS, dir: &str, query: &str, results: &mut Vec<String>) {
    if let Ok(entries) = fs.read_dir(dir) {
        for (name, entry) in entries {
            let path = if dir == "/" {
                format!("/{}", name)
            } else {
                format!("{}/{}", dir, name)
            };

            if name.contains(query) || path.contains(query) {
                results.push(path.clone());
            }

            if entry.file_type == eudaemonsh::FileType::Directory {
                search_recursive(fs, &path, query, results);
            }
        }
    }
}

/// The Eudaemon agent that provides filesystem and shell access to Claude.
struct EudaemonAgent {
    filesystem: EudaemonFileSystem,
    fs_for_shell: RealTimeFilesystem,
    shell_state: Mutex<ShellState>,
}

impl EudaemonAgent {
    /// Creates a new EudaemonAgent with the given filesystem size in bytes.
    fn new(fs_size_bytes: usize) -> Self {
        let fs = EudaemonFilesystem::new_memory(
            fs_size_bytes,
            DeviceId::new(1),
            current_time_ms as fn() -> i64,
        )
        .expect("failed to create EudaemonFilesystem");

        // Create the filesystem wrapper and a clone for shell commands
        let filesystem = EudaemonFileSystem::new(fs.dup());
        let fs_for_shell = fs;

        Self {
            filesystem,
            fs_for_shell,
            shell_state: Mutex::new(ShellState::new()),
        }
    }
}

#[async_trait]
impl Agent for EudaemonAgent {
    async fn max_tokens(&self) -> u32 {
        16384
    }

    async fn model(&self) -> Model {
        Model::Known(claudius::KnownModel::ClaudeOpus45)
    }

    async fn tools(&self) -> Vec<Arc<dyn claudius::Tool<Self>>> {
        vec![
            Arc::new(ToolTextEditor20250728 {
                name: "str_replace_based_edit_tool".to_string(),
                cache_control: None,
                max_characters: None,
            }),
            Arc::new(ToolBash20250124 {
                name: "bash".to_string(),
                cache_control: None,
            }),
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

    async fn hook_message(&self, resp: &Message) -> Result<(), Error> {
        for block in &resp.content {
            match block {
                ContentBlock::Text(text_block) => {
                    println!("{}", text_block.text);
                }
                ContentBlock::ToolUse(tool_use) => {
                    println!(
                        "[Tool: {} ({})]\n{}",
                        tool_use.name,
                        tool_use.id,
                        serde_json::to_string_pretty(&tool_use.input).unwrap_or_default()
                    );
                }
                ContentBlock::Thinking(thinking) => {
                    println!("[Thinking: {}]", thinking.thinking);
                }
                _ => {}
            }
        }
        Ok(())
    }

    async fn text_editor(&self, tool_use: ToolUseBlock) -> Result<String, std::io::Error> {
        // Parse the tool input and dispatch to the appropriate filesystem method
        let input = &tool_use.input;

        let command = input
            .get("command")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::InvalidInput, "missing command field")
            })?;

        match command {
            "view" => {
                let path = input.get("path").and_then(|v| v.as_str()).ok_or_else(|| {
                    std::io::Error::new(std::io::ErrorKind::InvalidInput, "missing path field")
                })?;

                let view_range = if let Some(range) = input.get("view_range") {
                    if let Some(arr) = range.as_array() {
                        if arr.len() == 2 {
                            let start = arr[0].as_u64().unwrap_or(1) as u32;
                            let end = arr[1].as_u64().unwrap_or(u32::MAX as u64) as u32;
                            Some((start, end))
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                } else {
                    None
                };

                self.filesystem.view(path, view_range).await
            }
            "str_replace" => {
                let path = input.get("path").and_then(|v| v.as_str()).ok_or_else(|| {
                    std::io::Error::new(std::io::ErrorKind::InvalidInput, "missing path field")
                })?;
                let old_str = input
                    .get("old_str")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| {
                        std::io::Error::new(
                            std::io::ErrorKind::InvalidInput,
                            "missing old_str field",
                        )
                    })?;
                let new_str = input
                    .get("new_str")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| {
                        std::io::Error::new(
                            std::io::ErrorKind::InvalidInput,
                            "missing new_str field",
                        )
                    })?;

                self.filesystem.str_replace(path, old_str, new_str).await
            }
            "insert" => {
                let path = input.get("path").and_then(|v| v.as_str()).ok_or_else(|| {
                    std::io::Error::new(std::io::ErrorKind::InvalidInput, "missing path field")
                })?;
                let insert_line = input
                    .get("insert_line")
                    .and_then(|v| v.as_u64())
                    .ok_or_else(|| {
                        std::io::Error::new(
                            std::io::ErrorKind::InvalidInput,
                            "missing insert_line field",
                        )
                    })? as u32;
                let new_str = input
                    .get("new_str")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| {
                        std::io::Error::new(
                            std::io::ErrorKind::InvalidInput,
                            "missing new_str field",
                        )
                    })?;

                self.filesystem.insert(path, insert_line, new_str).await
            }
            "create" => {
                let path = input.get("path").and_then(|v| v.as_str()).ok_or_else(|| {
                    std::io::Error::new(std::io::ErrorKind::InvalidInput, "missing path field")
                })?;
                let file_text =
                    input
                        .get("file_text")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| {
                            std::io::Error::new(
                                std::io::ErrorKind::InvalidInput,
                                "missing file_text field",
                            )
                        })?;

                self.filesystem.create(path, file_text).await
            }
            other => Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("unknown command: {}", other),
            )),
        }
    }

    async fn bash(&self, command: &str, restart: bool) -> Result<String, std::io::Error> {
        // Reset shell state if restart is requested
        if restart {
            let mut state = self.shell_state.lock().unwrap();
            *state = ShellState::new();
        }

        // Create fresh stdin/stdout/stderr for this command
        let stdin = StringStdin::new("");
        let stdout = StringStdout::new();
        let stderr = StringStderr::new();

        // Create an environment using the persistent shell state
        let mut env = {
            let state = self.shell_state.lock().unwrap();
            Environment {
                stdin,
                stdout: stdout.clone(),
                stderr: stderr.clone(),
                fs: self.fs_for_shell.dup(),
                env: state.env.clone(),
                vars: state.vars.clone(),
                args: vec!["/bin/eudaemonsh".to_string()],
                cwd: state.cwd.clone(),
                exit_signaled: Arc::new(AtomicBool::new(false)),
            }
        };

        // Run the command
        let exit_code = sh::run(command.to_string(), &mut env)
            .map_err(|e| std::io::Error::other(format!("shell error: {:?}", e)))?;

        // Persist the shell state for the next invocation
        {
            let mut state = self.shell_state.lock().unwrap();
            state.env = env.env;
            state.vars = env.vars;
            state.cwd = env.cwd;
        }

        // Collect output
        let stdout_str = stdout.into_string();
        let stderr_str = stderr.into_string();

        let mut result = String::new();
        if !stdout_str.is_empty() {
            result.push_str(&stdout_str);
        }
        if !stderr_str.is_empty() {
            if !result.is_empty() {
                result.push('\n');
            }
            result.push_str("stderr:\n");
            result.push_str(&stderr_str);
        }
        result.push_str(&format!("\nexit code: {}", exit_code.code()));

        Ok(result)
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Create the Anthropic client
    let client = Anthropic::new(None)?;

    // Create the agent with 4MB filesystem (1024 blocks * 4KB)
    let mut agent = EudaemonAgent::new(4 * 1024 * 1024);

    // Create a budget
    let budget = Arc::new(Budget::from_dollars_with_rates(
        1.0,  // $10 budget
        500,  // $5 per million
        2500, // $25 per million
        625,  // $6.25 per million
        50,   // $.50 per million
    ));

    // Create initial messages from command line args or stdin
    let args: Vec<String> = std::env::args().collect();
    let prompt = if args.len() > 1 {
        args[1..].join(" ")
    } else {
        // Read from stdin
        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;
        input.trim().to_string()
    };

    if prompt.is_empty() {
        eprintln!("Usage: eudaemoncli <prompt>");
        std::process::exit(1);
    }

    let mut messages = vec![claudius::MessageParam {
        role: claudius::MessageRole::User,
        content: claudius::MessageParamContent::String(prompt),
    }];

    // Run the conversation
    let stop_reason = agent.take_turn(&client, &mut messages, &budget).await?;

    // Print the final response
    if let Some(last_msg) = messages.last()
        && last_msg.role == claudius::MessageRole::Assistant
    {
        match &last_msg.content {
            claudius::MessageParamContent::String(s) => println!("{}", s),
            claudius::MessageParamContent::Array(blocks) => {
                for block in blocks {
                    if let claudius::ContentBlock::Text(text_block) = block {
                        println!("{}", text_block.text);
                    }
                }
            }
        }
    }

    println!("\n[Stop reason: {:?}]", stop_reason);
    println!(
        "[Remaining budget: ${:.4}]",
        budget.remaining_micro_cents() as f64 / 100_000_000.0
    );

    Ok(())
}

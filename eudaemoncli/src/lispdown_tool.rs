use std::io;
use std::ops::ControlFlow;

use async_trait::async_trait;
use claudius::{
    Anthropic, Error, IntermediateToolResult, Tool, ToolCallback, ToolParam, ToolResult,
    ToolResultBlock, ToolUnionParam, ToolUseBlock,
};
use eudaemonty::{
    DirEntry, DirectoryFilesystem, Error as FsError, FileMetadata, FileType,
    Filesystem as EuFilesystem, TimeSpec,
};
use serde::Deserialize;
use serde_json::json;
use utf8path::Path;

use crate::EudaemonAgent;

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum LispdownAction {
    Eval,
    Help,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct LispdownToolInput {
    action: LispdownAction,
    #[serde(default)]
    expression: Option<String>,
    #[serde(default)]
    topic: Option<String>,
    #[serde(default)]
    working_directory: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct LispdownTool;

impl Tool<EudaemonAgent> for LispdownTool {
    fn name(&self) -> String {
        "lispdown".to_string()
    }

    fn callback(&self) -> Box<dyn ToolCallback<EudaemonAgent> + '_> {
        Box::new(LispdownToolCallback)
    }

    fn to_param(&self) -> ToolUnionParam {
        ToolUnionParam::CustomTool(
            ToolParam::new(
                self.name(),
                json!({
                    "type": "object",
                    "properties": {
                        "action": {
                            "type": "string",
                            "enum": ["eval", "help"],
                            "description": "Use \"eval\" to run a lispdown expression or \"help\" to inspect embedded lispdown documentation."
                        },
                        "expression": {
                            "type": "string",
                            "description": "A single lispdown expression to evaluate when action is \"eval\". Use (begin ...) to sequence multiple steps."
                        },
                        "topic": {
                            "type": "string",
                            "description": "A specific help topic to read when action is \"help\". Omit it to list available topics."
                        },
                        "working_directory": {
                            "type": "string",
                            "description": "Optional rooted virtual directory like \"/\" or \"/notes\" used to resolve relative paths. Defaults to \"/\"."
                        }
                    },
                    "required": ["action"],
                    "additionalProperties": false
                }),
            )
            .with_description(
                "Evaluate lispdown against the same rooted knowledge base exposed by eudaemoncli. \
                 The VM includes core Lisp, JSON, markdown, and filesystem builtins such as \
                 load, save, list-files, fs-read, fs-write, and help. Use this tool for structured \
                 markdown transformations instead of free-form text editing when possible."
                    .to_string(),
            ),
        )
    }
}

struct LispdownToolCallback;

#[async_trait]
impl ToolCallback<EudaemonAgent> for LispdownToolCallback {
    async fn compute_tool_result(
        &self,
        _client: &Anthropic,
        agent: &EudaemonAgent,
        tool_use: &ToolUseBlock,
    ) -> Box<dyn IntermediateToolResult> {
        let input: LispdownToolInput = match serde_json::from_value(tool_use.input.clone()) {
            Ok(input) => input,
            Err(err) => return Box::new(error_tool_result(&tool_use.id, err.to_string())),
        };

        Box::new(
            match execute_lispdown_tool(agent.filesystem.as_str(), &input) {
                Ok(output) => success_tool_result(&tool_use.id, output),
                Err(err) => error_tool_result(&tool_use.id, err),
            },
        )
    }

    async fn apply_tool_result(
        &self,
        _client: &Anthropic,
        _agent: &mut EudaemonAgent,
        _tool_use: &ToolUseBlock,
        intermediate: Box<dyn IntermediateToolResult>,
    ) -> ToolResult {
        let Some(intermediate) = intermediate.as_any().downcast_ref::<ToolResult>() else {
            return ControlFlow::Break(Error::unknown(
                "intermediate tool result fails to deserialize",
            ));
        };
        intermediate.clone()
    }
}

fn success_tool_result(tool_use_id: &str, content: String) -> ToolResult {
    ControlFlow::Continue(Ok(
        ToolResultBlock::new(tool_use_id.to_string()).with_string_content(content)
    ))
}

fn error_tool_result(tool_use_id: &str, message: String) -> ToolResult {
    ControlFlow::Continue(Err(ToolResultBlock::new(tool_use_id.to_string())
        .with_string_content(message)
        .with_error(true)))
}

pub(crate) fn execute_lispdown_tool(
    filesystem_root: &str,
    input: &LispdownToolInput,
) -> Result<String, String> {
    match input.action {
        LispdownAction::Help => execute_help(input),
        LispdownAction::Eval => execute_eval(filesystem_root, input),
    }
}

fn execute_help(input: &LispdownToolInput) -> Result<String, String> {
    let Some(topic) = input
        .topic
        .as_deref()
        .map(str::trim)
        .filter(|topic| !topic.is_empty())
    else {
        return Ok(format_help_topics());
    };

    lispdown::get_help(topic)
        .map(str::to_string)
        .ok_or_else(|| format!("unknown lispdown help topic: {topic}"))
}

fn execute_eval(filesystem_root: &str, input: &LispdownToolInput) -> Result<String, String> {
    let expression = input
        .expression
        .as_deref()
        .map(str::trim)
        .filter(|expression| !expression.is_empty())
        .ok_or_else(|| "expression is required when action is \"eval\"".to_string())?;

    let working_directory =
        normalize_virtual_path("/", input.working_directory.as_deref().unwrap_or("/"))
            .map_err(|err| err.to_string())?;

    let filesystem = DirectoryFilesystem::new(filesystem_root).map_err(|err| err.to_string())?;
    let filesystem = RootedLispdownFilesystem::new(filesystem, &working_directory)
        .map_err(|err| err.to_string())?;
    let mut vm = create_lispdown_vm(filesystem);
    let mut parser = lispdown::Parser::new(expression);
    let expr = parser.parse().map_err(|err| err.to_string())?;
    let result = vm.eval(&expr).map_err(|err| err.to_string())?;
    Ok(result.to_string())
}

fn create_lispdown_vm<FS: EuFilesystem + 'static>(
    filesystem: RootedLispdownFilesystem<FS>,
) -> lispdown::Vm {
    let mut vm = lispdown::Vm::new();
    vm.register_builtins();
    vm.register_json_builtins();
    lispdown::register_markdown_builtins(&mut vm);
    vm.set_filesystem(Box::new(filesystem));
    vm.register_filesystem_builtins();
    vm
}

fn format_help_topics() -> String {
    let mut out = String::from("Available lispdown help topics:\n");
    for (category, topics) in [
        ("builtins", extract_topics(&lispdown::BUILTINS_DOCS)),
        ("json", extract_topics(&lispdown::JSON_DOCS)),
        ("filesystem", extract_topics(&lispdown::FILESYSTEM_DOCS)),
        ("markdown", extract_topics(&lispdown::MARKDOWN_DOCS)),
    ] {
        out.push_str(category);
        out.push_str(": ");
        out.push_str(&topics.join(", "));
        out.push('\n');
    }
    out.push_str("Use action=\"help\" with a topic to read one reference page.");
    out
}

fn extract_topics(docs: &[(&str, &str)]) -> Vec<String> {
    let mut topics: Vec<String> = docs
        .iter()
        .filter_map(|(path, _)| {
            let filename = path.strip_suffix(".md")?;
            let name = filename.rsplit('/').next()?;
            if let Some(base) = name.strip_suffix("-p") {
                Some(format!("{base}?"))
            } else {
                Some(name.to_string())
            }
        })
        .collect();
    topics.sort();
    topics
}

#[derive(Clone, Debug)]
struct RootedLispdownFilesystem<FS> {
    fs: FS,
    cwd: String,
}

impl<FS: EuFilesystem> RootedLispdownFilesystem<FS> {
    fn new(fs: FS, cwd: &str) -> Result<Self, FsError> {
        Ok(Self {
            fs,
            cwd: normalize_virtual_path("/", cwd)?,
        })
    }

    fn resolve(&self, path: &str) -> Result<String, FsError> {
        normalize_virtual_path(&self.cwd, path).map(|path| virtual_to_root_relative(&path))
    }
}

impl<FS: EuFilesystem> EuFilesystem for RootedLispdownFilesystem<FS> {
    fn root(&self) -> Path<'_> {
        Path::new(&self.cwd)
    }

    fn dup(&self) -> Self {
        Self {
            fs: self.fs.dup(),
            cwd: self.cwd.clone(),
        }
    }

    fn read_to_string(&self, path: &str) -> Result<String, FsError> {
        let resolved = self.resolve(path)?;
        self.fs.read_to_string(&resolved)
    }

    fn exists(&self, path: &str) -> bool {
        self.resolve(path)
            .map(|resolved| self.fs.exists(&resolved))
            .unwrap_or(false)
    }

    fn metadata(&self, path: &str) -> Result<FileMetadata, FsError> {
        let resolved = self.resolve(path)?;
        self.fs.metadata(&resolved)
    }

    fn truncate(&self, path: &str, size: u64) -> Result<(), FsError> {
        let resolved = self.resolve(path)?;
        self.fs.truncate(&resolved, size)
    }

    fn truncate_existing(&self, path: &str, size: u64) -> Result<bool, FsError> {
        let resolved = self.resolve(path)?;
        self.fs.truncate_existing(&resolved, size)
    }

    fn punch_hole(&self, path: &str, offset: u64, length: u64) -> Result<(), FsError> {
        let resolved = self.resolve(path)?;
        self.fs.punch_hole(&resolved, offset, length)
    }

    fn write_string(&self, path: &str, contents: &str) -> Result<(), FsError> {
        let resolved = self.resolve(path)?;
        self.fs.write_string(&resolved, contents)
    }

    fn append_string(&self, path: &str, contents: &str) -> Result<(), FsError> {
        let resolved = self.resolve(path)?;
        self.fs.append_string(&resolved, contents)
    }

    fn mkdir(&self, path: &str) -> Result<(), FsError> {
        let resolved = self.resolve(path)?;
        self.fs.mkdir(&resolved)
    }

    fn mkdir_all(&self, path: &str) -> Result<(), FsError> {
        let resolved = self.resolve(path)?;
        self.fs.mkdir_all(&resolved)
    }

    fn is_dir(&self, path: &str) -> bool {
        self.resolve(path)
            .map(|resolved| self.fs.is_dir(&resolved))
            .unwrap_or(false)
    }

    fn read_dir(&self, path: &str) -> Result<Vec<(String, DirEntry)>, FsError> {
        let resolved = self.resolve(path)?;
        self.fs.read_dir(&resolved)
    }

    fn stat(&self, path: &str) -> Result<DirEntry, FsError> {
        let resolved = self.resolve(path)?;
        self.fs.stat(&resolved)
    }

    fn lstat(&self, path: &str) -> Result<DirEntry, FsError> {
        let resolved = self.resolve(path)?;
        self.fs.lstat(&resolved)
    }

    fn symlink(&self, target: &str, linkpath: &str) -> Result<(), FsError> {
        let resolved_linkpath = self.resolve(linkpath)?;
        self.fs.symlink(target, &resolved_linkpath)
    }

    fn link(&self, src: &str, dst: &str) -> Result<(), FsError> {
        let resolved_src = self.resolve(src)?;
        let resolved_dst = self.resolve(dst)?;
        self.fs.link(&resolved_src, &resolved_dst)
    }

    fn unlink(&self, path: &str) -> Result<(), FsError> {
        let resolved = self.resolve(path)?;
        self.fs.unlink(&resolved)
    }

    fn rmdir(&self, path: &str) -> Result<(), FsError> {
        let resolved = self.resolve(path)?;
        self.fs.rmdir(&resolved)
    }

    fn readlink(&self, path: &str) -> Result<String, FsError> {
        let resolved = self.resolve(path)?;
        self.fs.readlink(&resolved)
    }

    fn set_times(&self, path: &str, atime: TimeSpec, mtime: TimeSpec) -> Result<(), FsError> {
        let resolved = self.resolve(path)?;
        self.fs.set_times(&resolved, atime, mtime)
    }

    fn lset_times(&self, path: &str, atime: TimeSpec, mtime: TimeSpec) -> Result<(), FsError> {
        let resolved = self.resolve(path)?;
        self.fs.lset_times(&resolved, atime, mtime)
    }

    fn create_file(&self, path: &str) -> Result<bool, FsError> {
        let resolved = self.resolve(path)?;
        self.fs.create_file(&resolved)
    }

    fn rename(&self, src: &str, dst: &str) -> Result<(), FsError> {
        let resolved_src = self.resolve(src)?;
        let resolved_dst = self.resolve(dst)?;
        self.fs.rename(&resolved_src, &resolved_dst)
    }

    fn mkstemp(&self, template: &str) -> Result<String, FsError> {
        let resolved = self.resolve(template)?;
        self.fs
            .mkstemp(&resolved)
            .map(|path| format!("/{}", path.trim_start_matches('/')))
    }

    fn mkdtemp(&self, template: &str) -> Result<String, FsError> {
        let resolved = self.resolve(template)?;
        self.fs
            .mkdtemp(&resolved)
            .map(|path| format!("/{}", path.trim_start_matches('/')))
    }

    fn list_markdown_files(&self) -> Result<Vec<String>, FsError> {
        let base = virtual_to_root_relative(&self.cwd);
        let mut files = Vec::new();
        find_markdown_recursive(&self.fs, &base, &base, &mut files)?;
        files.sort();
        Ok(files)
    }
}

fn normalize_virtual_path(cwd: &str, path: &str) -> Result<String, FsError> {
    let mut components = base_components(cwd)?;
    let path = if path.is_empty() { "." } else { path };

    if path.starts_with('/') {
        components.clear();
    }

    for component in path.split('/') {
        match component {
            "" | "." => {}
            ".." => {
                if components.pop().is_none() {
                    return Err(FsError::Io(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "paths may not escape the rooted filesystem",
                    )));
                }
            }
            component => components.push(component.to_string()),
        }
    }

    if components.is_empty() {
        Ok("/".to_string())
    } else {
        Ok(format!("/{}", components.join("/")))
    }
}

fn base_components(cwd: &str) -> Result<Vec<String>, FsError> {
    if cwd.is_empty() || cwd == "/" {
        return Ok(Vec::new());
    }
    if !cwd.starts_with('/') {
        return Err(FsError::Io(io::Error::new(
            io::ErrorKind::InvalidInput,
            "working_directory must be rooted",
        )));
    }

    let mut components = Vec::new();
    for component in cwd.split('/') {
        match component {
            "" | "." => {}
            ".." => {
                if components.pop().is_none() {
                    return Err(FsError::Io(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "paths may not escape the rooted filesystem",
                    )));
                }
            }
            component => components.push(component.to_string()),
        }
    }
    Ok(components)
}

fn virtual_to_root_relative(path: &str) -> String {
    path.trim_start_matches('/').to_string()
}

fn find_markdown_recursive<FS: EuFilesystem>(
    fs: &FS,
    base: &str,
    path: &str,
    results: &mut Vec<String>,
) -> Result<(), FsError> {
    let entries = fs.read_dir(path)?;

    for (name, entry) in entries {
        if name == "." || name == ".." {
            continue;
        }
        let full_path = if path.is_empty() {
            name.clone()
        } else {
            format!("{path}/{name}")
        };

        if entry.file_type == FileType::Directory {
            find_markdown_recursive(fs, base, &full_path, results)?;
        } else if name.ends_with(".md") || name.ends_with(".MD") {
            let rel_path = full_path
                .strip_prefix(base)
                .unwrap_or(&full_path)
                .trim_start_matches('/');
            results.push(rel_path.to_string());
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    fn make_temp_dir() -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!(
            "eudaemoncli-lispdown-{}-{}",
            std::process::id(),
            unique
        ));
        fs::create_dir_all(&dir).expect("failed to create temp dir");
        dir
    }

    #[test]
    fn help_without_topic_lists_categories() {
        let dir = make_temp_dir();
        let input = LispdownToolInput {
            action: LispdownAction::Help,
            expression: None,
            topic: None,
            working_directory: None,
        };

        let output = execute_lispdown_tool(dir.to_str().unwrap(), &input).unwrap();
        assert!(output.contains("builtins"));
        assert!(output.contains("markdown"));

        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn eval_can_write_markdown_into_the_rooted_filesystem() {
        let dir = make_temp_dir();
        let input = LispdownToolInput {
            action: LispdownAction::Eval,
            expression: Some(
                "(save (markdown-to-sexpr \"# Title\\n\\nBody\") \"generated.md\")".to_string(),
            ),
            topic: None,
            working_directory: None,
        };

        let output = execute_lispdown_tool(dir.to_str().unwrap(), &input).unwrap();
        assert!(output.contains("generated.md"));
        let saved = fs::read_to_string(dir.join("generated.md")).unwrap();
        assert!(saved.contains("# Title"));
        assert!(saved.contains("Body"));

        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn eval_respects_the_virtual_working_directory() {
        let dir = make_temp_dir();
        fs::create_dir_all(dir.join("notes")).unwrap();
        fs::write(dir.join("notes/todo.md"), "# TODO").unwrap();
        let input = LispdownToolInput {
            action: LispdownAction::Eval,
            expression: Some("(list-files)".to_string()),
            topic: None,
            working_directory: Some("/notes".to_string()),
        };

        let output = execute_lispdown_tool(dir.to_str().unwrap(), &input).unwrap();
        assert!(output.contains("todo.md"));
        assert!(!output.contains("notes/todo.md"));

        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn eval_rejects_paths_that_escape_the_root() {
        let dir = make_temp_dir();
        let input = LispdownToolInput {
            action: LispdownAction::Eval,
            expression: Some("(fs-read \"../secret.md\")".to_string()),
            topic: None,
            working_directory: Some("/".to_string()),
        };

        let err = execute_lispdown_tool(dir.to_str().unwrap(), &input).unwrap_err();
        assert!(err.contains("escape"));

        fs::remove_dir_all(dir).unwrap();
    }
}

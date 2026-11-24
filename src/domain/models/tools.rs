use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;
use uuid::Uuid;

const MAX_ARGUMENT_BYTES: usize = 16 * 1024;
pub const MAX_RESULT_BYTES: usize = 32 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolError {
    InvalidRequest,
    InvalidResult,
    NoPendingCall,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolName {
    InvokeSubagent,
    ViewFile,
    GrepSearch,
    ReplaceFileContent,
    RunCommand,
}

impl ToolName {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::InvokeSubagent => "invoke_subagent",
            Self::ViewFile => "view_file",
            Self::GrepSearch => "grep_search",
            Self::ReplaceFileContent => "replace_file_content",
            Self::RunCommand => "run_command",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ToolRequest {
    pub call_id: Uuid,
    pub name: ToolName,
    pub arguments: Value,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct SubagentArgs {
    role: String,
    task: String,
    context: Option<String>,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ViewFileArgs {
    path: String,
    start_line: Option<u32>,
    end_line: Option<u32>,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct GrepArgs {
    query: String,
    paths: Vec<String>,
    max_results: Option<u8>,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ReplaceArgs {
    path: String,
    find: String,
    replace: String,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct CommandArgs {
    cwd: String,
    argv: Vec<String>,
}

fn bounded_string(value: &str, max: usize) -> bool {
    !value.trim().is_empty() && value.len() <= max && !value.contains('\0')
}

impl ToolRequest {
    pub fn from_provider(name: &str, arguments: Value) -> Result<Self, ToolError> {
        if arguments.to_string().len() > MAX_ARGUMENT_BYTES {
            return Err(ToolError::InvalidRequest);
        }
        let (name, arguments) = match name {
            "invoke_subagent" => {
                let args: SubagentArgs =
                    serde_json::from_value(arguments).map_err(|_| ToolError::InvalidRequest)?;
                if !bounded_string(&args.role, 80)
                    || !bounded_string(&args.task, 8 * 1024)
                    || args.context.as_ref().is_some_and(|s| s.len() > 8 * 1024)
                {
                    return Err(ToolError::InvalidRequest);
                }
                (
                    ToolName::InvokeSubagent,
                    serde_json::to_value(args).map_err(|_| ToolError::InvalidRequest)?,
                )
            }
            "view_file" => {
                let args: ViewFileArgs =
                    serde_json::from_value(arguments).map_err(|_| ToolError::InvalidRequest)?;
                let first_line = args.start_line.unwrap_or(1);
                if !bounded_string(&args.path, 1024)
                    || args
                        .end_line
                        .is_some_and(|end| end < first_line || end.saturating_sub(first_line) > 500)
                {
                    return Err(ToolError::InvalidRequest);
                }
                (
                    ToolName::ViewFile,
                    serde_json::to_value(args).map_err(|_| ToolError::InvalidRequest)?,
                )
            }
            "grep_search" => {
                let args: GrepArgs =
                    serde_json::from_value(arguments).map_err(|_| ToolError::InvalidRequest)?;
                if !bounded_string(&args.query, 4096)
                    || args.paths.is_empty()
                    || args.paths.len() > 32
                    || args.paths.iter().any(|s| !bounded_string(s, 1024))
                    || args.max_results.is_some_and(|n| n == 0 || n > 100)
                {
                    return Err(ToolError::InvalidRequest);
                }
                (
                    ToolName::GrepSearch,
                    serde_json::to_value(args).map_err(|_| ToolError::InvalidRequest)?,
                )
            }
            "replace_file_content" => {
                let args: ReplaceArgs =
                    serde_json::from_value(arguments).map_err(|_| ToolError::InvalidRequest)?;
                if !bounded_string(&args.path, 1024)
                    || args.find.is_empty()
                    || args.find.len() > 16 * 1024
                    || args.replace.len() > 16 * 1024
                {
                    return Err(ToolError::InvalidRequest);
                }
                (
                    ToolName::ReplaceFileContent,
                    serde_json::to_value(args).map_err(|_| ToolError::InvalidRequest)?,
                )
            }
            "run_command" => {
                let args: CommandArgs =
                    serde_json::from_value(arguments).map_err(|_| ToolError::InvalidRequest)?;
                if !bounded_string(&args.cwd, 1024)
                    || args.argv.is_empty()
                    || args.argv.len() > 64
                    || args.argv.iter().any(|s| s.len() > 4096 || s.contains('\0'))
                {
                    return Err(ToolError::InvalidRequest);
                }
                (
                    ToolName::RunCommand,
                    serde_json::to_value(args).map_err(|_| ToolError::InvalidRequest)?,
                )
            }
            _ => return Err(ToolError::InvalidRequest),
        };
        Ok(Self {
            call_id: Uuid::now_v7(),
            name,
            arguments,
        })
    }
}

#[derive(Debug, Clone)]
pub struct ToolResult {
    pub call_id: Uuid,
    pub name: ToolName,
    pub output: String,
}

#[derive(Debug, Clone)]
pub struct ToolExchange {
    pub request: ToolRequest,
    pub output: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PendingTool {
    pub call_id: Uuid,
    pub name: ToolName,
}

#[derive(Clone)]
pub struct ToolControl {
    pending: Arc<Mutex<Option<PendingTool>>>,
    result_tx: mpsc::UnboundedSender<ToolResult>,
}

impl ToolControl {
    pub fn new(result_tx: mpsc::UnboundedSender<ToolResult>) -> Self {
        Self {
            pending: Arc::new(Mutex::new(None)),
            result_tx,
        }
    }

    pub fn set_pending(&self, pending: PendingTool) -> bool {
        let mut slot = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        if slot.is_some() {
            return false;
        }
        *slot = Some(pending);
        true
    }

    pub fn clear_pending(&self, call_id: Uuid) {
        let mut slot = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        if slot.is_some_and(|pending| pending.call_id == call_id) {
            *slot = None;
        }
    }

    pub fn submit(&self, result: ToolResult) -> Result<(), ToolError> {
        if result.output.len() > MAX_RESULT_BYTES {
            return Err(ToolError::InvalidResult);
        }
        let mut slot = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        if !slot
            .is_some_and(|pending| pending.call_id == result.call_id && pending.name == result.name)
        {
            return Err(ToolError::NoPendingCall);
        }
        *slot = None;
        self.result_tx
            .send(result)
            .map_err(|_| ToolError::NoPendingCall)
    }
}

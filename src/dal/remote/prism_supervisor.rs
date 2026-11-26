//! Manages the staged on-demand Prism host service (`prism.service`) and the active profile.
//! The service is unprivileged, loopback-only, unauthenticated, and never started at boot.

use crate::{
    core::config::Config,
    domain::models::{driver::DriverError, model_profile::ModelProfile, usage::Pending},
};
use sha2::{Digest, Sha256};
use std::{path::Path, process::Stdio, sync::Arc, time::Duration};
use tokio::{
    io::AsyncReadExt,
    process::Command,
    sync::{Mutex, watch},
};

const SERVER_SHA256: &str = "89e7f20b23b97bf77b91a0143a168763f7835fd6e0a7a7a23bb70951d991c02c";
pub const ALLOWED_UNIT: &str = "prism.service";

pub fn clear_runtime_api_auth_env(command: &mut Command) {
    command
        .env_remove("LLAMA_API_KEY")
        .env_remove("LLAMA_ARG_API_KEY_FILE");
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UnitStatus {
    pub load_state: String,
    pub active_state: String,
    pub sub_state: String,
    pub main_pid: u32,
    pub control_group: String,
    pub invocation_id: String,
}

impl UnitStatus {
    pub fn is_active(&self) -> bool {
        self.active_state == "active"
            && self.sub_state == "running"
            && self.main_pid > 0
            && !self.invocation_id.is_empty()
    }

    pub fn is_inactive(&self) -> bool {
        (self.active_state == "inactive"
            || self.active_state == "failed"
            || self.load_state == "not-found")
            && self.main_pid == 0
    }
}

pub fn parse_unit_status(output: &str) -> Result<UnitStatus, DriverError> {
    let mut status = UnitStatus::default();
    for line in output.lines() {
        if let Some((key, val)) = line.split_once('=') {
            match key.trim() {
                "LoadState" => status.load_state = val.trim().to_string(),
                "ActiveState" => status.active_state = val.trim().to_string(),
                "SubState" => status.sub_state = val.trim().to_string(),
                "MainPID" => status.main_pid = val.trim().parse::<u32>().unwrap_or(0),
                "ControlGroup" => status.control_group = val.trim().to_string(),
                "InvocationID" => status.invocation_id = val.trim().to_string(),
                _ => {}
            }
        }
    }
    Ok(status)
}

pub trait SystemdRunner: Send + Sync {
    fn start_unit<'a>(&'a self, unit: &'a str) -> Pending<'a, Result<(), DriverError>>;
    fn stop_unit<'a>(&'a self, unit: &'a str) -> Pending<'a, Result<(), DriverError>>;
    fn show_unit<'a>(&'a self, unit: &'a str) -> Pending<'a, Result<UnitStatus, DriverError>>;
}

pub struct SystemctlRunner {
    systemctl_path: std::path::PathBuf,
    timeout: Duration,
}

impl Default for SystemctlRunner {
    fn default() -> Self {
        Self {
            systemctl_path: std::path::PathBuf::from("/usr/bin/systemctl"),
            timeout: Duration::from_secs(15),
        }
    }
}

impl SystemctlRunner {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_path(path: std::path::PathBuf) -> Self {
        Self {
            systemctl_path: path,
            timeout: Duration::from_secs(15),
        }
    }

    pub fn with_path_and_timeout(path: std::path::PathBuf, timeout: Duration) -> Self {
        Self {
            systemctl_path: path,
            timeout,
        }
    }
}

impl SystemdRunner for SystemctlRunner {
    fn start_unit<'a>(&'a self, unit: &'a str) -> Pending<'a, Result<(), DriverError>> {
        Box::pin(async move {
            if unit != ALLOWED_UNIT {
                return Err(DriverError::InvalidInput);
            }
            let mut command = Command::new(&self.systemctl_path);
            command.arg("--no-ask-password");
            command.args(["start", ALLOWED_UNIT]);
            command.stdin(Stdio::null());
            command.stdout(Stdio::null());
            command.stderr(Stdio::piped());
            command.kill_on_drop(true);
            let mut child = command.spawn().map_err(|_| DriverError::Ollama)?;
            let status = match tokio::time::timeout(self.timeout, child.wait()).await {
                Ok(Ok(status)) => status,
                Ok(Err(_)) => return Err(DriverError::Ollama),
                Err(_) => {
                    let _ = child.kill().await;
                    let _ = child.wait().await;
                    return Err(DriverError::Ollama);
                }
            };
            if !status.success() {
                return Err(DriverError::Ollama);
            }
            Ok(())
        })
    }

    fn stop_unit<'a>(&'a self, unit: &'a str) -> Pending<'a, Result<(), DriverError>> {
        Box::pin(async move {
            if unit != ALLOWED_UNIT {
                return Err(DriverError::InvalidInput);
            }
            let mut command = Command::new(&self.systemctl_path);
            command.arg("--no-ask-password");
            command.args(["stop", ALLOWED_UNIT]);
            command.stdin(Stdio::null());
            command.stdout(Stdio::null());
            command.stderr(Stdio::piped());
            command.kill_on_drop(true);
            let mut child = command.spawn().map_err(|_| DriverError::Ollama)?;
            let status = match tokio::time::timeout(self.timeout, child.wait()).await {
                Ok(Ok(status)) => status,
                Ok(Err(_)) => return Err(DriverError::Ollama),
                Err(_) => {
                    let _ = child.kill().await;
                    let _ = child.wait().await;
                    return Err(DriverError::Ollama);
                }
            };
            if !status.success() {
                return Err(DriverError::Ollama);
            }
            Ok(())
        })
    }

    fn show_unit<'a>(&'a self, unit: &'a str) -> Pending<'a, Result<UnitStatus, DriverError>> {
        Box::pin(async move {
            if unit != ALLOWED_UNIT {
                return Err(DriverError::InvalidInput);
            }
            let mut command = Command::new(&self.systemctl_path);
            command.arg("--no-ask-password");
            command.args([
                "show",
                ALLOWED_UNIT,
                "--property=LoadState,ActiveState,SubState,MainPID,ControlGroup,InvocationID",
            ]);
            command.stdin(Stdio::null());
            command.stdout(Stdio::piped());
            command.stderr(Stdio::piped());
            command.kill_on_drop(true);
            let mut child = command.spawn().map_err(|_| DriverError::Ollama)?;
            let mut stdout = child.stdout.take().ok_or(DriverError::Ollama)?;
            let timeout_duration = std::cmp::min(self.timeout, Duration::from_secs(10));
            let outcome = tokio::time::timeout(timeout_duration, async {
                let mut text = Vec::new();
                stdout.read_to_end(&mut text).await?;
                let status = child.wait().await?;
                Ok::<_, std::io::Error>((status, text))
            })
            .await;
            let (status, text) = match outcome {
                Ok(Ok((status, text))) => (status, text),
                Ok(Err(_)) => return Err(DriverError::Ollama),
                Err(_) => {
                    let _ = child.kill().await;
                    let _ = child.wait().await;
                    return Err(DriverError::Ollama);
                }
            };
            if !status.success() {
                return Err(DriverError::Ollama);
            }
            let text = String::from_utf8(text).map_err(|_| DriverError::Ollama)?;
            parse_unit_status(&text)
        })
    }
}

#[derive(Debug, Clone)]
pub struct ActiveService {
    pub profile: ModelProfile,
    pub pid: u32,
    pub invocation_id: String,
    pub control_group: String,
    pub stopping: bool,
}

pub struct PrismSupervisor {
    runner: Arc<dyn SystemdRunner>,
    pub(crate) child: Mutex<Option<ActiveService>>,
    #[cfg(test)]
    pub(crate) skip_hash_for_test: bool,
}

impl Default for PrismSupervisor {
    fn default() -> Self {
        Self {
            runner: Arc::new(SystemctlRunner::new()),
            child: Mutex::new(None),
            #[cfg(test)]
            skip_hash_for_test: false,
        }
    }
}

impl PrismSupervisor {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_runner(runner: Arc<dyn SystemdRunner>) -> Self {
        Self {
            runner,
            child: Mutex::new(None),
            #[cfg(test)]
            skip_hash_for_test: false,
        }
    }

    pub async fn stop(&self) -> Result<(), DriverError> {
        let mut slot = self.child.lock().await;
        self.stop_locked(&mut slot).await
    }

    async fn stop_quiescent(&self) -> Result<(), DriverError> {
        self.runner.stop_unit(ALLOWED_UNIT).await?;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        loop {
            let status = self.runner.show_unit(ALLOWED_UNIT).await?;
            if status.is_inactive() {
                return Ok(());
            }
            if tokio::time::Instant::now() > deadline {
                return Err(DriverError::Ollama);
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    async fn stop_locked(&self, slot: &mut Option<ActiveService>) -> Result<(), DriverError> {
        if let Some(active) = slot.as_mut() {
            active.stopping = true;
        }

        if slot.is_none() {
            let status = self.runner.show_unit(ALLOWED_UNIT).await?;
            if status.is_inactive() {
                return Ok(());
            }
        }

        let res = self.stop_quiescent().await;
        if res.is_ok() {
            *slot = None;
        }
        res
    }

    pub async fn is_running(&self, profile: ModelProfile) -> Result<bool, DriverError> {
        let slot = self.child.lock().await;
        let Some(active) = slot.as_ref() else {
            return Ok(false);
        };
        if active.profile != profile || active.stopping {
            return Ok(false);
        }
        let status = self.runner.show_unit(ALLOWED_UNIT).await?;
        if status.is_active()
            && status.main_pid == active.pid
            && status.invocation_id == active.invocation_id
        {
            Ok(true)
        } else {
            Ok(false)
        }
    }

    pub async fn ensure(
        &self,
        config: &Config,
        profile: ModelProfile,
        cancel: &mut watch::Receiver<bool>,
    ) -> Result<(), DriverError> {
        if !config.prism_url_is_loopback() {
            return Err(DriverError::InvalidInput);
        }
        if profile != ModelProfile::Bonsai25Uncensored && profile != ModelProfile::Bonsai25Official
        {
            return Err(DriverError::InvalidInput);
        }
        let mut slot = self.child.lock().await;
        let current_status = self.runner.show_unit(ALLOWED_UNIT).await?;
        if let Some(active) = slot.as_ref()
            && active.profile == profile
            && !active.stopping
            && current_status.is_active()
            && current_status.main_pid == active.pid
            && current_status.invocation_id == active.invocation_id
        {
            return Ok(());
        }

        // Before writing selector or starting, require the actual unit to be confirmed quiescent.
        // Even if slot is None (e.g. fresh supervisor instance), any existing running, activating,
        // or stale unit must be stopped and verified inactive.
        if !current_status.is_inactive() || slot.is_some() {
            self.stop_locked(&mut slot).await?;
        }

        #[cfg(test)]
        let skip_hash = self.skip_hash_for_test;
        #[cfg(not(test))]
        let skip_hash = false;

        if !skip_hash {
            let filename = profile
                .artifact_filename()
                .ok_or(DriverError::InvalidInput)?;
            let expected_model_hash = profile.artifact_sha256().ok_or(DriverError::InvalidInput)?;
            let runtime = config.prism_server_path.clone();
            let model = config.bonsai_model_directory.join(filename);
            let runtime_hash =
                tokio::time::timeout(Duration::from_secs(90), sha256(&runtime, cancel))
                    .await
                    .map_err(|_| DriverError::Ollama)??;
            let model_hash = tokio::time::timeout(Duration::from_secs(180), sha256(&model, cancel))
                .await
                .map_err(|_| DriverError::Ollama)??;
            if runtime_hash != SERVER_SHA256 || model_hash != expected_model_hash {
                return Err(DriverError::Ollama);
            }
        }

        if *cancel.borrow() {
            return Err(DriverError::Cancelled);
        }

        // Selector file update only happens after old unit is confirmed inactive
        write_profile_atomic(&config.prism_profile_path, profile).await?;

        if *cancel.borrow() {
            return Err(DriverError::Cancelled);
        }

        // Track starting state before starting so any subsequent error or cancel cleans up
        *slot = Some(ActiveService {
            profile,
            pid: 0,
            invocation_id: String::new(),
            control_group: String::new(),
            stopping: false,
        });

        if let Err(e) = self.runner.start_unit(ALLOWED_UNIT).await {
            let _ = self.stop_locked(&mut slot).await;
            return Err(e);
        }

        // Poll until active with valid MainPID and InvocationID
        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        let status = loop {
            if *cancel.borrow() {
                self.stop_locked(&mut slot).await?;
                return Err(DriverError::Cancelled);
            }
            let status = match self.runner.show_unit(ALLOWED_UNIT).await {
                Ok(s) => s,
                Err(e) => {
                    let _ = self.stop_locked(&mut slot).await;
                    return Err(e);
                }
            };
            if status.is_active() {
                break status;
            }
            if tokio::time::Instant::now() > deadline {
                self.stop_locked(&mut slot).await?;
                return Err(DriverError::Ollama);
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        };

        *slot = Some(ActiveService {
            profile,
            pid: status.main_pid,
            invocation_id: status.invocation_id,
            control_group: status.control_group,
            stopping: false,
        });

        Ok(())
    }
}

pub async fn write_profile_atomic(path: &Path, profile: ModelProfile) -> Result<(), DriverError> {
    if profile != ModelProfile::Bonsai25Uncensored && profile != ModelProfile::Bonsai25Official {
        return Err(DriverError::InvalidInput);
    }
    let parent = path.parent().ok_or(DriverError::InvalidInput)?;
    if !tokio::fs::try_exists(parent).await.unwrap_or(false) {
        let _ = tokio::fs::create_dir_all(parent).await;
    }
    let tmp_path = parent.join(format!(".profile.{}.tmp", uuid::Uuid::new_v4()));
    let content = format!("{}\n", profile.id());
    if tokio::fs::write(&tmp_path, content.as_bytes())
        .await
        .is_err()
    {
        let _ = tokio::fs::remove_file(&tmp_path).await;
        return Err(DriverError::Ollama);
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if tokio::fs::set_permissions(&tmp_path, std::fs::Permissions::from_mode(0o640))
            .await
            .is_err()
        {
            let _ = tokio::fs::remove_file(&tmp_path).await;
            return Err(DriverError::Ollama);
        }
    }

    if tokio::fs::rename(&tmp_path, path).await.is_err() {
        let _ = tokio::fs::remove_file(&tmp_path).await;
        return Err(DriverError::Ollama);
    }

    Ok(())
}

impl super::prism::PrismProcessControl for PrismSupervisor {
    fn ensure<'a>(
        &'a self,
        config: &'a Config,
        profile: ModelProfile,
        cancel: &'a mut watch::Receiver<bool>,
    ) -> Pending<'a, Result<(), DriverError>> {
        Box::pin(async move { PrismSupervisor::ensure(self, config, profile, cancel).await })
    }

    fn stop<'a>(&'a self) -> Pending<'a, Result<(), DriverError>> {
        Box::pin(async move { PrismSupervisor::stop(self).await })
    }

    fn is_running<'a>(&'a self, profile: ModelProfile) -> Pending<'a, Result<bool, DriverError>> {
        Box::pin(async move { PrismSupervisor::is_running(self, profile).await })
    }
}

async fn sha256(path: &Path, cancel: &mut watch::Receiver<bool>) -> Result<String, DriverError> {
    let mut file = tokio::fs::File::open(path)
        .await
        .map_err(|_| DriverError::Ollama)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        let count = tokio::select! {
            biased;
            _ = cancelled(cancel) => return Err(DriverError::Cancelled),
            count = file.read(&mut buffer) => count.map_err(|_| DriverError::Ollama)?,
        };
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

async fn cancelled(cancel: &mut watch::Receiver<bool>) {
    loop {
        if *cancel.borrow() {
            return;
        }
        if cancel.changed().await.is_err() {
            std::future::pending::<()>().await;
        }
    }
}

#[cfg(test)]
#[cfg(unix)]
fn process_group_exists(pid: u32) -> bool {
    // SAFETY: signal 0 only checks the existence of the process group.
    if unsafe { libc::kill(-(pid as i32), 0) } == 0 {
        return true;
    }
    std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
}

#[cfg(test)]
#[cfg(not(unix))]
fn process_group_exists(_pid: u32) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::{
        dal::remote::{model_router::ModelRouter, ollama::Ollama, prism::Prism},
        domain::{
            models::{
                context::{AssembledContext, ContextInput, ContextText},
                driver::{CallInput, CallOutcome, DriverEvent, ModelDriver, OutputModel},
                identity::Caller,
                model_profile::ModelProfile,
                thinking::{ThinkingInput, ThinkingResult},
                tools::{PendingTool, ToolControl, ToolName, ToolResult},
                usage::{
                    CallUsage, LedgerUnavailable, Pending, Share, Usage, UsageLedger, WaitingCall,
                },
            },
            services::balancer::Scheduler,
        },
    };
    use chrono::{DateTime, Utc};
    use futures_util::StreamExt;
    use std::sync::Arc;
    use tokio::sync::{mpsc, watch};

    struct EvaluationLedger;
    impl UsageLedger for EvaluationLedger {
        fn share<'a>(
            &'a self,
            _: &'a str,
            _: DateTime<Utc>,
        ) -> Pending<'a, Result<Share, LedgerUnavailable>> {
            Box::pin(async {
                Ok(Share {
                    usage: Usage::default(),
                    weight: 1.0,
                    loyalty: None,
                })
            })
        }
        fn record<'a>(&'a self, _: CallUsage) -> Pending<'a, Result<(), LedgerUnavailable>> {
            Box::pin(async { Ok(()) })
        }
        fn waiting<'a>(
            &'a self,
            _: Vec<WaitingCall>,
        ) -> Pending<'a, Result<(), LedgerUnavailable>> {
            Box::pin(async { Ok(()) })
        }
    }

    fn eval_input(
        prompt: &str,
        images: Vec<String>,
        prefix: Option<String>,
        allow_tools: bool,
    ) -> CallInput {
        CallInput {
            role: "answer".into(),
            system: "Be accurate, concise, and follow the requested language and format.".into(),
            prompt: prompt.into(),
            images,
            temperature: 0.0,
            max_tokens: 256,
            context_tokens: 8192,
            prefix,
            allow_tools,
            tool_history: vec![],
        }
    }

    fn cortex_eval_context(
        config: &Config,
        question: &str,
        files: Vec<ContextText>,
    ) -> AssembledContext {
        crate::domain::services::context::assemble(
            config,
            ContextInput {
                context_tokens: Some(8192),
                output_tokens: Some(256),
                question: question.into(),
                files,
                ..Default::default()
            },
        )
        .expect("production Cortex context assembly with bounded synthetic evidence")
    }

    fn synthetic_evidence(label: &str, text: &str) -> ContextText {
        assert!(text.len() <= 1024, "synthetic evidence must stay bounded");
        ContextText {
            label: label.into(),
            source: Some("synthetic-eval-fixture".into()),
            text: text.into(),
        }
    }

    async fn evaluation_thinking_answer(
        router: &Arc<ModelRouter>,
        config: &Config,
        output: &OutputModel,
        context: &AssembledContext,
        prefix: Option<String>,
    ) -> CallOutcome {
        let thinking = crate::domain::services::thinking::Thinking::new(config, router.clone());
        let input = ThinkingInput {
            model: output,
            context,
            images: vec![],
            prefix,
            tools_enabled: false,
            budget: None,
        };
        let (events, receiver, event_task) = crate::domain::services::events::channel(config, None);
        let (cancel_tx, cancel) = watch::channel(false);
        let answer_state = ThinkingResult::default();
        let result = {
            let call = thinking.answer(&input, &answer_state, &events, cancel);
            tokio::pin!(call);
            let deadline = tokio::time::sleep(Duration::from_secs(240));
            tokio::pin!(deadline);
            let mut monitor = tokio::time::interval(Duration::from_secs(5));
            loop {
                tokio::select! {
                    result = &mut call => break result,
                    _ = monitor.tick() => {
                        if evaluation_resources_ok(700).is_err() {
                            let _ = cancel_tx.send(true);
                        }
                    }
                    _ = &mut deadline => {
                        let _ = cancel_tx.send(true);
                        break tokio::time::timeout(Duration::from_secs(60), &mut call)
                            .await.expect("bounded Thinking::answer cancellation");
                    }
                }
            }
        };
        drop(events);
        drop(receiver);
        event_task.await.expect("bounded event output task");
        result.expect("real Thinking::answer provider call")
    }

    async fn evaluation_thinking_tool_double(
        router: &Arc<ModelRouter>,
        config: &Config,
        output: &OutputModel,
        context: &AssembledContext,
    ) -> (CallOutcome, Vec<PendingTool>) {
        let thinking = crate::domain::services::thinking::Thinking::new(config, router.clone());
        let input = ThinkingInput {
            model: output,
            context,
            images: vec![],
            prefix: None,
            tools_enabled: true,
            budget: None,
        };
        let (events, receiver, event_task) = crate::domain::services::events::channel(config, None);
        let (tool_tx, mut tool_results) = mpsc::unbounded_channel();
        let tool_control = ToolControl::new(tool_tx);
        let (cancel_tx, cancel) = watch::channel(false);
        let mut requests = Vec::new();
        let answer_state = ThinkingResult::default();
        let result = {
            let call = thinking.answer_with_tools(
                &input,
                &answer_state,
                &events,
                cancel,
                &tool_control,
                &mut tool_results,
            );
            tokio::pin!(call);
            let deadline = tokio::time::sleep(Duration::from_secs(240));
            tokio::pin!(deadline);
            let mut monitor = tokio::time::interval(Duration::from_secs(5));
            let mut poll_tools = tokio::time::interval(Duration::from_millis(20));
            loop {
                tokio::select! {
                    result = &mut call => break result,
                    _ = poll_tools.tick() => {
                        if let Some(request) = tool_control.pending()
                            && !requests.iter().any(|seen: &PendingTool| seen.call_id == request.call_id)
                        {
                            eprintln!("EVAL {}", serde_json::json!({"case":"synthetic-tool-request","name":request.name.as_str(),"arguments":request.arguments}));
                            assert_eq!(request.name, ToolName::ViewFile, "reject unexpected tool before fixture response");
                            assert!(
                                is_evaluation_fixture_tool(&request),
                                "reject unexpected tool arguments before fixture response"
                            );
                            requests.push(request.clone());
                            tool_control.submit(ToolResult {
                                call_id: request.call_id,
                                name: request.name,
                                output: "Synthetic fixture content: job KITE-204 status READY, retry_count 2.".into(),
                            }).expect("respond only to a pending synthetic tool request");
                        }
                    }
                    _ = monitor.tick() => {
                        if evaluation_resources_ok(700).is_err() {
                            let _ = cancel_tx.send(true);
                        }
                    }
                    _ = &mut deadline => {
                        let _ = cancel_tx.send(true);
                        break tokio::time::timeout(Duration::from_secs(60), &mut call)
                            .await.expect("bounded synthetic tool-call cancellation");
                    }
                }
            }
        };
        drop(events);
        drop(receiver);
        event_task.await.expect("bounded event output task");
        (
            result.expect("real Thinking::answer_with_tools provider call"),
            requests,
        )
    }

    fn is_evaluation_fixture_tool(request: &PendingTool) -> bool {
        if request.name != ToolName::ViewFile {
            return false;
        }
        let Some(object) = request.arguments.as_object() else {
            return false;
        };
        if object
            .keys()
            .any(|key| !["path", "start_line", "end_line"].contains(&key.as_str()))
            || object.get("path") != Some(&serde_json::json!("/evaluation-only/kite-204.txt"))
        {
            return false;
        }
        ["start_line", "end_line"].into_iter().all(|key| {
            object
                .get(key)
                .is_none_or(|value| value.is_null() || value.as_u64().is_some_and(|line| line > 0))
        })
    }

    #[test]
    fn synthetic_tool_gate_accepts_normalized_view_file_args_only() {
        use crate::domain::models::tools::ToolRequest;

        let request = |name: &str, args: serde_json::Value| {
            let parsed = ToolRequest::from_provider(name, args).expect("valid provider tool args");
            PendingTool {
                call_id: uuid::Uuid::new_v4(),
                name: parsed.name,
                arguments: parsed.arguments,
            }
        };

        let path_only = request(
            "view_file",
            serde_json::json!({"path":"/evaluation-only/kite-204.txt"}),
        );
        assert_eq!(path_only.arguments["start_line"], serde_json::Value::Null);
        assert_eq!(path_only.arguments["end_line"], serde_json::Value::Null);
        assert!(is_evaluation_fixture_tool(&path_only));

        let ranged = request(
            "view_file",
            serde_json::json!({"path":"/evaluation-only/kite-204.txt","start_line":1,"end_line":2}),
        );
        assert!(is_evaluation_fixture_tool(&ranged));

        let wrong_path = request("view_file", serde_json::json!({"path":"/etc/passwd"}));
        assert!(!is_evaluation_fixture_tool(&wrong_path));
        let wrong_tool = request(
            "grep_search",
            serde_json::json!({"query":"x","paths":["/evaluation-only/kite-204.txt"]}),
        );
        assert!(!is_evaluation_fixture_tool(&wrong_tool));
    }

    fn middle_word_prefix(text: &str) -> Option<String> {
        let midpoint = text.len() / 2;
        let split = text
            .char_indices()
            .filter(|(_, character)| character.is_whitespace())
            .map(|(index, _)| index)
            .min_by_key(|index| index.abs_diff(midpoint))?;
        let prefix = text[..split].trim_end();
        (!prefix.is_empty() && prefix.len() < text.len()).then(|| prefix.to_owned())
    }

    #[test]
    fn middle_word_prefix_uses_utf8_word_boundary() {
        let answer = "naïve Rust explains values clearly";
        let prefix = middle_word_prefix(answer).expect("whitespace boundary in answer");
        assert!(answer.starts_with(&prefix));
        assert!(prefix.len() < answer.len());
        assert!(
            answer[prefix.len()..]
                .chars()
                .next()
                .is_some_and(char::is_whitespace)
        );
    }

    fn evaluation_resources_ok(min_gpu_free_mib: u64) -> Result<(), String> {
        let mem = std::fs::read_to_string("/proc/meminfo").map_err(|e| e.to_string())?;
        let available_kib = mem
            .lines()
            .find_map(|line| {
                line.strip_prefix("MemAvailable:")
                    .and_then(|value| value.split_whitespace().next())
                    .and_then(|value| value.parse::<u64>().ok())
            })
            .ok_or("MemAvailable missing")?;
        if available_kib < 4 * 1024 * 1024 {
            return Err(format!("MemAvailable below 4 GiB: {available_kib} KiB"));
        }
        let load = std::fs::read_to_string("/proc/loadavg")
            .map_err(|e| e.to_string())?
            .split_whitespace()
            .next()
            .and_then(|value| value.parse::<f64>().ok())
            .ok_or("loadavg missing")?;
        if load > 24.0 {
            return Err(format!("load average too high: {load}"));
        }
        let gpu = std::process::Command::new("timeout")
            .args([
                "5s",
                "nvidia-smi",
                "--query-gpu=memory.free",
                "--format=csv,noheader,nounits",
            ])
            .output()
            .map_err(|e| e.to_string())?;
        if !gpu.status.success() {
            return Err(format!(
                "nvidia-smi failed: {}",
                String::from_utf8_lossy(&gpu.stderr)
            ));
        }
        let free_mib = String::from_utf8_lossy(&gpu.stdout)
            .trim()
            .parse::<u64>()
            .map_err(|e| e.to_string())?;
        if free_mib < min_gpu_free_mib {
            return Err(format!(
                "GPU free VRAM below {min_gpu_free_mib} MiB: {free_mib} MiB"
            ));
        }
        eprintln!("GUARD mem_available_kib={available_kib} load1={load} gpu_free_mib={free_mib}");
        Ok(())
    }

    async fn ollama_loaded(config: &Config) -> Result<Vec<String>, String> {
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(3))
            .timeout(Duration::from_secs(10))
            .build()
            .map_err(|e| e.to_string())?;
        let response = client
            .get(format!(
                "{}/api/ps",
                config.ollama_url.trim_end_matches('/')
            ))
            .send()
            .await
            .map_err(|e| e.to_string())?;
        if !response.status().is_success() {
            return Err(format!("Ollama /api/ps status {}", response.status()));
        }
        let mut stream = response.bytes_stream();
        let mut bytes = Vec::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| e.to_string())?;
            if bytes.len().saturating_add(chunk.len()) > 64 * 1024 {
                return Err("Ollama /api/ps response exceeded 64 KiB".into());
            }
            bytes.extend_from_slice(&chunk);
        }
        let value: serde_json::Value = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
        Ok(value
            .get("models")
            .and_then(serde_json::Value::as_array)
            .ok_or("Ollama models missing")?
            .iter()
            .filter_map(|item| {
                item.get("name")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned)
            })
            .collect())
    }

    async fn unload_owned_t0(config: &Config) -> Result<bool, String> {
        let loaded = ollama_loaded(config).await?;
        if loaded.len() != 1 || (loaded[0] != config.text_model && loaded[0] != config.vision_model)
        {
            return Ok(false);
        }
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(3))
            .timeout(Duration::from_secs(10))
            .build()
            .map_err(|e| e.to_string())?;
        let response = client
            .post(format!(
                "{}/api/generate",
                config.ollama_url.trim_end_matches('/')
            ))
            .json(&serde_json::json!({"model":loaded[0],"keep_alive":0,"stream":false,"prompt":""}))
            .send()
            .await
            .map_err(|e| e.to_string())?;
        if !response.status().is_success() {
            return Err(format!("Ollama unload status {}", response.status()));
        }
        for _ in 0..30 {
            if ollama_loaded(config).await?.is_empty() {
                return Ok(true);
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        Err("owned T0 model did not unload within 30 seconds".into())
    }

    fn evaluation_output(profile: ModelProfile, has_images: bool) -> OutputModel {
        OutputModel::for_profile(
            &Caller {
                app_id: "bonsai-eval".into(),
                user_id: Some("local-eval".into()),
                is_admin: false,
            },
            uuid::Uuid::now_v7(),
            profile,
            has_images,
        )
    }

    fn process_start_ticks(pid: u32) -> Option<String> {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        stat.rsplit_once(')')?
            .1
            .split_whitespace()
            .nth(19)
            .map(str::to_owned)
    }

    fn assert_measured_answer(outcome: &crate::domain::models::driver::CallOutcome) {
        assert!(
            outcome.error.is_none(),
            "provider returned an answer error: {:?}",
            outcome.error
        );
        assert!(
            outcome.usage_measured,
            "provider usage must include both prompt and completion tokens"
        );
        assert!(
            outcome.usage.input_tokens > 0 && outcome.usage.output_tokens > 0,
            "measured input/output token counts must be nonzero"
        );
        let ttft = outcome
            .usage
            .time_to_first_token_ms
            .expect("answer TTFT is available");
        assert!(
            ttft <= outcome.usage.total_ms,
            "TTFT cannot exceed total elapsed time"
        );
        assert!(
            outcome
                .tokens_per_second
                .is_some_and(|value| value.is_finite() && value > 0.0),
            "decode throughput must be measurable for a nonempty answer"
        );
        let inspection = outcome
            .inspection
            .as_ref()
            .expect("answer inspection is present");
        assert!(
            inspection.matches(&outcome.text),
            "inspection/logprobs must cover the complete answer; reason={:?}",
            inspection.reason
        );
    }

    async fn evaluation_model_router_setup()
    -> (Config, Scheduler, Arc<PrismSupervisor>, Arc<ModelRouter>) {
        evaluation_resources_ok(7600)
            .expect("safe host resources before model use; MTP requires headroom");
        let available = std::net::TcpListener::bind("127.0.0.1:0").expect("free loopback port");
        let port = available.local_addr().unwrap().port();
        drop(available);
        let config = Config {
            bonsai_enabled: true,
            prism_server_path: std::env::var("CORTEX_TEST_PRISM_SERVER")
                .expect("runner pins CORTEX_TEST_PRISM_SERVER")
                .into(),
            bonsai_model_directory: std::env::var("CORTEX_TEST_BONSAI_DIR")
                .expect("runner pins CORTEX_TEST_BONSAI_DIR")
                .into(),
            ollama_url: std::env::var("CORTEX_TEST_OLLAMA_URL")
                .expect("runner pins CORTEX_TEST_OLLAMA_URL"),
            prism_url: format!("http://127.0.0.1:{port}"),
            ..Config::default()
        };
        assert!(config.prism_url_is_loopback());
        assert!(
            ollama_loaded(&config)
                .await
                .expect("Ollama preflight")
                .is_empty(),
            "pre-existing Ollama models: do not unload another user's work"
        );
        let apps = std::process::Command::new("timeout")
            .args([
                "5s",
                "nvidia-smi",
                "--query-compute-apps=pid,name",
                "--format=csv,noheader",
            ])
            .output()
            .expect("nvidia-smi compute-process preflight");
        assert!(
            apps.status.success(),
            "nvidia-smi failed: {}",
            String::from_utf8_lossy(&apps.stderr)
        );
        assert!(
            String::from_utf8_lossy(&apps.stdout).trim().is_empty(),
            "GPU compute process already active; do not disturb it"
        );

        let queue = Scheduler::new(Arc::new(EvaluationLedger));
        let supervisor = Arc::new(PrismSupervisor::new());
        let ollama = Arc::new(
            Ollama::new(config.clone(), queue.clone()).with_prism_supervisor(supervisor.clone()),
        );
        let prism = Arc::new(Prism::managed(
            config.clone(),
            queue.clone(),
            supervisor.clone(),
        ));
        let router = Arc::new(ModelRouter::new(ollama, prism, queue.clone()));
        (config, queue, supervisor, router)
    }

    async fn evaluation_cancellation_case(
        router: &Arc<ModelRouter>,
        queue: &Scheduler,
        supervisor: &Arc<PrismSupervisor>,
        profile: ModelProfile,
        owned_t0: &mut bool,
    ) {
        let cancel_output = evaluation_output(profile, false);
        let (events, mut event_rx) = mpsc::unbounded_channel();
        let (cancel_tx, cancel_rx) = watch::channel(false);
        let cancel_call = router.call(
            &cancel_output,
            eval_input(
                "Write a detailed explanation with many examples of Rust ownership and borrowing.",
                vec![],
                None,
                false,
            ),
            events,
            cancel_rx,
        );
        tokio::pin!(cancel_call);
        let mut saw_cancel_token = false;
        let mut saw_model_loading = false;
        let mut saw_model_ready = false;
        let mut saw_queued_wait = false;
        let mut t1_events_open = true;
        let mut t0_events_open = true;
        let mut queued_registered = false;
        let mut pressure_detected = false;
        let mut cancel_result = None;
        let mut queued_result = None;
        let mut observation_monitor = tokio::time::interval(Duration::from_secs(5));
        // First establish that the selected T1 profile really owns the slot
        // and emits a token. Only then enqueue T0; otherwise a scheduler race
        // can make the test observe T0 before Prism ever starts.
        let t1_observation = tokio::time::timeout(Duration::from_secs(180), async {
            while !saw_cancel_token && cancel_result.is_none() {
                tokio::select! {
                    biased;
                    event = event_rx.recv(), if t1_events_open => {
                        match event {
                            Some(DriverEvent::Token { .. }) => saw_cancel_token = true,
                            Some(DriverEvent::ModelLoading { .. }) => saw_model_loading = true,
                            Some(DriverEvent::ModelReady { .. }) => saw_model_ready = true,
                            None => t1_events_open = false,
                            _ => {}
                        }
                    }
                    result = &mut cancel_call, if cancel_result.is_none() => cancel_result = Some(result),
                    _ = observation_monitor.tick() => {
                        if evaluation_resources_ok(700).is_err() {
                            pressure_detected = true;
                            let _ = cancel_tx.send(true);
                            break;
                        }
                    }
                }
            }
        })
        .await;

        let t1_ready = t1_observation.is_ok()
            && !pressure_detected
            && saw_cancel_token
            && cancel_result.is_none();
        eprintln!(
            "EVAL {}",
            serde_json::json!({"case":"cancellation-t1-observation","profile":profile.id(),"model_loading_event":saw_model_loading,"model_ready_event":saw_model_ready,"t1_token_observed":saw_cancel_token,"observation_timed_out":t1_observation.is_err(),"resource_pressure":pressure_detected,"t1_finished_before_token":cancel_result.is_some(),"t1_result":cancel_result.as_ref().map(|result|format!("{:?}",result.as_ref().err())),"t1_partial":cancel_result.as_ref().and_then(|result|result.as_ref().ok()).map(|outcome|outcome.text.as_str())})
        );
        if !t1_ready {
            let _ = cancel_tx.send(true);
            if cancel_result.is_none() {
                match tokio::time::timeout(Duration::from_secs(90), cancel_call.as_mut()).await {
                    Ok(result) => cancel_result = Some(result),
                    Err(_) => {
                        supervisor
                            .stop()
                            .await
                            .expect("bounded owned Prism stop after missing T1 observation");
                        cancel_result = Some(
                            tokio::time::timeout(Duration::from_secs(60), cancel_call.as_mut())
                                .await
                                .expect("settle T1 after owned Prism teardown"),
                        );
                    }
                }
            }
            eprintln!(
                "EVAL {}",
                serde_json::json!({"case":"cancellation-t1-settlement","profile":profile.id(),"result":cancel_result.as_ref().map(|result|format!("{:?}",result.as_ref().err()))})
            );
            assert!(
                t1_ready,
                "T1 must emit a token and remain active before starting queued T0"
            );
        }

        *owned_t0 = true;
        let queued_output = evaluation_output(ModelProfile::CortexT0, false);
        let (queued_events, mut queued_event_rx) = mpsc::unbounded_channel();
        let (queued_cancel_tx, queued_cancel_rx) = watch::channel(false);
        let queued_call = router.call(
            &queued_output,
            eval_input(
                "In one sentence, define Rust ownership.",
                vec![],
                None,
                false,
            ),
            queued_events,
            queued_cancel_rx,
        );
        tokio::pin!(queued_call);
        let queued_observation = tokio::time::timeout(Duration::from_secs(30), async {
            while !saw_queued_wait && queued_result.is_none() {
                tokio::select! {
                    biased;
                    event = queued_event_rx.recv(), if !saw_queued_wait && t0_events_open => {
                        match event {
                            Some(DriverEvent::Waiting) => {
                                saw_queued_wait = true;
                                queued_registered = queue.waiting_count() > 0;
                            }
                            None => t0_events_open = false,
                            _ => {}
                        }
                    }
                    result = &mut queued_call, if queued_result.is_none() => queued_result = Some(result),
                    result = &mut cancel_call, if cancel_result.is_none() => cancel_result = Some(result),
                    event = event_rx.recv(), if !saw_cancel_token && t1_events_open => {
                        match event {
                            Some(DriverEvent::Token { .. }) => saw_cancel_token = true,
                            Some(DriverEvent::ModelLoading { .. }) => saw_model_loading = true,
                            Some(DriverEvent::ModelReady { .. }) => saw_model_ready = true,
                            None => t1_events_open = false,
                            _ => {}
                        }
                    }
                    _ = observation_monitor.tick() => {
                        if evaluation_resources_ok(700).is_err() {
                            pressure_detected = true;
                            let _ = cancel_tx.send(true);
                            let _ = queued_cancel_tx.send(true);
                            break;
                        }
                    }
                }
            }
        })
        .await;
        let t1_still_active_when_t0_waited = cancel_result.is_none();
        eprintln!(
            "EVAL {}",
            serde_json::json!({"case":"cancellation-queue-observation","profile":profile.id(),"model_loading_event":saw_model_loading,"model_ready_event":saw_model_ready,"t1_token_observed":saw_cancel_token,"t0_waiting_observed":saw_queued_wait,"t0_registered_in_scheduler":queued_registered,"t1_still_active_when_t0_waited":t1_still_active_when_t0_waited,"t0_observation_timed_out":queued_observation.is_err(),"resource_pressure":pressure_detected,"t1_finished_before_queue_observed":!t1_still_active_when_t0_waited,"t1_result":cancel_result.as_ref().map(|result|format!("{:?}",result.as_ref().err())),"t0_finished_before_wait_observed":queued_result.is_some(),"t0_result":queued_result.as_ref().map(|result|format!("{:?}",result.as_ref().err()))})
        );
        let _ = cancel_tx.send(true);
        if pressure_detected {
            let _ = queued_cancel_tx.send(true);
        }
        let drain = async {
            if cancel_result.is_none() {
                cancel_result = Some(cancel_call.as_mut().await);
            }
            if queued_result.is_none() {
                queued_result = Some(queued_call.as_mut().await);
            }
        };
        tokio::pin!(drain);
        let mut drain_monitor = tokio::time::interval(Duration::from_secs(5));
        let drained = tokio::time::timeout(Duration::from_secs(90), async {
            loop {
                tokio::select! {
                    _ = &mut drain => break,
                    _ = drain_monitor.tick() => {
                        if evaluation_resources_ok(700).is_err() {
                            pressure_detected = true;
                            let _ = cancel_tx.send(true);
                            let _ = queued_cancel_tx.send(true);
                            eprintln!("GUARD_STOP resource pressure while settling cancelled T1 and queued T0");
                        }
                    }
                }
            }
        })
        .await
        .is_ok();
        if !drained {
            eprintln!(
                "GUARD_STOP cancellation/queued-T0 drain exceeded 90 seconds; stopping only the owned Prism child"
            );
            supervisor
                .stop()
                .await
                .expect("bounded owned Prism stop after drain timeout");
            tokio::time::timeout(Duration::from_secs(60), async {
                if cancel_result.is_none() {
                    cancel_result = Some(cancel_call.as_mut().await);
                }
                if queued_result.is_none() {
                    queued_result = Some(queued_call.as_mut().await);
                }
            })
            .await
            .expect("bounded model-future settlement after owned Prism stop");
        }
        let partial_result = cancel_result.expect("cancel call result collected");
        let queued_result = queued_result.expect("queued call result collected");
        eprintln!(
            "EVAL {}",
            serde_json::json!({"case":"cancellation-settlement","profile":profile.id(),"t1_result":format!("{:?}",partial_result.as_ref().err()),"t0_result":format!("{:?}",queued_result.as_ref().err()),"t1_partial":partial_result.as_ref().ok().map(|outcome|outcome.text.as_str()),"t0_text":queued_result.as_ref().ok().map(|outcome|outcome.text.as_str()),"drain_timed_out":!drained,"resource_pressure":pressure_detected})
        );
        assert!(
            queued_observation.is_ok()
                && !pressure_detected
                && saw_cancel_token
                && saw_queued_wait
                && queued_registered
                && t1_still_active_when_t0_waited,
            "T1 token must precede an actually registered queued T0 before cancellation"
        );
        let partial = partial_result.expect("partial result survives cancellation");
        assert_eq!(
            partial.error,
            Some(crate::domain::models::driver::DriverError::Cancelled)
        );
        assert!(!partial.text.is_empty());
        assert!(partial.tool_request.is_none());
        assert!(
            supervisor.child.lock().await.is_none(),
            "owned Prism must be reaped before permit release"
        );
        let queued = queued_result.expect("queued T0 runs after Prism teardown");
        assert!(queued.error.is_none());
        eprintln!(
            "EVAL {}",
            serde_json::json!({"case":"cancel-then-queued-t0","profile":profile.id(),"partial_text":partial.text,"partial_error":"cancelled","queued_t0_text":queued.text,"queued_after_teardown":true})
        );
    }

    #[tokio::test]
    #[ignore = "requires explicit global heavy/GPU locks; real local models and runtime"]
    async fn gpu_model_router_acceptance_uncensored_official_t0_and_cancel() {
        use futures_util::FutureExt;
        let (config, queue, supervisor, router) = evaluation_model_router_setup().await;
        let mut owned_t0 = false;
        let result = std::panic::AssertUnwindSafe(async {
            let mut quality_failures: Vec<String> = Vec::new();
            let uncensored = evaluation_output(ModelProfile::Bonsai25Uncensored, false);
            let original_question =
                "According to the synthetic record for KITE-204, what status and retry count are recorded? Answer in one sentence.";
            let original_context = cortex_eval_context(
                &config,
                original_question,
                vec![synthetic_evidence(
                    "KITE-204 synthetic record",
                    "Synthetic record KITE-204: status READY; retry_count 2.",
                )],
            );
            let cold = evaluation_thinking_answer(
                &router,
                &config,
                &uncensored,
                &original_context,
                None,
            )
            .await;
            eprintln!("EVAL {}", serde_json::json!({"case":"uncensored-en-cold","profile":uncensored.model_id(),"text":cold.text,"error":cold.error.as_ref().map(|error| format!("{error:?}")),"ttft_ms":cold.usage.time_to_first_token_ms,"input_tokens":cold.usage.input_tokens,"output_tokens":cold.usage.output_tokens,"usage_measured":cold.usage_measured,"total_ms":cold.usage.total_ms,"tokens_per_second":cold.tokens_per_second}));
            assert_measured_answer(&cold);
            if !(cold.text.to_lowercase().contains("ready") && cold.text.contains('2')) {
                quality_failures.push("uncensored EN factual answer missed READY or retry_count=2".into());
            }

            let distractor_question = "According to this record, what are the status, retry count, and maintainer? If a field is absent, say so without guessing.";
            let distractor_context = cortex_eval_context(
                &config,
                distractor_question,
                vec![synthetic_evidence(
                    "KITE-204 record with absent field",
                    "Synthetic record KITE-204: status READY; retry_count 2; maintainer field not recorded.",
                )],
            );
            let distractor = evaluation_thinking_answer(
                &router,
                &config,
                &uncensored,
                &distractor_context,
                None,
            ).await;
            eprintln!("EVAL {}", serde_json::json!({"case":"uncensored-grounding-unsupported-field","text":distractor.text,"error":distractor.error.as_ref().map(|error|format!("{error:?}")),"usage_measured":distractor.usage_measured}));
            assert_measured_answer(&distractor);
            let unsupported_ack = ["not recorded", "not specified", "not provided", "unknown", "absent", "no maintainer"]
                .iter().any(|phrase| distractor.text.to_lowercase().contains(*phrase));
            if !(distractor.text.to_lowercase().contains("ready")
                && distractor.text.contains('2')
                && unsupported_ack)
            {
                quality_failures.push("uncensored answer failed to ground supported facts and acknowledge the absent maintainer field".into());
            }
            let first_pid = supervisor.child.lock().await.as_ref().expect("owned uncensored child").pid;
            let first_start = process_start_ticks(first_pid).expect("read owned Prism process identity");

            let saved_prefix = middle_word_prefix(&cold.text)
                .expect("generated English answer has a valid middle word boundary");
            let prefix = evaluation_thinking_answer(
                &router,
                &config,
                &uncensored,
                &original_context,
                Some(saved_prefix.clone()),
            )
            .await;
            let continued_answer = format!("{saved_prefix}{}", prefix.text);
            eprintln!("EVAL {}", serde_json::json!({"case":"uncensored-prefix","saved_prefix":saved_prefix,"continuation_text":prefix.text,"combined_text":continued_answer,"error":prefix.error.as_ref().map(|error| format!("{error:?}")),"ttft_ms":prefix.usage.time_to_first_token_ms,"output_tokens":prefix.usage.output_tokens,"usage_measured":prefix.usage_measured,"total_ms":prefix.usage.total_ms,"tokens_per_second":prefix.tokens_per_second}));
            assert_measured_answer(&prefix);
            if !(continued_answer.to_lowercase().contains("ready") && continued_answer.contains('2')) {
                quality_failures.push("uncensored continuation omitted supported READY or retry_count=2".into());
            }
            assert_eq!(supervisor.child.lock().await.as_ref().expect("same owned child").pid, first_pid,
                "healthy same-profile calls reuse one process");
            assert_eq!(process_start_ticks(first_pid).as_deref(), Some(first_start.as_str()),
                "same-profile calls preserve the original process instance");

            let json_context = cortex_eval_context(
                &config,
                "From the synthetic ledger, return only JSON with boolean key ok and integer key count.",
                vec![synthetic_evidence(
                    "JSON fixture",
                    "Synthetic ledger entry: ok=true; count=3.",
                )],
            );
            let json = evaluation_thinking_answer(
                &router,
                &config,
                &uncensored,
                &json_context,
                None,
            )
            .await;
            assert_measured_answer(&json);
            let json_ok = serde_json::from_str::<serde_json::Value>(json.text.trim())
                .is_ok_and(|parsed| parsed["ok"] == true && parsed["count"] == 3);
            eprintln!("EVAL {}", serde_json::json!({"case":"uncensored-json","text":json.text,"valid_expected_json":json_ok,"usage_measured":json.usage_measured}));
            if !json_ok { quality_failures.push("uncensored JSON did not match the requested schema and values".into()); }

            let tool_context = cortex_eval_context(
                &config,
                "Use the view_file tool to read the synthetic path /evaluation-only/kite-204.txt, then state the record's status and retry count.",
                vec![synthetic_evidence(
                    "Tool fixture index",
                    "The requested file path is synthetic and exists only in the evaluator's fake tool response.",
                )],
            );
            let (tool, tool_requests) = evaluation_thinking_tool_double(
                &router,
                &config,
                &uncensored,
                &tool_context,
            )
            .await;
            assert_eq!(tool_requests.len(), 1, "one synthetic tool request expected");
            assert_eq!(tool_requests[0].name, ToolName::ViewFile);
            assert_eq!(tool_requests[0].arguments["path"], "/evaluation-only/kite-204.txt");
            assert!(tool.usage_measured && tool.usage.output_tokens > 0 && tool.usage.total_ms > 0);
            eprintln!("EVAL {}", serde_json::json!({"case":"uncensored-tool-double","tool":tool_requests[0].name.as_str(),"arguments":tool_requests[0].arguments,"effects":"synthetic-only","text":tool.text,"usage_measured":tool.usage_measured}));
            if !(tool.text.to_lowercase().contains("ready") && tool.text.contains('2')) { quality_failures.push("uncensored tool answer omitted fixture status or retry count".into()); }

            for (language, question, evidence) in [
                (
                    "en",
                    "According to the synthetic evidence, when is a Rust Option value absent? Answer in one sentence.",
                    "In Rust, Option<T> is None when the optional value is absent, and Some(value) when it is present.",
                ),
                (
                    "pt-BR",
                    "Segundo a evidência sintética, quando um valor Option de Rust está ausente? Responda em uma frase.",
                    "Em Rust, Option<T> é None quando o valor opcional está ausente e Some(valor) quando está presente.",
                ),
                (
                    "th",
                    "ตอบเป็นภาษาไทยเท่านั้น: ตามหลักฐานสังเคราะห์ Option ของ Rust หมายถึงค่าไม่อยู่เมื่อใด",
                    "ใน Rust, Option<T> เป็น None เมื่อค่าไม่มี และเป็น Some(value) เมื่อค่ามีอยู่",
                ),
            ] {
                let context = cortex_eval_context(
                    &config,
                    question,
                    vec![synthetic_evidence("Language fact fixture", evidence)],
                );
                let outcome = evaluation_thinking_answer(
                    &router,
                    &config,
                    &uncensored,
                    &context,
                    None,
                )
                .await;
                assert_measured_answer(&outcome);
                let han = outcome.text.chars().filter(|c| ('\u{4e00}'..='\u{9fff}').contains(c)).count();
                let thai = outcome.text.chars().filter(|c| ('\u{0e00}'..='\u{0e7f}').contains(c)).count();
                assert!(outcome.error.is_none());
                eprintln!("EVAL {}", serde_json::json!({"case":"atomic-language","language":language,"han_codepoints":han,"thai_codepoints":thai,"text":outcome.text,"known_thai_mixed_chinese_is_quality_failure":language=="th" && han>0}));
                let language_ok = !outcome.text.trim().is_empty()
                    && (outcome.text.contains("None") || language == "th")
                    && (language != "th" || (han == 0 && thai > 0));
                if !language_ok { quality_failures.push(format!("uncensored {language} answer failed the raw-language/content criteria")); }
            }
            let uncensored_pt_context = cortex_eval_context(
                &config,
                "Segundo a evidência sintética, qual é o status do registro KITE-204 e qual é a contagem de tentativas? Responda em português brasileiro.",
                vec![synthetic_evidence("Registro KITE-204", "Registro sintético KITE-204: status READY; retry_count 2.")],
            );
            let uncensored_pt = evaluation_thinking_answer(&router, &config, &uncensored, &uncensored_pt_context, None).await;
            assert_measured_answer(&uncensored_pt);
            eprintln!("EVAL {}", serde_json::json!({"case":"uncensored-pt-br","text":uncensored_pt.text,"usage_measured":uncensored_pt.usage_measured}));
            let pt = uncensored_pt.text.to_lowercase();
            if !(pt.contains("ready") && uncensored_pt.text.contains('2') && (pt.contains("status") || pt.contains("situação"))) {
                quality_failures.push("uncensored PT-BR answer missed the supported status/count or language cue".into());
            }

            let official = evaluation_output(ModelProfile::Bonsai25Official, false);
            let answer = evaluation_thinking_answer(
                &router,
                &config,
                &official,
                &original_context,
                None,
            )
            .await;
            assert_measured_answer(&answer);
            let official_child = supervisor.child.lock().await;
            let official_pid = official_child.as_ref().expect("owned official child").pid;
            assert_eq!(official_child.as_ref().unwrap().profile, ModelProfile::Bonsai25Official);
            let official_start = process_start_ticks(official_pid).expect("read switched Prism process identity");
            assert!(official_pid != first_pid || official_start != first_start,
                "profile switch must replace the owned process, even if the OS reuses its PID");
            if official_pid != first_pid {
                assert!(!process_group_exists(first_pid), "previous Prism process group must be gone");
            }
            eprintln!("EVAL {}", serde_json::json!({"case":"official-en-switch","profile":official.model_id(),"pid":official_pid,"ttft_ms":answer.usage.time_to_first_token_ms,"total_ms":answer.usage.total_ms,"text":answer.text,"error":answer.error.as_ref().map(|error|format!("{error:?}"))}));
            if !(answer.text.to_lowercase().contains("ready") && answer.text.contains('2')) {
                quality_failures.push("official EN factual answer missed READY or retry_count=2".into());
            }
            drop(official_child);
            let official_saved_prefix = middle_word_prefix(&answer.text)
                .expect("official generated answer has a valid middle word boundary");
            let official_prefix = evaluation_thinking_answer(
                &router,
                &config,
                &official,
                &original_context,
                Some(official_saved_prefix.clone()),
            )
            .await;
            let official_continued = format!("{official_saved_prefix}{}", official_prefix.text);
            eprintln!("EVAL {}", serde_json::json!({"case":"official-prefix","saved_prefix":official_saved_prefix,"continuation_text":official_prefix.text,"combined_text":official_continued,"error":official_prefix.error.as_ref().map(|error|format!("{error:?}")),"ttft_ms":official_prefix.usage.time_to_first_token_ms,"usage_measured":official_prefix.usage_measured}));
            assert_measured_answer(&official_prefix);
            if !(official_continued.to_lowercase().contains("ready") && official_continued.contains('2')) {
                quality_failures.push("official continuation omitted supported READY or retry_count=2".into());
            }
            assert_eq!(supervisor.child.lock().await.as_ref().expect("official same-profile child").pid, official_pid);
            let (official_tool, official_tool_requests) = evaluation_thinking_tool_double(
                &router,
                &config,
                &official,
                &tool_context,
            )
            .await;
            assert_eq!(official_tool_requests.len(), 1);
            assert_eq!(official_tool_requests[0].name, ToolName::ViewFile);
            assert_eq!(official_tool_requests[0].arguments["path"], "/evaluation-only/kite-204.txt");
            assert!(official_tool.usage_measured && official_tool.usage.output_tokens > 0);
            eprintln!("EVAL {}", serde_json::json!({"case":"official-tool-double","tool":official_tool_requests[0].name.as_str(),"arguments":official_tool_requests[0].arguments,"effects":"synthetic-only","text":official_tool.text,"usage_measured":official_tool.usage_measured}));
            if !(official_tool.text.to_lowercase().contains("ready") && official_tool.text.contains('2')) { quality_failures.push("official tool answer omitted fixture status or retry count".into()); }
            let official_json = evaluation_thinking_answer(
                &router,
                &config,
                &official,
                &json_context,
                None,
            ).await;
            assert_measured_answer(&official_json);
            let official_json_ok = serde_json::from_str::<serde_json::Value>(official_json.text.trim())
                .is_ok_and(|parsed| parsed["ok"] == true && parsed["count"] == 3);
            eprintln!("EVAL {}", serde_json::json!({"case":"official-json","text":official_json.text,"valid_expected_json":official_json_ok,"usage_measured":official_json.usage_measured}));
            if !official_json_ok { quality_failures.push("official JSON did not match the requested schema and values".into()); }
            let pt_context = cortex_eval_context(
                &config,
                "Segundo a evidência sintética, qual é o status do registro KITE-204 e qual é a contagem de tentativas? Responda em português brasileiro.",
                vec![synthetic_evidence("Registro KITE-204", "Registro sintético KITE-204: status READY; retry_count 2.")],
            );
            let official_pt = evaluation_thinking_answer(&router, &config, &official, &pt_context, None).await;
            assert_measured_answer(&official_pt);
            eprintln!("EVAL {}", serde_json::json!({"case":"official-pt-br","text":official_pt.text,"usage_measured":official_pt.usage_measured}));
            let pt = official_pt.text.to_lowercase();
            if !(pt.contains("ready") && official_pt.text.contains('2') && (pt.contains("status") || pt.contains("situação"))) {
                quality_failures.push("official PT-BR answer missed the supported status/count or language cue".into());
            }
            let thai_context = cortex_eval_context(
                &config,
                "ตอบเป็นภาษาไทยเท่านั้น: ตามหลักฐานสังเคราะห์ Option ของ Rust หมายถึงค่าไม่อยู่เมื่อใด",
                vec![synthetic_evidence(
                    "Thai language fact fixture",
                    "ใน Rust, Option<T> เป็น None เมื่อค่าไม่มี และ Some(value) เมื่อค่ามีอยู่",
                )],
            );
            let official_thai = evaluation_thinking_answer(
                &router,
                &config,
                &official,
                &thai_context,
                None,
            )
            .await;
            assert_measured_answer(&official_thai);
            let official_han = official_thai.text.chars().filter(|c| ('\u{4e00}'..='\u{9fff}').contains(c)).count();
            let official_thai_chars = official_thai.text.chars().filter(|c| ('\u{0e00}'..='\u{0e7f}').contains(c)).count();
            let official_inspection = official_thai.inspection.as_ref().map(|value| (value.reason.clone(), value.tokens.len()));
            eprintln!("EVAL {}", serde_json::json!({"case":"official-thai-raw","han_codepoints":official_han,"thai_codepoints":official_thai_chars,"inspection":official_inspection,"usage_measured":official_thai.usage_measured,"text":official_thai.text,"postprocessing":false}));
            if !(official_han == 0 && official_thai_chars > 0) { quality_failures.push("official Thai raw answer was not Thai-only script".into()); }

            // Load the previously empty Ollama T0 vision model only through the router.
            let image = {
                use image::{ImageBuffer, ImageFormat, Rgb};
                let mut bands = ImageBuffer::new(96, 48);
                for (x, y, pixel) in bands.enumerate_pixels_mut() {
                    *pixel = if y < 16 { Rgb([230, 30, 30]) } else if y < 32 { Rgb([20, 210, 40]) } else { Rgb([20, 40, 230]) };
                    let _ = x;
                }
                let mut bytes = std::io::Cursor::new(Vec::new());
                image::DynamicImage::ImageRgb8(bands).write_to(&mut bytes, ImageFormat::Png).unwrap();
                use base64::Engine;
                base64::engine::general_purpose::STANDARD.encode(bytes.into_inner())
            };
            owned_t0 = true;
            let vision = evaluation_output(ModelProfile::Bonsai25Uncensored, false);
            let thinking = crate::domain::services::thinking::Thinking::new(&config, router.clone());
            let (events, receiver, event_task) = crate::domain::services::events::channel(&config, None);
            let vision_result = {
                let (vision_cancel_tx, cancel) = watch::channel(false);
                let vision_call = thinking.describe_images_for_text_profile(
                    &vision,
                    "Describe the visible image briefly, including its colors.",
                    &[],
                    vec![image],
                    None,
                    &events,
                    cancel,
                );
                tokio::pin!(vision_call);
                let vision_deadline = tokio::time::sleep(Duration::from_secs(240));
                tokio::pin!(vision_deadline);
                let mut vision_monitor = tokio::time::interval(Duration::from_secs(5));
                let mut pressure_cancelled_vision = false;
                loop {
                    tokio::select! {
                        result = &mut vision_call => break result,
                        _ = vision_monitor.tick() => {
                            if evaluation_resources_ok(700).is_err() && !pressure_cancelled_vision {
                                pressure_cancelled_vision = true;
                                let _ = vision_cancel_tx.send(true);
                            }
                        }
                        _ = &mut vision_deadline => {
                            let _ = vision_cancel_tx.send(true);
                            break tokio::time::timeout(Duration::from_secs(60), &mut vision_call)
                                .await.expect("bounded T0 prepass cancellation");
                        }
                    }
                }
            };
            let (description, vision_usage) = vision_result.expect("real T0 image prepass");
            drop(events);
            drop(receiver);
            event_task.await.expect("event output task");
            assert!(!description.trim().is_empty());
            assert!(supervisor.child.lock().await.is_none(), "switch to T0 must stop the owned Prism process");
            assert_eq!(ollama_loaded(&config).await.expect("T0 loaded model identity"), vec![config.vision_model.clone()]);
            eprintln!("EVAL {}", serde_json::json!({"case":"t0-image-prepass-source","model":config.vision_model,"description":description,"input_tokens":vision_usage.input_tokens,"output_tokens":vision_usage.output_tokens,"ttft_ms":vision_usage.time_to_first_token_ms,"total_ms":vision_usage.total_ms}));
            assert!(unload_owned_t0(&config).await.expect("unload only the test's sole T0 vision model"));
            let text_profile = evaluation_output(ModelProfile::Bonsai25Uncensored, false);
            let image_context = cortex_eval_context(
                &config,
                "According to the synthetic image description, state the three visible color bands.",
                vec![synthetic_evidence("T0 synthetic image description", &description)],
            );
            let grounded = evaluation_thinking_answer(
                &router,
                &config,
                &text_profile,
                &image_context,
                None,
            )
            .await;
            assert_measured_answer(&grounded);
            eprintln!("EVAL {}", serde_json::json!({"case":"t0-prepass-to-t1-text","profile":text_profile.model_id(),"images_sent_to_t1":0,"text":grounded.text,"usage_measured":grounded.usage_measured}));
            let grounded_colors = grounded.text.to_lowercase();
            if !(grounded_colors.contains("red") && grounded_colors.contains("green") && grounded_colors.contains("blue")) {
                quality_failures.push("T0 image-prepass/T1 text answer omitted one or more supported color bands".into());
            }

            for cancellation_profile in [ModelProfile::Bonsai25Uncensored, ModelProfile::Bonsai25Official] {
                evaluation_cancellation_case(
                    &router,
                    &queue,
                    &supervisor,
                    cancellation_profile,
                    &mut owned_t0,
                )
                .await;
            }
            eprintln!("QUALITY_FAILURES {}", serde_json::json!({"count":quality_failures.len(),"cases":&quality_failures}));
            assert!(quality_failures.is_empty(), "semantic quality criteria failed after all profiles and lifecycle cases; see QUALITY_FAILURES and preceding raw EVAL records");
        }).catch_unwind().await;

        // Cleanup only this supervisor child. T0 started empty by the preflight;
        // unload only if its exact model is the sole resident now.
        let stop_result = supervisor.stop().await;
        let cleanup_state = async {
            if owned_t0 {
                let _ = unload_owned_t0(&config).await?;
            }
            let remaining = ollama_loaded(&config).await?;
            if !remaining.is_empty() {
                return Err(format!("Ollama models remain after owned cleanup; preserved without unloading: {remaining:?}"));
            }
            Ok::<(), String>(())
        }.await;
        stop_result.expect("owned Prism cleanup");
        cleanup_state.expect("verify Ollama is empty or report preserved unexpected models");
        if let Err(panic) = result {
            std::panic::resume_unwind(panic);
        }
    }

    #[tokio::test]
    #[ignore = "requires separate explicit GPU authorization; runs only Official cancellation/queue case"]
    async fn gpu_official_cancellation_queue_acceptance() {
        use futures_util::FutureExt;

        let (config, queue, supervisor, router) = evaluation_model_router_setup().await;
        let mut owned_t0 = false;
        let result = std::panic::AssertUnwindSafe(async {
            // The failed A5 Official iteration began after an Uncensored→T0
            // transition, so reproduce the T0-resident precondition without
            // repeating either profile's quality suite.
            owned_t0 = true;
            let setup_output = evaluation_output(ModelProfile::CortexT0, false);
            let (setup_events, _setup_rx) = mpsc::unbounded_channel();
            let (setup_cancel_tx, setup_cancel) = watch::channel(false);
            let setup_call = router.call(
                &setup_output,
                eval_input("Reply with READY.", vec![], None, false),
                setup_events,
                setup_cancel,
            );
            tokio::pin!(setup_call);
            let deadline = tokio::time::sleep(Duration::from_secs(300));
            tokio::pin!(deadline);
            let mut monitor = tokio::time::interval(Duration::from_secs(5));
            let mut setup_pressure = false;
            let setup_result = loop {
                tokio::select! {
                    result = &mut setup_call => break result,
                    _ = monitor.tick() => {
                        if evaluation_resources_ok(700).is_err() {
                            setup_pressure = true;
                            let _ = setup_cancel_tx.send(true);
                            break tokio::time::timeout(Duration::from_secs(60), &mut setup_call)
                                .await.expect("bounded T0 setup cancellation");
                        }
                    }
                    _ = &mut deadline => {
                        let _ = setup_cancel_tx.send(true);
                        break tokio::time::timeout(Duration::from_secs(60), &mut setup_call)
                            .await.expect("bounded T0 setup timeout cancellation");
                    }
                }
            };
            let loaded = ollama_loaded(&config).await;
            eprintln!(
                "EVAL {}",
                serde_json::json!({"case":"official-cancel-t0-setup","text":setup_result.as_ref().ok().map(|answer|answer.text.as_str()),"error":setup_result.as_ref().err().map(|error|format!("{error:?}")),"loaded_models":loaded.as_ref().ok(),"setup_pressure":setup_pressure})
            );
            assert!(!setup_pressure, "T0 setup crossed a resource guard");
            let setup_answer = setup_result.expect("T0 setup provider call");
            assert!(setup_answer.error.is_none(), "T0 setup error: {:?}", setup_answer.error);
            assert!(!setup_answer.text.trim().is_empty());
            assert_eq!(
                loaded.expect("T0 setup model identity"),
                vec![config.text_model.clone()],
                "T0 must be the sole resident before the Official transition"
            );

            evaluation_cancellation_case(
                &router,
                &queue,
                &supervisor,
                ModelProfile::Bonsai25Official,
                &mut owned_t0,
            )
            .await;
        })
        .catch_unwind()
        .await;

        let stop_result = supervisor.stop().await;
        let cleanup_state = async {
            if owned_t0 {
                let _ = unload_owned_t0(&config).await?;
            }
            let remaining = ollama_loaded(&config).await?;
            if !remaining.is_empty() {
                return Err(format!("Ollama models remain after owned cleanup; preserved without unloading: {remaining:?}"));
            }
            Ok::<(), String>(())
        }
        .await;
        stop_result.expect("owned Official Prism cleanup");
        cleanup_state.expect("verify Ollama is empty or preserve unexpected models");
        if let Err(panic) = result {
            std::panic::resume_unwind(panic);
        }
    }

    #[tokio::test]
    #[ignore = "requires separate explicit GPU authorization; captures one synthetic prefix request and SSE"]
    async fn gpu_prism_prefix_diagnostic_uncensored() {
        use futures_util::FutureExt;

        assert!(
            std::env::var_os("CORTEX_TEST_CAPTURE_PRISM_PREFIX_SSE").is_some(),
            "diagnostic wrapper must explicitly enable bounded raw SSE capture"
        );
        evaluation_resources_ok(7600)
            .expect("safe host resources before model use; MTP requires headroom");
        let available = std::net::TcpListener::bind("127.0.0.1:0").expect("free loopback port");
        let port = available.local_addr().unwrap().port();
        drop(available);
        let config = Config {
            bonsai_enabled: true,
            prism_server_path: std::env::var("CORTEX_TEST_PRISM_SERVER")
                .expect("diagnostic wrapper pins CORTEX_TEST_PRISM_SERVER")
                .into(),
            bonsai_model_directory: std::env::var("CORTEX_TEST_BONSAI_DIR")
                .expect("diagnostic wrapper pins CORTEX_TEST_BONSAI_DIR")
                .into(),
            ollama_url: std::env::var("CORTEX_TEST_OLLAMA_URL")
                .expect("diagnostic wrapper pins CORTEX_TEST_OLLAMA_URL"),
            prism_url: format!("http://127.0.0.1:{port}"),
            ..Config::default()
        };
        assert!(config.prism_url_is_loopback());
        assert!(
            ollama_loaded(&config)
                .await
                .expect("Ollama preflight")
                .is_empty(),
            "pre-existing Ollama models: do not unload another user's work"
        );
        let apps = std::process::Command::new("timeout")
            .args([
                "5s",
                "nvidia-smi",
                "--query-compute-apps=pid,name",
                "--format=csv,noheader",
            ])
            .output()
            .expect("nvidia-smi compute-process preflight");
        assert!(
            apps.status.success(),
            "nvidia-smi failed: {}",
            String::from_utf8_lossy(&apps.stderr)
        );
        assert!(
            String::from_utf8_lossy(&apps.stdout).trim().is_empty(),
            "GPU compute process already active; do not disturb it"
        );

        let queue = Scheduler::new(Arc::new(EvaluationLedger));
        let supervisor = Arc::new(PrismSupervisor::new());
        let ollama = Arc::new(
            Ollama::new(config.clone(), queue.clone()).with_prism_supervisor(supervisor.clone()),
        );
        let prism = Arc::new(Prism::managed(
            config.clone(),
            queue.clone(),
            supervisor.clone(),
        ));
        let router = Arc::new(ModelRouter::new(ollama, prism, queue));
        let result = std::panic::AssertUnwindSafe(async {
            let output = evaluation_output(ModelProfile::Bonsai25Uncensored, false);
            let original_question =
                "According to the synthetic record for KITE-204, what status and retry count are recorded? Answer in one sentence.";
            let original_context = cortex_eval_context(
                &config,
                original_question,
                vec![synthetic_evidence(
                    "KITE-204 synthetic record",
                    "Synthetic record KITE-204: status READY; retry_count 2.",
                )],
            );
            let cold = evaluation_thinking_answer(
                &router,
                &config,
                &output,
                &original_context,
                None,
            )
            .await;
            assert!(!cold.text.trim().is_empty(), "initial synthetic answer is nonempty");
            assert_measured_answer(&cold);
            let saved_prefix = middle_word_prefix(&cold.text)
                .expect("initial answer has a valid middle word boundary");
            let continuation = evaluation_thinking_answer(
                &router,
                &config,
                &output,
                &original_context,
                Some(saved_prefix.clone()),
            )
            .await;
            eprintln!(
                "PREFIX_DIAGNOSTIC_RESULT {}",
                serde_json::json!({
                    "profile": output.model_id(),
                    "initial_text": cold.text,
                    "initial_tokens": cold.usage.output_tokens,
                    "saved_prefix": saved_prefix,
                    "continuation_text": continuation.text,
                    "continuation_input_tokens": continuation.usage.input_tokens,
                    "continuation_output_tokens": continuation.usage.output_tokens,
                    "continuation_ttft_ms": continuation.usage.time_to_first_token_ms,
                    "continuation_measured": continuation.usage_measured,
                    "continuation_error": continuation.error.as_ref().map(|error| format!("{error:?}")),
                    "inspection_reason": continuation.inspection.as_ref().and_then(|inspection| inspection.reason.clone()),
                })
            );
            assert!(
                continuation.error.is_none(),
                "provider must complete the targeted prefix request without a stream error"
            );
            assert!(
                !continuation.text.trim().is_empty(),
                "a prefix cut in the middle of the generated answer must yield visible continuation text"
            );
            assert_measured_answer(&continuation);
        })
        .catch_unwind()
        .await;
        supervisor
            .stop()
            .await
            .expect("stop/reap only the diagnostic Prism child");
        assert!(
            ollama_loaded(&config)
                .await
                .expect("verify Ollama stayed empty")
                .is_empty(),
            "diagnostic must not load or unload T0 models"
        );
        if let Err(panic) = result {
            std::panic::resume_unwind(panic);
        }
    }

    struct MockSystemdRunner {
        status: Mutex<UnitStatus>,
        start_calls: Mutex<Vec<String>>,
        stop_calls: Mutex<Vec<String>>,
        show_calls: Mutex<Vec<String>>,
        fail_start: Mutex<bool>,
        fail_stop: Mutex<bool>,
        fail_show: Mutex<bool>,
        fail_show_after_start: Mutex<bool>,
        stop_hang: Mutex<bool>,
        stop_clears_active: bool,
        after_start_cancel_sender: Mutex<Option<watch::Sender<bool>>>,
    }

    impl MockSystemdRunner {
        fn new(initial_status: UnitStatus) -> Self {
            Self {
                status: Mutex::new(initial_status),
                start_calls: Mutex::new(Vec::new()),
                stop_calls: Mutex::new(Vec::new()),
                show_calls: Mutex::new(Vec::new()),
                fail_start: Mutex::new(false),
                fail_stop: Mutex::new(false),
                fail_show: Mutex::new(false),
                fail_show_after_start: Mutex::new(false),
                stop_hang: Mutex::new(false),
                stop_clears_active: true,
                after_start_cancel_sender: Mutex::new(None),
            }
        }
    }

    impl SystemdRunner for MockSystemdRunner {
        fn start_unit<'a>(&'a self, unit: &'a str) -> Pending<'a, Result<(), DriverError>> {
            Box::pin(async move {
                if unit != ALLOWED_UNIT {
                    return Err(DriverError::InvalidInput);
                }
                self.start_calls.lock().await.push(unit.to_string());
                if *self.fail_start.lock().await {
                    return Err(DriverError::Ollama);
                }
                let mut status = self.status.lock().await;
                status.active_state = "active".to_string();
                status.sub_state = "running".to_string();
                if status.main_pid == 0 {
                    status.main_pid = 4242;
                }
                if status.invocation_id.is_empty() {
                    status.invocation_id = "test-inv-001".to_string();
                }
                if status.control_group.is_empty() {
                    status.control_group = "/system.slice/prism.service".to_string();
                }
                if let Some(tx) = self.after_start_cancel_sender.lock().await.as_ref() {
                    let _ = tx.send(true);
                }
                Ok(())
            })
        }

        fn stop_unit<'a>(&'a self, unit: &'a str) -> Pending<'a, Result<(), DriverError>> {
            Box::pin(async move {
                if unit != ALLOWED_UNIT {
                    return Err(DriverError::InvalidInput);
                }
                self.stop_calls.lock().await.push(unit.to_string());
                if *self.fail_stop.lock().await {
                    return Err(DriverError::Ollama);
                }
                if *self.stop_hang.lock().await {
                    return Ok(());
                }
                if self.stop_clears_active {
                    let mut status = self.status.lock().await;
                    status.active_state = "inactive".to_string();
                    status.sub_state = "dead".to_string();
                    status.main_pid = 0;
                }
                Ok(())
            })
        }

        fn show_unit<'a>(&'a self, unit: &'a str) -> Pending<'a, Result<UnitStatus, DriverError>> {
            Box::pin(async move {
                if unit != ALLOWED_UNIT {
                    return Err(DriverError::InvalidInput);
                }
                self.show_calls.lock().await.push(unit.to_string());
                if *self.fail_show.lock().await {
                    return Err(DriverError::Ollama);
                }
                if *self.fail_show_after_start.lock().await
                    && !self.start_calls.lock().await.is_empty()
                {
                    *self.fail_show_after_start.lock().await = false;
                    return Err(DriverError::Ollama);
                }
                Ok(self.status.lock().await.clone())
            })
        }
    }

    struct TestDir {
        path: std::path::PathBuf,
    }

    impl TestDir {
        fn new() -> Self {
            let path =
                std::env::temp_dir().join(format!("cortex-prism-test-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&path).expect("create test dir");
            Self { path }
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    #[tokio::test]
    async fn systemctl_runner_enforces_unit_allowlist() {
        let runner = SystemctlRunner::new();
        assert_eq!(
            runner.start_unit("forbidden.service").await.unwrap_err(),
            DriverError::InvalidInput
        );
        assert_eq!(
            runner.stop_unit("cortex-api.service").await.unwrap_err(),
            DriverError::InvalidInput
        );
        assert_eq!(
            runner.show_unit("other.service").await.unwrap_err(),
            DriverError::InvalidInput
        );
    }

    #[test]
    fn parse_unit_status_extracts_properties() {
        let raw = "\
ActiveState=active
SubState=running
MainPID=54321
ControlGroup=/system.slice/prism.service
InvocationID=inv-789-xyz
SomethingElse=ignored
";
        let status = parse_unit_status(raw).unwrap();
        assert_eq!(status.active_state, "active");
        assert_eq!(status.sub_state, "running");
        assert_eq!(status.main_pid, 54321);
        assert_eq!(status.control_group, "/system.slice/prism.service");
        assert_eq!(status.invocation_id, "inv-789-xyz");
        assert!(status.is_active());
        assert!(!status.is_inactive());

        let inactive_raw = "\
ActiveState=inactive
SubState=dead
MainPID=0
ControlGroup=
InvocationID=
";
        let inact = parse_unit_status(inactive_raw).unwrap();
        assert!(!inact.is_active());
        assert!(inact.is_inactive());
    }

    #[tokio::test]
    async fn mock_runner_starts_and_reuses_same_profile() {
        let test_dir = TestDir::new();
        let profile_path = test_dir.path.join("profile");
        let runner = Arc::new(MockSystemdRunner::new(UnitStatus {
            load_state: "loaded".into(),
            active_state: "inactive".into(),
            sub_state: "dead".into(),
            main_pid: 0,
            control_group: String::new(),
            invocation_id: String::new(),
        }));
        let mut supervisor = PrismSupervisor::with_runner(runner.clone());
        supervisor.skip_hash_for_test = true;

        let config = Config {
            prism_profile_path: profile_path.clone(),
            ..Config::default()
        };
        let (_cancel_tx, mut cancel) = watch::channel(false);

        // First start
        supervisor
            .ensure(&config, ModelProfile::Bonsai25Uncensored, &mut cancel)
            .await
            .unwrap();

        assert_eq!(
            runner.start_calls.lock().await.as_slice(),
            &["prism.service"]
        );
        assert!(
            supervisor
                .is_running(ModelProfile::Bonsai25Uncensored)
                .await
                .unwrap()
        );
        assert!(
            !supervisor
                .is_running(ModelProfile::Bonsai25Official)
                .await
                .unwrap()
        );

        // Verify selector file
        let written_profile = tokio::fs::read_to_string(&profile_path).await.unwrap();
        assert_eq!(written_profile.trim(), "bonsai25-uncensored");

        // Second call with same profile: reuse without new start_unit call
        supervisor
            .ensure(&config, ModelProfile::Bonsai25Uncensored, &mut cancel)
            .await
            .unwrap();
        assert_eq!(runner.start_calls.lock().await.len(), 1);
        assert!(
            supervisor
                .is_running(ModelProfile::Bonsai25Uncensored)
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn mock_runner_profile_switch_stops_old_before_writing_new() {
        let test_dir = TestDir::new();
        let profile_path = test_dir.path.join("profile");
        let runner = Arc::new(MockSystemdRunner::new(UnitStatus {
            load_state: "loaded".into(),
            active_state: "inactive".into(),
            sub_state: "dead".into(),
            main_pid: 0,
            control_group: String::new(),
            invocation_id: String::new(),
        }));
        let mut supervisor = PrismSupervisor::with_runner(runner.clone());
        supervisor.skip_hash_for_test = true;

        let config = Config {
            prism_profile_path: profile_path.clone(),
            ..Config::default()
        };
        let (_cancel_tx, mut cancel) = watch::channel(false);

        // Start Uncensored
        supervisor
            .ensure(&config, ModelProfile::Bonsai25Uncensored, &mut cancel)
            .await
            .unwrap();
        assert_eq!(
            tokio::fs::read_to_string(&profile_path)
                .await
                .unwrap()
                .trim(),
            "bonsai25-uncensored"
        );

        // Switch to Official
        supervisor
            .ensure(&config, ModelProfile::Bonsai25Official, &mut cancel)
            .await
            .unwrap();

        // Verify stop was called before starting the new profile
        assert_eq!(
            runner.stop_calls.lock().await.as_slice(),
            &["prism.service"]
        );
        assert_eq!(runner.start_calls.lock().await.len(), 2);
        assert_eq!(
            tokio::fs::read_to_string(&profile_path)
                .await
                .unwrap()
                .trim(),
            "bonsai25-official"
        );
        assert!(
            supervisor
                .is_running(ModelProfile::Bonsai25Official)
                .await
                .unwrap()
        );
        assert!(
            !supervisor
                .is_running(ModelProfile::Bonsai25Uncensored)
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn stop_failure_fails_closed_and_marks_inactive() {
        let runner = Arc::new(MockSystemdRunner::new(UnitStatus {
            load_state: "loaded".into(),
            active_state: "active".into(),
            sub_state: "running".into(),
            main_pid: 1234,
            control_group: "/system.slice/prism.service".into(),
            invocation_id: "inv-123".into(),
        }));
        *runner.fail_stop.lock().await = true;

        let supervisor = PrismSupervisor::with_runner(runner.clone());
        {
            let mut slot = supervisor.child.lock().await;
            *slot = Some(ActiveService {
                profile: ModelProfile::Bonsai25Uncensored,
                pid: 1234,
                invocation_id: "inv-123".into(),
                control_group: "/system.slice/prism.service".into(),
                stopping: false,
            });
        }

        // Attempting to stop should fail
        let res = supervisor.stop().await;
        assert!(res.is_err());

        // Service should not report as healthy running
        assert!(
            !supervisor
                .is_running(ModelProfile::Bonsai25Uncensored)
                .await
                .unwrap()
        );
    }

    #[derive(Clone, Default)]
    struct LocalOllamaDouble {
        ps_calls: std::sync::Arc<std::sync::atomic::AtomicUsize>,
        generate_calls: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    }

    async fn run_mock_ollama(listener: tokio::net::TcpListener, state: LocalOllamaDouble) {
        use std::sync::atomic::Ordering;
        let app = axum::Router::new()
            .route(
                "/api/ps",
                axum::routing::get({
                    let state = state.clone();
                    move || {
                        state.ps_calls.fetch_add(1, Ordering::SeqCst);
                        async { axum::Json(serde_json::json!({ "models": [] })) }
                    }
                }),
            )
            .route(
                "/api/generate",
                axum::routing::post({
                    let state = state.clone();
                    move || {
                        state.generate_calls.fetch_add(1, Ordering::SeqCst);
                        async { axum::Json(serde_json::json!({ "done": true, "response": "ok" })) }
                    }
                }),
            );
        let _ = axum::serve(listener, app).await;
    }

    #[tokio::test]
    async fn scheduler_blocks_next_provider_while_prism_unconfirmed_or_stopping() {
        use std::sync::atomic::Ordering;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let mock = LocalOllamaDouble::default();
        let _server = tokio::spawn(run_mock_ollama(listener, mock.clone()));

        let queue = Scheduler::new(Arc::new(EvaluationLedger));
        let runner = Arc::new(MockSystemdRunner::new(UnitStatus {
            load_state: "loaded".into(),
            active_state: "active".into(),
            sub_state: "running".into(),
            main_pid: 1234,
            control_group: "/system.slice/prism.service".into(),
            invocation_id: "inv-123".into(),
        }));
        *runner.fail_stop.lock().await = true;

        let supervisor = Arc::new(PrismSupervisor::with_runner(runner.clone()));
        {
            let mut slot = supervisor.child.lock().await;
            *slot = Some(ActiveService {
                profile: ModelProfile::Bonsai25Uncensored,
                pid: 1234,
                invocation_id: "inv-123".into(),
                control_group: "/system.slice/prism.service".into(),
                stopping: false,
            });
        }

        let config = Config {
            bonsai_enabled: true,
            ollama_url: format!("http://{addr}"),
            ..Config::default()
        };
        let ollama = Arc::new(
            Ollama::new(config.clone(), queue.clone()).with_prism_supervisor(supervisor.clone()),
        );

        let permit = queue
            .acquire("test-actor".into(), uuid::Uuid::new_v4())
            .await
            .unwrap();

        // 1. Attempt provider while Prism stop is unconfirmed/failing.
        let res = ollama.release_models_for_local_job(&permit).await;
        assert_eq!(res.unwrap_err(), DriverError::Ollama);
        // Confirm ZERO provider requests reached the local fake provider while Prism stop is unconfirmed.
        assert_eq!(
            mock.ps_calls.load(Ordering::SeqCst),
            0,
            "provider must receive zero requests while Prism stop is unconfirmed"
        );

        // 2. Repair stop in runner; retry reaches local fake provider.
        *runner.fail_stop.lock().await = false;
        let res_ok = ollama.release_models_for_local_job(&permit).await;
        assert!(res_ok.is_ok());
        assert!(
            mock.ps_calls.load(Ordering::SeqCst) > 0,
            "retry after confirmed stop must reach the local provider"
        );
    }

    #[tokio::test]
    async fn ollama_t0_works_when_bonsai_disabled_and_unit_absent() {
        use std::sync::atomic::Ordering;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let mock = LocalOllamaDouble::default();
        let _server = tokio::spawn(run_mock_ollama(listener, mock.clone()));

        let queue = Scheduler::new(Arc::new(EvaluationLedger));
        let config = Config {
            bonsai_enabled: false,
            ollama_url: format!("http://{addr}"),
            ..Config::default()
        };

        // Missing-unit SystemctlRunner response shape (not-found / inactive / dead).
        let runner = Arc::new(MockSystemdRunner::new(UnitStatus {
            load_state: "not-found".into(),
            active_state: "inactive".into(),
            sub_state: "dead".into(),
            main_pid: 0,
            control_group: String::new(),
            invocation_id: String::new(),
        }));
        let supervisor = Arc::new(PrismSupervisor::with_runner(runner.clone()));
        let ollama = Arc::new(
            Ollama::new(config.clone(), queue.clone()).with_prism_supervisor(supervisor.clone()),
        );
        let permit = queue
            .acquire("test-actor".into(), uuid::Uuid::new_v4())
            .await
            .unwrap();

        // Release models / prepare local job for T0 or Whisper:
        let res = ollama.release_models_for_local_job(&permit).await;
        assert!(res.is_ok());
        // Confirm zero systemctl stop commands were issued when unit is absent and Bonsai disabled.
        assert_eq!(runner.stop_calls.lock().await.len(), 0);
        // Confirm preparation/provider boundary reaches local fake Ollama provider without systemctl.
        assert!(
            mock.ps_calls.load(Ordering::SeqCst) > 0,
            "T0/Whisper path must reach local provider when unit is absent"
        );
    }

    #[tokio::test]
    async fn stale_unit_reuse_rejection() {
        let test_dir = TestDir::new();
        let profile_path = test_dir.path.join("profile");
        let runner = Arc::new(MockSystemdRunner::new(UnitStatus {
            load_state: "loaded".into(),
            active_state: "inactive".into(),
            sub_state: "dead".into(),
            main_pid: 0,
            control_group: String::new(),
            invocation_id: String::new(),
        }));
        let mut supervisor = PrismSupervisor::with_runner(runner.clone());
        supervisor.skip_hash_for_test = true;

        let config = Config {
            prism_profile_path: profile_path.clone(),
            ..Config::default()
        };
        let (_cancel_tx, mut cancel) = watch::channel(false);

        supervisor
            .ensure(&config, ModelProfile::Bonsai25Uncensored, &mut cancel)
            .await
            .unwrap();

        // Simulate stale InvocationID in unit status
        {
            let mut status = runner.status.lock().await;
            status.invocation_id = "stale-invocation-different".into();
        }

        // is_running must return false due to stale InvocationID
        assert!(
            !supervisor
                .is_running(ModelProfile::Bonsai25Uncensored)
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn cancellation_before_start_skips_start() {
        let test_dir = TestDir::new();
        let profile_path = test_dir.path.join("profile");
        let runner = Arc::new(MockSystemdRunner::new(UnitStatus {
            active_state: "inactive".into(),
            sub_state: "dead".into(),
            main_pid: 0,
            control_group: String::new(),
            invocation_id: String::new(),
            ..Default::default()
        }));
        let mut supervisor = PrismSupervisor::with_runner(runner.clone());
        supervisor.skip_hash_for_test = true;

        let config = Config {
            prism_profile_path: profile_path.clone(),
            ..Config::default()
        };
        let (_cancel_tx, mut cancel) = watch::channel(true); // already cancelled

        let res = supervisor
            .ensure(&config, ModelProfile::Bonsai25Uncensored, &mut cancel)
            .await;
        assert_eq!(res.unwrap_err(), DriverError::Cancelled);
        assert_eq!(runner.start_calls.lock().await.len(), 0);
    }

    #[tokio::test]
    async fn cancellation_after_start_triggers_immediate_stop() {
        let test_dir = TestDir::new();
        let profile_path = test_dir.path.join("profile");
        let runner = Arc::new(MockSystemdRunner::new(UnitStatus {
            active_state: "inactive".into(),
            sub_state: "dead".into(),
            main_pid: 0,
            control_group: String::new(),
            invocation_id: String::new(),
            ..Default::default()
        }));
        let (cancel_tx, mut cancel) = watch::channel(false);
        *runner.after_start_cancel_sender.lock().await = Some(cancel_tx);

        let mut supervisor = PrismSupervisor::with_runner(runner.clone());
        supervisor.skip_hash_for_test = true;

        let config = Config {
            prism_profile_path: profile_path.clone(),
            ..Config::default()
        };

        let res = supervisor
            .ensure(&config, ModelProfile::Bonsai25Uncensored, &mut cancel)
            .await;
        assert_eq!(res.unwrap_err(), DriverError::Cancelled);
        assert_eq!(runner.start_calls.lock().await.len(), 1);
        assert_eq!(runner.stop_calls.lock().await.len(), 1);
        assert!(supervisor.child.lock().await.is_none());
    }

    #[tokio::test]
    async fn show_error_after_start_quiesces_and_cleans_slot() {
        let test_dir = TestDir::new();
        let profile_path = test_dir.path.join("profile");
        let runner = Arc::new(MockSystemdRunner::new(UnitStatus {
            active_state: "inactive".into(),
            sub_state: "dead".into(),
            main_pid: 0,
            control_group: String::new(),
            invocation_id: String::new(),
            ..Default::default()
        }));
        *runner.fail_show_after_start.lock().await = true;

        let mut supervisor = PrismSupervisor::with_runner(runner.clone());
        supervisor.skip_hash_for_test = true;

        let config = Config {
            prism_profile_path: profile_path.clone(),
            ..Config::default()
        };
        let (_cancel_tx, mut cancel) = watch::channel(false);

        let res = supervisor
            .ensure(&config, ModelProfile::Bonsai25Uncensored, &mut cancel)
            .await;
        assert_eq!(res.unwrap_err(), DriverError::Ollama);
        assert_eq!(runner.start_calls.lock().await.len(), 1);
        assert_eq!(runner.stop_calls.lock().await.len(), 1);
        assert!(supervisor.child.lock().await.is_none());
    }

    #[tokio::test]
    async fn start_error_plus_cleanup_error_fails_closed() {
        let test_dir = TestDir::new();
        let profile_path = test_dir.path.join("profile");
        let runner = Arc::new(MockSystemdRunner::new(UnitStatus {
            active_state: "inactive".into(),
            sub_state: "dead".into(),
            main_pid: 0,
            control_group: String::new(),
            invocation_id: String::new(),
            ..Default::default()
        }));
        *runner.fail_start.lock().await = true;
        *runner.fail_stop.lock().await = true;

        let mut supervisor = PrismSupervisor::with_runner(runner.clone());
        supervisor.skip_hash_for_test = true;

        let config = Config {
            prism_profile_path: profile_path.clone(),
            ..Config::default()
        };
        let (_cancel_tx, mut cancel) = watch::channel(false);

        let res = supervisor
            .ensure(&config, ModelProfile::Bonsai25Uncensored, &mut cancel)
            .await;
        assert_eq!(res.unwrap_err(), DriverError::Ollama);
        assert_eq!(runner.start_calls.lock().await.len(), 1);
        assert_eq!(runner.stop_calls.lock().await.len(), 1);
    }

    #[tokio::test]
    async fn systemctl_runner_timeout_reaps_child() {
        let test_dir = TestDir::new();
        let script_path = test_dir.path.join("fake-systemctl");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let script_content = "#!/bin/sh\nsleep 30\n";
            std::fs::write(&script_path, script_content).unwrap();
            let mut perms = std::fs::metadata(&script_path).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&script_path, perms).unwrap();
        }
        let runner =
            SystemctlRunner::with_path_and_timeout(script_path, Duration::from_millis(150));
        let res = runner.start_unit(ALLOWED_UNIT).await;
        assert_eq!(res.unwrap_err(), DriverError::Ollama);
    }

    #[tokio::test]
    async fn non_loopback_url_rejected() {
        let runner = Arc::new(MockSystemdRunner::new(UnitStatus::default()));
        let supervisor = PrismSupervisor::with_runner(runner);
        let config = Config {
            prism_url: "http://192.168.1.100:49100".into(),
            ..Config::default()
        };
        let (_cancel_tx, mut cancel) = watch::channel(false);
        assert_eq!(
            supervisor
                .ensure(&config, ModelProfile::Bonsai25Uncensored, &mut cancel)
                .await
                .unwrap_err(),
            DriverError::InvalidInput
        );
    }

    #[tokio::test]
    async fn profile_selector_rejects_arbitrary_or_cortex_t0() {
        let test_dir = TestDir::new();
        let profile_path = test_dir.path.join("profile");
        assert_eq!(
            write_profile_atomic(&profile_path, ModelProfile::CortexT0)
                .await
                .unwrap_err(),
            DriverError::InvalidInput
        );
        let runner = Arc::new(MockSystemdRunner::new(UnitStatus::default()));
        let supervisor = PrismSupervisor::with_runner(runner);
        let config = Config {
            prism_profile_path: profile_path,
            ..Config::default()
        };
        let (_cancel_tx, mut cancel) = watch::channel(false);
        assert_eq!(
            supervisor
                .ensure(&config, ModelProfile::CortexT0, &mut cancel)
                .await
                .unwrap_err(),
            DriverError::InvalidInput
        );
    }

    #[tokio::test]
    async fn prism_child_environment_clears_pinned_runtime_auth_inputs() {
        // llama-server --help identifies exactly these auth environment inputs.
        let mut command = Command::new("/usr/bin/env");
        command
            .env("LLAMA_API_KEY", "must-not-inherit")
            .env("LLAMA_ARG_API_KEY_FILE", "/private/test/key-file");
        clear_runtime_api_auth_env(&mut command);
        let output = command.output().await.expect("read child environment");
        assert!(output.status.success());
        let environment = String::from_utf8(output.stdout).unwrap();
        assert!(
            !environment
                .lines()
                .any(|line| line.starts_with("LLAMA_API_KEY="))
        );
        assert!(
            !environment
                .lines()
                .any(|line| line.starts_with("LLAMA_ARG_API_KEY_FILE="))
        );
    }

    #[test]
    fn staged_manifest_matches_checksums() {
        let manifest_content =
            std::fs::read_to_string("deploy/prism/manifest.json").expect("read manifest.json");
        let manifest: serde_json::Value =
            serde_json::from_str(&manifest_content).expect("parse manifest.json");
        let files = manifest["staged_files"]
            .as_array()
            .expect("staged_files array");
        assert_eq!(files.len(), 4);
        for file in files {
            let path = file["path"].as_str().expect("path string");
            let expected_sha256 = file["sha256"].as_str().expect("sha256 string");
            let content = std::fs::read(path).unwrap_or_else(|e| panic!("read {path}: {e}"));
            let mut hasher = Sha256::new();
            hasher.update(&content);
            let calculated = format!("{:x}", hasher.finalize());
            assert_eq!(
                calculated, expected_sha256,
                "Hash mismatch for {path}: expected {expected_sha256}, got {calculated}"
            );
        }
    }

    #[test]
    fn profiles_resolve_only_pinned_artifact_names_and_hashes() {
        for profile in [
            ModelProfile::Bonsai25Official,
            ModelProfile::Bonsai25Uncensored,
        ] {
            assert!(profile.artifact_filename().unwrap().ends_with(".gguf"));
            assert_eq!(profile.artifact_sha256().unwrap().len(), 64);
            assert!(profile.runtime_alias().unwrap().starts_with("bonsai25-"));
        }
        assert!(ModelProfile::CortexT0.artifact_filename().is_none());
    }

    #[tokio::test]
    async fn fresh_supervisor_with_already_active_unit_stops_before_start() {
        let test_dir = TestDir::new();
        let profile_path = test_dir.path.join("profile");
        let runner = Arc::new(MockSystemdRunner::new(UnitStatus {
            load_state: "loaded".into(),
            active_state: "active".into(),
            sub_state: "running".into(),
            main_pid: 9999,
            control_group: "/system.slice/prism.service".into(),
            invocation_id: "prior-running-inv".into(),
        }));
        let mut supervisor = PrismSupervisor::with_runner(runner.clone());
        supervisor.skip_hash_for_test = true;

        let config = Config {
            prism_profile_path: profile_path.clone(),
            ..Config::default()
        };
        let (_cancel_tx, mut cancel) = watch::channel(false);

        // Ensure fresh supervisor slot is None:
        assert!(supervisor.child.lock().await.is_none());

        supervisor
            .ensure(&config, ModelProfile::Bonsai25Uncensored, &mut cancel)
            .await
            .unwrap();

        // Must have called stop_unit to quiesce the unknown running unit before starting
        assert_eq!(
            runner.stop_calls.lock().await.as_slice(),
            &["prism.service"]
        );
        assert_eq!(
            runner.start_calls.lock().await.as_slice(),
            &["prism.service"]
        );
        assert!(
            supervisor
                .is_running(ModelProfile::Bonsai25Uncensored)
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn fresh_supervisor_with_activating_unit_stops_before_start() {
        let test_dir = TestDir::new();
        let profile_path = test_dir.path.join("profile");
        let runner = Arc::new(MockSystemdRunner::new(UnitStatus {
            load_state: "loaded".into(),
            active_state: "activating".into(),
            sub_state: "start-pre".into(),
            main_pid: 0,
            control_group: "/system.slice/prism.service".into(),
            invocation_id: "activating-inv".into(),
        }));
        let mut supervisor = PrismSupervisor::with_runner(runner.clone());
        supervisor.skip_hash_for_test = true;

        let config = Config {
            prism_profile_path: profile_path.clone(),
            ..Config::default()
        };
        let (_cancel_tx, mut cancel) = watch::channel(false);

        supervisor
            .ensure(&config, ModelProfile::Bonsai25Uncensored, &mut cancel)
            .await
            .unwrap();

        assert_eq!(
            runner.stop_calls.lock().await.as_slice(),
            &["prism.service"]
        );
        assert_eq!(
            runner.start_calls.lock().await.as_slice(),
            &["prism.service"]
        );
    }

    #[tokio::test]
    async fn starting_unit_failure_quiesces_unit_and_cleans_slot() {
        let test_dir = TestDir::new();
        let profile_path = test_dir.path.join("profile");
        let runner = Arc::new(MockSystemdRunner::new(UnitStatus {
            load_state: "loaded".into(),
            active_state: "inactive".into(),
            sub_state: "dead".into(),
            main_pid: 0,
            control_group: String::new(),
            invocation_id: String::new(),
        }));
        *runner.fail_start.lock().await = true;

        let mut supervisor = PrismSupervisor::with_runner(runner.clone());
        supervisor.skip_hash_for_test = true;

        let config = Config {
            prism_profile_path: profile_path.clone(),
            ..Config::default()
        };
        let (_cancel_tx, mut cancel) = watch::channel(false);

        let res = supervisor
            .ensure(&config, ModelProfile::Bonsai25Uncensored, &mut cancel)
            .await;
        assert!(res.is_err());
        assert!(supervisor.child.lock().await.is_none());
    }

    #[tokio::test]
    async fn write_profile_atomic_cleans_up_temp_on_failure() {
        let test_dir = TestDir::new();
        let blocker = test_dir.path.join("not_a_dir");
        std::fs::write(&blocker, b"blocking file").unwrap();
        let invalid_profile_path = blocker.join("sub/profile");

        let res =
            write_profile_atomic(&invalid_profile_path, ModelProfile::Bonsai25Uncensored).await;
        assert!(res.is_err());

        let entries = std::fs::read_dir(&test_dir.path).unwrap();
        for entry in entries {
            let path = entry.unwrap().path();
            let name = path.file_name().unwrap().to_string_lossy();
            assert!(
                !name.starts_with(".profile.") || !name.ends_with(".tmp"),
                "leftover tmpfile found: {}",
                name
            );
        }
    }
}

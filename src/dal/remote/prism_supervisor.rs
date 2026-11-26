//! Owns the one Prism server process and one allowlisted loaded Bonsai artifact.
//! The server is never started at application boot and is not a system service.
use crate::{
    core::config::Config,
    domain::models::{driver::DriverError, model_profile::ModelProfile},
};
use reqwest::header::HeaderValue;
use sha2::{Digest, Sha256};
use std::{path::Path, process::Stdio, time::Duration};
use tokio::{
    io::AsyncReadExt,
    process::{Child, Command},
    sync::{Mutex, watch},
};

const SERVER_SHA256: &str = "89e7f20b23b97bf77b91a0143a168763f7835fd6e0a7a7a23bb70951d991c02c";

fn new_api_key() -> String {
    uuid::Uuid::new_v4().to_string()
}

struct OwnedServer {
    profile: ModelProfile,
    api_key: String,
    pid: u32,
    stopping: bool,
    child: Child,
}

#[derive(Default)]
pub struct PrismSupervisor {
    child: Mutex<Option<OwnedServer>>,
}

impl PrismSupervisor {
    pub fn new() -> Self {
        Self::default()
    }

    /// Stop only this supervisor's child process group. No URL/API kill request is used.
    pub async fn stop(&self) -> Result<(), DriverError> {
        let mut slot = self.child.lock().await;
        stop_owned(&mut slot).await
    }

    pub async fn is_running(&self, profile: ModelProfile) -> Result<bool, DriverError> {
        let mut slot = self.child.lock().await;
        match slot.as_mut() {
            Some(owned) if owned.profile == profile && !owned.stopping => Ok(owned
                .child
                .try_wait()
                .map_err(|_| DriverError::Ollama)?
                .is_none()),
            _ => Ok(false),
        }
    }

    /// Hash the pinned runtime and model before starting the fixed server command.
    /// Repeated requests for the same healthy profile keep the one process loaded.
    pub async fn ensure(
        &self,
        config: &Config,
        profile: ModelProfile,
        cancel: &mut watch::Receiver<bool>,
    ) -> Result<(), DriverError> {
        if !config.prism_url_is_loopback() {
            return Err(DriverError::InvalidInput);
        }
        let alias = profile.runtime_alias().ok_or(DriverError::InvalidInput)?;
        let filename = profile
            .artifact_filename()
            .ok_or(DriverError::InvalidInput)?;
        let expected_model_hash = profile.artifact_sha256().ok_or(DriverError::InvalidInput)?;
        let mut slot = self.child.lock().await;
        if let Some(owned) = slot.as_mut()
            && owned.profile == profile
            && !owned.stopping
            && owned
                .child
                .try_wait()
                .map_err(|_| DriverError::Ollama)?
                .is_none()
        {
            return Ok(());
        }
        stop_owned(&mut slot).await?;
        let runtime = config.prism_server_path.clone();
        let model = config.bonsai_model_directory.join(filename);
        let runtime_hash = tokio::time::timeout(Duration::from_secs(90), sha256(&runtime, cancel))
            .await
            .map_err(|_| DriverError::Ollama)??;
        let model_hash = tokio::time::timeout(Duration::from_secs(180), sha256(&model, cancel))
            .await
            .map_err(|_| DriverError::Ollama)??;
        if runtime_hash != SERVER_SHA256 || model_hash != expected_model_hash {
            return Err(DriverError::Ollama);
        }
        if *cancel.borrow() {
            return Err(DriverError::Cancelled);
        }
        // The pinned server reads LLAMA_API_KEY. Generate a distinct bearer key
        // for every owned child; it never enters Config, argv, or logs.
        let api_key = new_api_key();
        let endpoint = url::Url::parse(&config.prism_url).map_err(|_| DriverError::InvalidInput)?;
        let port = endpoint
            .port_or_known_default()
            .ok_or(DriverError::InvalidInput)?;
        let mut command = Command::new(runtime);
        let runtime_dir = config
            .prism_server_path
            .parent()
            .ok_or(DriverError::InvalidInput)?;
        let mut library_path = std::ffi::OsString::from(runtime_dir);
        if let Some(existing) = std::env::var_os("LD_LIBRARY_PATH") {
            library_path.push(":");
            library_path.push(existing);
        }
        command.env("LD_LIBRARY_PATH", library_path);
        command.env("LLAMA_API_KEY", &api_key);
        command
            .args([
                "--host",
                "127.0.0.1",
                "--port",
                &port.to_string(),
                "--model",
                model.to_str().ok_or(DriverError::InvalidInput)?,
                "--alias",
                alias,
                "--ctx-size",
                "8192",
                "--parallel",
                "1",
                "--batch-size",
                "1024",
                "--ubatch-size",
                "256",
                "--threads",
                "6",
                "--threads-batch",
                "6",
                "--gpu-layers",
                "all",
                "--flash-attn",
                "on",
                "--cache-type-k",
                "q8_0",
                "--cache-type-v",
                "q8_0",
                "--reasoning-format",
                "deepseek",
                "--min-p",
                "0",
                "--jinja",
                "--no-webui",
                "--verbosity",
                "3",
                "--cors-origins",
                "localhost",
                "--no-cors-credentials",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .kill_on_drop(true);
        if profile.uses_mtp() {
            command.args([
                "--spec-type",
                "draft-mtp",
                "--spec-draft-n-min",
                "0",
                "--spec-draft-n-max",
                "3",
                "--spec-draft-p-min",
                "0",
            ]);
        }
        #[cfg(unix)]
        command.process_group(0);
        let child = command.spawn().map_err(|_| DriverError::Ollama)?;
        let pid = child.id().ok_or(DriverError::Ollama)?;
        *slot = Some(OwnedServer {
            profile,
            api_key,
            pid,
            stopping: false,
            child,
        });
        Ok(())
    }

    async fn authorization_header(
        &self,
        profile: ModelProfile,
    ) -> Result<HeaderValue, DriverError> {
        let mut slot = self.child.lock().await;
        let owned = slot.as_mut().ok_or(DriverError::Ollama)?;
        if owned.profile != profile || owned.stopping {
            return Err(DriverError::Ollama);
        }
        if owned
            .child
            .try_wait()
            .map_err(|_| DriverError::Ollama)?
            .is_some()
        {
            return Err(DriverError::Ollama);
        }
        let mut header = HeaderValue::from_str(&format!("Bearer {}", owned.api_key))
            .map_err(|_| DriverError::Ollama)?;
        header.set_sensitive(true);
        Ok(header)
    }
}

impl super::prism::PrismProcessControl for PrismSupervisor {
    fn ensure<'a>(
        &'a self,
        config: &'a Config,
        profile: ModelProfile,
        cancel: &'a mut watch::Receiver<bool>,
    ) -> crate::domain::models::usage::Pending<'a, Result<(), DriverError>> {
        Box::pin(async move { PrismSupervisor::ensure(self, config, profile, cancel).await })
    }

    fn stop<'a>(&'a self) -> crate::domain::models::usage::Pending<'a, Result<(), DriverError>> {
        Box::pin(async move { PrismSupervisor::stop(self).await })
    }

    fn authorization_header<'a>(
        &'a self,
        profile: ModelProfile,
    ) -> crate::domain::models::usage::Pending<'a, Result<HeaderValue, DriverError>> {
        Box::pin(async move { PrismSupervisor::authorization_header(self, profile).await })
    }

    fn is_running<'a>(
        &'a self,
        profile: ModelProfile,
    ) -> crate::domain::models::usage::Pending<'a, Result<bool, DriverError>> {
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

async fn stop_owned(slot: &mut Option<OwnedServer>) -> Result<(), DriverError> {
    let Some(owned) = slot.as_mut() else {
        return Ok(());
    };
    owned.stopping = true;
    stop_handle(owned).await?;
    if slot.is_some() {
        *slot = None;
    }
    Ok(())
}

async fn stop_handle(owned: &mut OwnedServer) -> Result<(), DriverError> {
    let pid = owned.pid;
    #[cfg(unix)]
    // SAFETY: the child was started in a new process group owned by this instance.
    let _ = unsafe { libc::kill(-(pid as i32), libc::SIGTERM) };
    let result = tokio::time::timeout(Duration::from_secs(5), owned.child.wait()).await;
    if !matches!(result, Ok(Ok(_))) {
        #[cfg(unix)]
        // SAFETY: same owned process group as above; only our process group is signalled.
        let _ = unsafe { libc::kill(-(pid as i32), libc::SIGKILL) };
        let _ = owned.child.start_kill();
        tokio::time::timeout(Duration::from_secs(5), owned.child.wait())
            .await
            .map_err(|_| DriverError::Ollama)?
            .map_err(|_| DriverError::Ollama)?;
    }
    #[cfg(unix)]
    {
        let group_exited = tokio::time::timeout(Duration::from_secs(5), async {
            while process_group_exists(pid) {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .is_ok();
        if !group_exited {
            // SAFETY: pid is the process-group ID assigned to this owned child.
            let _ = unsafe { libc::kill(-(pid as i32), libc::SIGKILL) };
            tokio::time::timeout(Duration::from_secs(2), async {
                while process_group_exists(pid) {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            })
            .await
            .map_err(|_| DriverError::Ollama)?;
        }
    }
    Ok(())
}

#[cfg(unix)]
fn process_group_exists(pid: u32) -> bool {
    // SAFETY: signal 0 only checks the existence of the supervisor-owned group.
    if unsafe { libc::kill(-(pid as i32), 0) } == 0 {
        return true;
    }
    std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_prism_child_gets_a_fresh_api_key() {
        let first = new_api_key();
        let restarted = new_api_key();
        assert_ne!(first, restarted);
        assert_eq!(first.len(), 36);
        assert_eq!(restarted.len(), 36);
    }
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

    async fn prism_status(
        client: &reqwest::Client,
        method: reqwest::Method,
        url: String,
        authorization: Option<HeaderValue>,
    ) -> reqwest::StatusCode {
        let mut request = client.request(method.clone(), url);
        if let Some(mut value) = authorization {
            value.set_sensitive(true);
            request = request.header(reqwest::header::AUTHORIZATION, value);
        }
        if method == reqwest::Method::POST {
            request = request.json(&serde_json::json!({
                "model":"bonsai25-uncensored",
                "messages":[{"role":"user","content":"smoke"}],
                "max_tokens":1
            }));
        }
        let response = request.send().await.expect("auth smoke HTTP response");
        response.status()
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
    #[ignore = "requires separate explicit GPU authorization; checks the default Uncensored auth handshake only"]
    async fn gpu_prism_auth_smoke_uncensored() {
        use futures_util::FutureExt;

        let (config, queue, supervisor, _router) = evaluation_model_router_setup().await;
        // Exercise the actual managed Prism driver and Scheduler, but avoid a
        // ModelRouter transition here: its Ollama unload path must not touch
        // models that could appear after the initial empty-state preflight.
        let prism = Prism::managed(config.clone(), queue, supervisor.clone());
        let profile = ModelProfile::Bonsai25Uncensored;
        let base = config.prism_url.trim_end_matches('/').to_owned();
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(3))
            .timeout(Duration::from_secs(15))
            .build()
            .expect("bounded local auth smoke client");
        let mut child_pid = None;
        let result = std::panic::AssertUnwindSafe(async {
            let (_cancel_tx, mut cancel) = watch::channel(false);
            let context = cortex_eval_context(
                &config,
                "According to the synthetic record for KITE-204, what status and retry count are recorded? Answer in one sentence.",
                vec![synthetic_evidence(
                    "KITE-204 synthetic record",
                    "Synthetic record KITE-204: status READY; retry_count 2.",
                )],
            );
            let output = evaluation_output(profile, false);
            let (events, _event_rx) = mpsc::unbounded_channel();
            let (cancel_tx, cancel_rx) = watch::channel(false);
            let input = CallInput {
                role: "answer".into(),
                system: context.system.clone(),
                prompt: context.prompt.clone(),
                images: vec![],
                temperature: 0.0,
                max_tokens: 128,
                context_tokens: 8192,
                prefix: None,
                allow_tools: false,
                tool_history: vec![],
            };
            let answer_call = prism.call(&output, input, events, cancel_rx);
            tokio::pin!(answer_call);
            let mut monitor = tokio::time::interval(Duration::from_secs(5));
            let answer = loop {
                tokio::select! {
                    biased;
                    outcome = &mut answer_call => break outcome.expect("managed Prism answer"),
                    _ = monitor.tick() => {
                        if evaluation_resources_ok(700).is_err() {
                            let _ = cancel_tx.send(true);
                        }
                    }
                }
            };
            assert_measured_answer(&answer);
            assert!(!answer.text.trim().is_empty());
            child_pid = supervisor.child.lock().await.as_ref().map(|owned| owned.pid);
            let pid = child_pid.expect("owned Prism child PID");
            let authorization = supervisor
                .authorization_header(profile)
                .await
                .expect("live matching child bearer");
            assert!(authorization.is_sensitive());
            let secret = authorization
                .to_str()
                .expect("ASCII bearer header")
                .strip_prefix("Bearer ")
                .expect("bearer scheme")
                .to_owned();
            assert!(!format!("{authorization:?}").contains(&secret));
            let commandline = std::fs::read(format!("/proc/{pid}/cmdline"))
                .expect("owned runtime command line");
            assert!(!commandline
                .windows(secret.len())
                .any(|window| window == secret.as_bytes()));

            let authenticated_props = client
                .get(format!("{base}/props"))
                .header(reqwest::header::AUTHORIZATION, authorization.clone())
                .send()
                .await
                .expect("authenticated props request");
            assert!(authenticated_props.status().is_success());
            let props: serde_json::Value = authenticated_props
                .json()
                .await
                .expect("authenticated props JSON");
            assert!(props["model_path"]
                .as_str()
                .is_some_and(|path| path.ends_with(profile.artifact_filename().unwrap())));
            let authenticated_models = client
                .get(format!("{base}/v1/models"))
                .header(reqwest::header::AUTHORIZATION, authorization.clone())
                .send()
                .await
                .expect("authenticated models request");
            assert!(authenticated_models.status().is_success());
            let models: serde_json::Value = authenticated_models
                .json()
                .await
                .expect("authenticated models JSON");
            assert!(models["data"].as_array().is_some_and(|data| data
                .iter()
                .any(|model| model["id"] == profile.runtime_alias().unwrap())));
            supervisor
                .ensure(&config, profile, &mut cancel)
                .await
                .expect("same healthy profile remains owned");
            assert_eq!(
                supervisor.child.lock().await.as_ref().map(|owned| owned.pid),
                Some(pid),
                "managed answer reused the same owned process"
            );
            assert_eq!(
                supervisor.authorization_header(profile).await.unwrap(),
                authorization,
                "managed answer reused the same per-child bearer"
            );

            let mut rejected_statuses = Vec::new();
            for (method, path) in [
                (reqwest::Method::GET, "/props"),
                (reqwest::Method::GET, "/v1/models"),
                (reqwest::Method::POST, "/v1/chat/completions"),
            ] {
                let missing =
                    prism_status(&client, method.clone(), format!("{base}{path}"), None).await;
                let mut wrong = HeaderValue::from_static("Bearer invalid-auth-smoke-key");
                wrong.set_sensitive(true);
                let invalid = prism_status(
                    &client,
                    method,
                    format!("{base}{path}"),
                    Some(wrong),
                )
                .await;
                rejected_statuses.push((path, missing.as_u16(), invalid.as_u16()));
            }
            eprintln!(
                "AUTH_SMOKE_REJECTION_STATUSES {}",
                serde_json::json!({"missing_and_wrong_key":rejected_statuses})
            );
            assert!(
                rejected_statuses
                    .iter()
                    .all(|(_, missing, invalid)| *missing == 401 && *invalid == 401),
                "all three API routes must reject missing and incorrect keys"
            );

            let evil_preflight = client
                .request(reqwest::Method::OPTIONS, format!("{base}/v1/chat/completions"))
                .header(reqwest::header::ORIGIN, "https://untrusted.example")
                .header(reqwest::header::ACCESS_CONTROL_REQUEST_METHOD, "POST")
                .send()
                .await
                .expect("bounded denied-origin CORS preflight");
            let evil_origin_header = evil_preflight
                .headers()
                .get(reqwest::header::ACCESS_CONTROL_ALLOW_ORIGIN)
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned);
            let evil_credentials = evil_preflight
                .headers()
                .get(reqwest::header::ACCESS_CONTROL_ALLOW_CREDENTIALS)
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned);
            let localhost_preflight = client
                .request(reqwest::Method::OPTIONS, format!("{base}/v1/chat/completions"))
                .header(reqwest::header::ORIGIN, "http://localhost:5173")
                .header(reqwest::header::ACCESS_CONTROL_REQUEST_METHOD, "POST")
                .send()
                .await
                .expect("bounded localhost CORS preflight");
            let localhost_origin_header = localhost_preflight
                .headers()
                .get(reqwest::header::ACCESS_CONTROL_ALLOW_ORIGIN)
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned);
            let localhost_credentials = localhost_preflight
                .headers()
                .get(reqwest::header::ACCESS_CONTROL_ALLOW_CREDENTIALS)
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned);
            eprintln!(
                "AUTH_SMOKE_CORS {}",
                serde_json::json!({
                    "untrusted_origin_reflected":evil_origin_header.as_deref(),
                    "untrusted_credentials":evil_credentials.as_deref(),
                    "localhost_origin":localhost_origin_header.as_deref(),
                    "localhost_credentials":localhost_credentials.as_deref()
                })
            );
            assert!(evil_origin_header.is_none());
            assert!(evil_credentials.as_deref().is_none_or(|value| value == "false"));
            assert_eq!(localhost_origin_header.as_deref(), Some("http://localhost:5173"));
            assert!(localhost_credentials
                .as_deref()
                .is_none_or(|value| value == "false"));
            eprintln!(
                "AUTH_SMOKE_RESULT {}",
                serde_json::json!({
                    "profile": profile.id(),
                    "pid_reused": pid,
                    "child_key_reused": true,
                    "missing_and_wrong_key_statuses": [401, 401],
                    "protected_paths": ["/props", "/v1/models", "/v1/chat/completions"],
                    "allowed_origin": "http://localhost:5173",
                    "untrusted_origin_reflected": false,
                    "cors_credentials": false,
                    "answer_nonempty": true,
                    "usage_measured": answer.usage_measured,
                    "output_tokens": answer.usage.output_tokens,
                    "ttft_ms": answer.usage.time_to_first_token_ms,
                    "total_ms": answer.usage.total_ms,
                    "text": answer.text,
                    "full_secret_logged": false
                })
            );
        })
        .catch_unwind()
        .await;

        let stop_result = supervisor.stop().await;
        let child_absent = supervisor.child.lock().await.is_none();
        let ollama_state = ollama_loaded(&config).await;
        stop_result.expect("stop and reap only the owned Prism child");
        #[cfg(unix)]
        let owned_process_group_absent = child_pid.map(|pid| !process_group_exists(pid));
        #[cfg(not(unix))]
        let owned_process_group_absent = None;
        eprintln!(
            "AUTH_SMOKE_CLEANUP {}",
            serde_json::json!({
                "child_slot_empty":child_absent,
                "owned_process_group_absent":owned_process_group_absent,
                "ollama_models":ollama_state.as_ref().ok()
            })
        );
        assert!(child_absent, "owned Prism child slot is empty after stop");
        assert!(
            supervisor.authorization_header(profile).await.is_err(),
            "stopped child no longer exposes a bearer"
        );
        if let Some(pid) = child_pid {
            #[cfg(unix)]
            assert!(
                !process_group_exists(pid),
                "owned Prism process group is reaped"
            );
        }
        assert!(
            ollama_state.expect("Ollama postcheck").is_empty(),
            "auth smoke never loads/unloads T0 or disturbs Ollama"
        );
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

    #[tokio::test]
    async fn stop_reaps_only_the_owned_process_group() {
        let mut command = Command::new("/usr/bin/sleep");
        command
            .arg("30")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        #[cfg(unix)]
        command.process_group(0);
        let child = command.spawn().expect("test child");
        let pid = child.id().unwrap();
        let supervisor = PrismSupervisor {
            child: Mutex::new(Some(OwnedServer {
                profile: ModelProfile::Bonsai25Official,
                api_key: new_api_key(),
                pid,
                stopping: false,
                child,
            })),
        };
        tokio::time::timeout(Duration::from_secs(2), supervisor.stop())
            .await
            .expect("bounded stop")
            .unwrap();
        assert!(supervisor.child.lock().await.is_none());
    }

    #[tokio::test]
    async fn same_profile_reuses_child_and_switch_stops_before_invalid_artifact_fails() {
        let mut command = Command::new("/usr/bin/sleep");
        command
            .arg("30")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        #[cfg(unix)]
        command.process_group(0);
        let child = command.spawn().expect("test child");
        let pid = child.id().unwrap();
        let supervisor = PrismSupervisor {
            child: Mutex::new(Some(OwnedServer {
                profile: ModelProfile::Bonsai25Official,
                api_key: new_api_key(),
                pid,
                stopping: false,
                child,
            })),
        };
        let config = Config::default();
        let (_cancel_tx, mut cancel) = watch::channel(false);
        supervisor
            .ensure(&config, ModelProfile::Bonsai25Official, &mut cancel)
            .await
            .unwrap();
        supervisor
            .ensure(&config, ModelProfile::Bonsai25Official, &mut cancel)
            .await
            .unwrap();
        assert_eq!(supervisor.child.lock().await.as_ref().unwrap().pid, pid);
        let first_header = supervisor
            .authorization_header(ModelProfile::Bonsai25Official)
            .await
            .unwrap();
        assert!(first_header.is_sensitive());
        let first_secret = std::str::from_utf8(first_header.as_bytes()).unwrap();
        assert!(!format!("{first_header:?}").contains(first_secret));
        assert_eq!(
            supervisor
                .authorization_header(ModelProfile::Bonsai25Official)
                .await
                .unwrap(),
            first_header,
            "a healthy same-profile child keeps its key"
        );
        assert!(
            supervisor
                .authorization_header(ModelProfile::Bonsai25Uncensored)
                .await
                .is_err()
        );

        assert!(
            supervisor
                .ensure(&config, ModelProfile::Bonsai25Uncensored, &mut cancel)
                .await
                .is_err()
        );
        assert!(supervisor.child.lock().await.is_none());
        assert!(
            supervisor
                .authorization_header(ModelProfile::Bonsai25Official)
                .await
                .is_err()
        );
        #[cfg(unix)]
        assert!(!process_group_exists(pid));

        let mut replacement = Command::new("/usr/bin/sleep");
        replacement
            .arg("30")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        #[cfg(unix)]
        replacement.process_group(0);
        let child = replacement.spawn().expect("replacement test child");
        let replacement_pid = child.id().unwrap();
        let replacement_key = new_api_key();
        *supervisor.child.lock().await = Some(OwnedServer {
            profile: ModelProfile::Bonsai25Uncensored,
            api_key: replacement_key.clone(),
            pid: replacement_pid,
            stopping: false,
            child,
        });
        let replacement_header = supervisor
            .authorization_header(ModelProfile::Bonsai25Uncensored)
            .await
            .unwrap();
        assert_ne!(replacement_header, first_header);
        assert_eq!(
            std::str::from_utf8(replacement_header.as_bytes()).unwrap(),
            format!("Bearer {replacement_key}")
        );
        supervisor.stop().await.unwrap();
        assert!(
            supervisor
                .authorization_header(ModelProfile::Bonsai25Uncensored)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn authorization_rejects_missing_or_exited_owned_child() {
        let supervisor = PrismSupervisor::new();
        assert!(
            supervisor
                .authorization_header(ModelProfile::Bonsai25Official)
                .await
                .is_err()
        );

        let mut command = Command::new("/usr/bin/true");
        #[cfg(unix)]
        command.process_group(0);
        let mut child = command.spawn().expect("short-lived test child");
        let pid = child.id().unwrap();
        child.wait().await.unwrap();
        *supervisor.child.lock().await = Some(OwnedServer {
            profile: ModelProfile::Bonsai25Official,
            api_key: new_api_key(),
            pid,
            stopping: false,
            child,
        });
        assert!(
            supervisor
                .authorization_header(ModelProfile::Bonsai25Official)
                .await
                .is_err()
        );
        supervisor.child.lock().await.take();
    }

    #[tokio::test]
    async fn cancelled_stop_marks_same_profile_unreusable_until_cleanup_retries() {
        use tokio::io::AsyncBufReadExt;
        let mut command = Command::new("/usr/bin/python3");
        command
            .args([
                "-u",
                "-c",
                "import signal,time; signal.signal(signal.SIGTERM, signal.SIG_IGN); print('ready', flush=True); time.sleep(30)",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        #[cfg(unix)]
        command.process_group(0);
        let mut child = command.spawn().expect("test child");
        let pid = child.id().unwrap();
        let mut stdout = tokio::io::BufReader::new(child.stdout.take().unwrap());
        let mut ready = String::new();
        tokio::time::timeout(Duration::from_secs(2), stdout.read_line(&mut ready))
            .await
            .expect("child initialized")
            .unwrap();
        assert_eq!(ready.trim(), "ready");
        // Restore the child handle after reading its readiness line.
        child.stdout = Some(stdout.into_inner());
        let supervisor = std::sync::Arc::new(PrismSupervisor {
            child: Mutex::new(Some(OwnedServer {
                profile: ModelProfile::Bonsai25Official,
                api_key: new_api_key(),
                pid,
                stopping: false,
                child,
            })),
        });
        assert!(
            tokio::time::timeout(Duration::from_millis(100), supervisor.stop())
                .await
                .is_err(),
            "ignored SIGTERM keeps the stop future pending"
        );
        assert!(
            supervisor
                .child
                .lock()
                .await
                .as_ref()
                .is_some_and(|owned| owned.stopping)
        );
        assert!(
            supervisor
                .authorization_header(ModelProfile::Bonsai25Official)
                .await
                .is_err()
        );
        assert!(
            !supervisor
                .is_running(ModelProfile::Bonsai25Official)
                .await
                .unwrap()
        );
        let config = Config::default();
        let (_cancel_tx, mut cancel) = watch::channel(false);
        assert!(
            supervisor
                .ensure(&config, ModelProfile::Bonsai25Official, &mut cancel)
                .await
                .is_err()
        );
        assert!(supervisor.child.lock().await.is_none());
        #[cfg(unix)]
        assert!(!process_group_exists(pid));
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
}

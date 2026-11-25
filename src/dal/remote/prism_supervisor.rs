//! Owns the one Prism server process and one allowlisted loaded Bonsai artifact.
//! The server is never started at application boot and is not a system service.
use crate::{
    core::config::Config,
    domain::models::{driver::DriverError, model_profile::ModelProfile},
};
use sha2::{Digest, Sha256};
use std::{path::Path, process::Stdio, time::Duration};
use tokio::{
    io::AsyncReadExt,
    process::{Child, Command},
    sync::{Mutex, watch},
};

const SERVER_SHA256: &str = "89e7f20b23b97bf77b91a0143a168763f7835fd6e0a7a7a23bb70951d991c02c";

struct OwnedServer {
    profile: ModelProfile,
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
            pid,
            stopping: false,
            child,
        });
        Ok(())
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
    use crate::{
        dal::remote::{model_router::ModelRouter, ollama::Ollama, prism::Prism},
        domain::{
            models::{
                driver::{CallInput, DriverEvent, ModelDriver, OutputModel},
                identity::Caller,
                model_profile::ModelProfile,
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

    fn cortex_eval_input(config: &Config, question: &str) -> CallInput {
        let assembled = crate::domain::services::context::assemble(
            config,
            crate::domain::models::context::ContextInput {
                context_tokens: Some(8192),
                output_tokens: Some(256),
                question: question.into(),
                ..Default::default()
            },
        )
        .expect("production Cortex context assembly");
        CallInput {
            role: "answer".into(),
            system: assembled.system,
            prompt: assembled.prompt,
            images: vec![],
            temperature: 0.0,
            max_tokens: 256,
            context_tokens: 8192,
            prefix: None,
            allow_tools: false,
            tool_history: vec![],
        }
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

    async fn evaluation_call(
        router: &ModelRouter,
        output: &OutputModel,
        input: CallInput,
    ) -> crate::domain::models::driver::CallOutcome {
        let (events, mut receiver) = mpsc::unbounded_channel();
        let (cancel_tx, cancel) = watch::channel(false);
        let call = router.call(output, input, events, cancel);
        tokio::pin!(call);
        let deadline = tokio::time::sleep(Duration::from_secs(240));
        tokio::pin!(deadline);
        let mut monitor = tokio::time::interval(Duration::from_secs(5));
        let mut saw_token = false;
        loop {
            tokio::select! {
                result = &mut call => return result.expect("model call completed successfully"),
                event = receiver.recv() => if matches!(event, Some(DriverEvent::Token { .. })) { saw_token = true; },
                _ = monitor.tick() => {
                    if evaluation_resources_ok(700).is_err() {
                        let _ = cancel_tx.send(true);
                    }
                }
                _ = &mut deadline => panic!("bounded evaluation call exceeded 240 seconds; inspect owned supervisor cleanup before retry"),
            }
            let _ = saw_token;
            evaluation_resources_ok(700).expect("resource pressure during call");
        }
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

    #[tokio::test]
    #[ignore = "requires explicit global heavy/GPU locks; real local models and runtime"]
    async fn gpu_model_router_acceptance_uncensored_official_t0_and_cancel() {
        use futures_util::FutureExt;
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
        let router = Arc::new(ModelRouter::new(ollama, prism, queue));
        let mut owned_t0 = false;
        let result = std::panic::AssertUnwindSafe(async {
            let uncensored = evaluation_output(ModelProfile::Bonsai25Uncensored, false);
            let cold = evaluation_call(&router, &uncensored,
                eval_input("In English, explain in one sentence what a Rust Result represents.", vec![], None, false)).await;
            assert!(!cold.text.trim().is_empty());
            assert_measured_answer(&cold);
            eprintln!("EVAL {}", serde_json::json!({"case":"uncensored-en-cold","profile":uncensored.model_id(),"ttft_ms":cold.usage.time_to_first_token_ms,"input_tokens":cold.usage.input_tokens,"output_tokens":cold.usage.output_tokens,"usage_measured":cold.usage_measured,"total_ms":cold.usage.total_ms,"tokens_per_second":cold.tokens_per_second,"text":cold.text}));
            let first_pid = supervisor.child.lock().await.as_ref().expect("owned uncensored child").pid;
            let first_start = process_start_ticks(first_pid).expect("read owned Prism process identity");

            let mut prefix_input = cortex_eval_input(&config, "Continue this answer with one short clause.");
            prefix_input.prefix = Some("A Rust Result is an enum that represents ".into());
            let prefix = evaluation_call(&router, &uncensored, prefix_input).await;
            assert_measured_answer(&prefix);
            assert_eq!(supervisor.child.lock().await.as_ref().expect("same owned child").pid, first_pid,
                "healthy same-profile calls reuse one process");
            assert_eq!(process_start_ticks(first_pid).as_deref(), Some(first_start.as_str()),
                "same-profile calls preserve the original process instance");
            eprintln!("EVAL {}", serde_json::json!({"case":"uncensored-prefix","ttft_ms":prefix.usage.time_to_first_token_ms,"output_tokens":prefix.usage.output_tokens,"usage_measured":prefix.usage_measured,"total_ms":prefix.usage.total_ms,"tokens_per_second":prefix.tokens_per_second,"text":prefix.text}));

            let json = evaluation_call(&router, &uncensored,
                cortex_eval_input(&config, "Return only JSON with keys ok (boolean) and count (integer), set to true and 3.")).await;
            assert_measured_answer(&json);
            let parsed: serde_json::Value = serde_json::from_str(json.text.trim()).expect("valid JSON answer");
            assert_eq!(parsed["ok"], true); assert_eq!(parsed["count"], 3);
            eprintln!("EVAL {}", serde_json::json!({"case":"uncensored-json","text":json.text,"usage_measured":json.usage_measured}));

            let mut tool_input = cortex_eval_input(&config,
                "Use the view_file tool to read the synthetic path /evaluation-only/no-such-file.txt. Do not claim it was read.");
            tool_input.allow_tools = true;
            let tool = evaluation_call(&router, &uncensored, tool_input).await;
            let tool_request = tool.tool_request.expect("model should return a synthetic tool request");
            assert_eq!(tool_request.name.as_str(), "view_file");
            assert_eq!(tool_request.arguments["path"], "/evaluation-only/no-such-file.txt");
            assert!(tool.usage_measured && tool.usage.output_tokens > 0 && tool.usage.total_ms > 0);
            eprintln!("EVAL {}", serde_json::json!({"case":"uncensored-tool-double","tool":tool_request.name.as_str(),"arguments":tool_request.arguments,"effects":"none"}));

            for (language, prompt) in [
                ("en", "Explain in one short sentence how to handle a missing optional value in Rust."),
                ("pt-BR", "Em uma frase curta, explique como tratar um valor opcional ausente em Rust."),
                ("th", "ตอบเป็นภาษาไทยเท่านั้น: อธิบายสั้น ๆ ว่า Rust ใช้ Option เพื่ออะไร"),
            ] {
                let outcome = evaluation_call(&router, &uncensored, cortex_eval_input(&config, prompt)).await;
                assert_measured_answer(&outcome);
                let han = outcome.text.chars().filter(|c| ('\u{4e00}'..='\u{9fff}').contains(c)).count();
                let thai = outcome.text.chars().filter(|c| ('\u{0e00}'..='\u{0e7f}').contains(c)).count();
                eprintln!("EVAL {}", serde_json::json!({"case":"atomic-language","language":language,"han_codepoints":han,"thai_codepoints":thai,"text":outcome.text,"known_thai_mixed_chinese_is_quality_failure":language=="th" && han>0}));
                assert!(outcome.error.is_none());
                assert!(!outcome.text.trim().is_empty());
            }

            let official = evaluation_output(ModelProfile::Bonsai25Official, false);
            let answer = evaluation_call(&router, &official,
                eval_input("In English, explain in one sentence what a Rust Result represents.", vec![], None, false)).await;
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
            eprintln!("EVAL {}", serde_json::json!({"case":"official-en-switch","profile":official.model_id(),"pid":official_pid,"ttft_ms":answer.usage.time_to_first_token_ms,"total_ms":answer.usage.total_ms,"text":answer.text}));
            drop(official_child);
            let mut official_prefix_input = cortex_eval_input(&config, "Continue this answer with one short clause.");
            official_prefix_input.prefix = Some("A Rust Result is an enum that represents ".into());
            let official_prefix = evaluation_call(&router, &official, official_prefix_input).await;
            assert_measured_answer(&official_prefix);
            assert_eq!(supervisor.child.lock().await.as_ref().expect("official same-profile child").pid, official_pid);
            eprintln!("EVAL {}", serde_json::json!({"case":"official-prefix","ttft_ms":official_prefix.usage.time_to_first_token_ms,"usage_measured":official_prefix.usage_measured,"text":official_prefix.text}));
            let mut official_tool_input = cortex_eval_input(&config,
                "Use the view_file tool to read the synthetic path /evaluation-only/no-such-file.txt. Do not claim it was read.");
            official_tool_input.allow_tools = true;
            let official_tool = evaluation_call(&router, &official, official_tool_input).await;
            let official_tool_request = official_tool.tool_request.expect("official profile returns the synthetic tool call");
            assert_eq!(official_tool_request.name.as_str(), "view_file");
            assert_eq!(official_tool_request.arguments["path"], "/evaluation-only/no-such-file.txt");
            assert!(official_tool.usage_measured && official_tool.usage.output_tokens > 0);
            eprintln!("EVAL {}", serde_json::json!({"case":"official-tool-double","tool":official_tool_request.name.as_str(),"arguments":official_tool_request.arguments,"effects":"none","usage_measured":official_tool.usage_measured}));
            let official_thai = evaluation_call(&router, &official,
                cortex_eval_input(&config, "ตอบเป็นภาษาไทยเท่านั้น: อธิบายสั้น ๆ ว่า Rust ใช้ Option เพื่ออะไร")).await;
            assert_measured_answer(&official_thai);
            let official_han = official_thai.text.chars().filter(|c| ('\u{4e00}'..='\u{9fff}').contains(c)).count();
            let official_thai_chars = official_thai.text.chars().filter(|c| ('\u{0e00}'..='\u{0e7f}').contains(c)).count();
            let official_inspection = official_thai.inspection.as_ref().map(|value| (value.reason.clone(), value.tokens.len()));
            eprintln!("EVAL {}", serde_json::json!({"case":"official-thai-raw","han_codepoints":official_han,"thai_codepoints":official_thai_chars,"inspection":official_inspection,"usage_measured":official_thai.usage_measured,"text":official_thai.text,"postprocessing":false}));

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
            let grounded = evaluation_call(&router, &text_profile,
                eval_input(&format!("Use only this image description and state its three bands: {}", description), vec![], None, false)).await;
            assert_measured_answer(&grounded);
            assert!(grounded.text.to_lowercase().contains("red") && grounded.text.to_lowercase().contains("green") && grounded.text.to_lowercase().contains("blue"));
            eprintln!("EVAL {}", serde_json::json!({"case":"t0-prepass-to-t1-text","profile":text_profile.model_id(),"images_sent_to_t1":0,"text":grounded.text,"usage_measured":grounded.usage_measured}));

            for cancellation_profile in [ModelProfile::Bonsai25Uncensored, ModelProfile::Bonsai25Official] {
            let cancel_output = evaluation_output(cancellation_profile, false);
            let (events, mut event_rx) = mpsc::unbounded_channel();
            let (cancel_tx, cancel_rx) = watch::channel(false);
            let cancel_call = router.call(&cancel_output,
                eval_input("Write a detailed explanation with many examples of Rust ownership and borrowing.", vec![], None, false),
                events, cancel_rx);
            tokio::pin!(cancel_call);
            let queued_output = evaluation_output(ModelProfile::CortexT0, false);
            let (queued_events, mut queued_event_rx) = mpsc::unbounded_channel();
            let (queued_cancel_tx, queued_cancel_rx) = watch::channel(false);
            let queued_call = router.call(&queued_output,
                eval_input("In one sentence, define Rust ownership.", vec![], None, false),
                queued_events, queued_cancel_rx);
            tokio::pin!(queued_call);
            let mut saw_cancel_token = false;
            let mut saw_queued_wait = false;
            let mut pressure_detected = false;
            let mut cancel_result = None;
            let mut queued_result = None;
            let mut observation_monitor = tokio::time::interval(Duration::from_secs(5));
            let observation = tokio::time::timeout(Duration::from_secs(180), async {
                while !(saw_cancel_token && saw_queued_wait) && cancel_result.is_none() && queued_result.is_none() {
                    tokio::select! {
                        result = &mut cancel_call, if cancel_result.is_none() => cancel_result = Some(result),
                        result = &mut queued_call, if queued_result.is_none() => queued_result = Some(result),
                        event = event_rx.recv(), if !saw_cancel_token => {
                            saw_cancel_token = matches!(event, Some(DriverEvent::Token { .. }));
                        }
                        event = queued_event_rx.recv(), if !saw_queued_wait => {
                            saw_queued_wait = matches!(event, Some(DriverEvent::Waiting));
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
            }).await;
            let _ = cancel_tx.send(true);
            let drain = async {
                if cancel_result.is_none() { cancel_result = Some(cancel_call.as_mut().await); }
                if queued_result.is_none() { queued_result = Some(queued_call.as_mut().await); }
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
            }).await.is_ok();
            if !drained {
                eprintln!("GUARD_STOP cancellation/queued-T0 drain exceeded 90 seconds; stopping only the owned Prism child");
                supervisor.stop().await.expect("bounded owned Prism stop after drain timeout");
                tokio::time::timeout(Duration::from_secs(60), async {
                    if cancel_result.is_none() { cancel_result = Some(cancel_call.as_mut().await); }
                    if queued_result.is_none() { queued_result = Some(queued_call.as_mut().await); }
                }).await.expect("bounded model-future settlement after owned Prism stop");
            }
            let partial_result = cancel_result.expect("cancel call result collected");
            let queued_result = queued_result.expect("queued call result collected");
            assert!(observation.is_ok() && !pressure_detected && saw_cancel_token && saw_queued_wait,
                "both T1 token and queued T0 waiting state must be observed before cancellation");
            let partial = partial_result.expect("partial result survives cancellation");
            assert_eq!(partial.error, Some(crate::domain::models::driver::DriverError::Cancelled));
            assert!(!partial.text.is_empty());
            assert!(partial.tool_request.is_none());
            assert!(supervisor.child.lock().await.is_none(), "owned Prism must be reaped before permit release");
            let queued = queued_result.expect("queued T0 runs after Prism teardown");
            assert!(queued.error.is_none());
            eprintln!("EVAL {}", serde_json::json!({"case":"cancel-then-queued-t0","profile":cancellation_profile.id(),"partial_text":partial.text,"partial_error":"cancelled","queued_t0_text":queued.text,"queued_after_teardown":true}));
            }
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

        assert!(
            supervisor
                .ensure(&config, ModelProfile::Bonsai25Uncensored, &mut cancel)
                .await
                .is_err()
        );
        assert!(supervisor.child.lock().await.is_none());
        #[cfg(unix)]
        assert!(!process_group_exists(pid));
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

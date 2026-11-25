use crate::{
    core::config::Config,
    domain::{
        models::{
            driver::*,
            run_record::ModelCall,
            usage::{Pending, Usage},
        },
        services::balancer::{Permit, Scheduler},
    },
};
use chrono::Utc;
use futures_util::StreamExt;
use serde::Deserialize;
use serde_json::{Value, json};
use std::time::Instant;
use tokio::sync::{mpsc, watch};
#[derive(Deserialize)]
struct Running {
    name: String,
    context_length: Option<u64>,
}
#[derive(Deserialize)]
struct RunningModels {
    models: Vec<Running>,
}
#[derive(Default, Deserialize)]
struct Message {
    #[serde(default)]
    content: String,
    #[serde(default)]
    tool_calls: Vec<ProviderToolCall>,
}
#[derive(Deserialize)]
struct ProviderToolCall {
    function: ProviderFunction,
}
#[derive(Deserialize)]
struct ProviderFunction {
    name: String,
    arguments: Value,
}
#[derive(Default, Deserialize)]
struct Frame {
    message: Option<Message>,
    /// Text of a raw `/api/generate` frame (continuations).
    response: Option<String>,
    #[serde(default)]
    done: bool,
    error: Option<String>,
    prompt_eval_count: Option<u64>,
    eval_count: Option<u64>,
    eval_duration: Option<u64>,
    logprobs: Option<Value>,
}
#[derive(Clone)]
pub struct Ollama {
    client: reqwest::Client,
    config: Config,
    queue: Scheduler,
    prism_supervisor: Option<std::sync::Arc<super::prism_supervisor::PrismSupervisor>>,
}
struct Collected {
    text: String,
    prompt: Option<u64>,
    tokens: Option<u64>,
    eval_ns: Option<u64>,
    first_ms: Option<u64>,
    sent: bool,
    inspection: crate::domain::models::inspection::InspectionCapture,
    inspection_limit: usize,
    probabilities: bool,
    option_rejected: bool,
    tool_request: Option<crate::domain::models::tools::ToolRequest>,
}
impl Ollama {
    pub fn new(config: Config, queue: Scheduler) -> Self {
        Self {
            client: reqwest::Client::builder()
                .use_rustls_tls()
                .connect_timeout(config.ollama_connect_timeout)
                .build()
                .expect("Ollama client"),
            config,
            queue,
            prism_supervisor: None,
        }
    }
    pub fn with_prism_supervisor(
        mut self,
        supervisor: std::sync::Arc<super::prism_supervisor::PrismSupervisor>,
    ) -> Self {
        self.prism_supervisor = Some(supervisor);
        self
    }
    async fn post(&self, path: &str, body: &Value) -> Result<reqwest::Response, DriverError> {
        self.post_to(&self.config.ollama_url, path, body).await
    }
    async fn post_to(
        &self,
        base: &str,
        path: &str,
        body: &Value,
    ) -> Result<reqwest::Response, DriverError> {
        let response = self
            .client
            .post(format!("{}{path}", base.trim_end_matches('/')))
            .json(body)
            .send()
            .await
            .map_err(|_| DriverError::Ollama)?;
        if !response.status().is_success() {
            return Err(DriverError::Ollama);
        }
        Ok(response)
    }
    async fn running_models(&self) -> Result<RunningModels, DriverError> {
        tokio::time::timeout(std::time::Duration::from_secs(30), async {
            let response = self
                .client
                .get(format!(
                    "{}/api/ps",
                    self.config.ollama_url.trim_end_matches('/')
                ))
                .send()
                .await
                .map_err(|_| DriverError::Ollama)?;
            if !response.status().is_success() {
                return Err(DriverError::Ollama);
            }
            let bytes = response.bytes().await.map_err(|_| DriverError::Ollama)?;
            if bytes.len() > 64 * 1024 {
                return Err(DriverError::Ollama);
            }
            serde_json::from_slice(&bytes).map_err(|_| DriverError::Ollama)
        })
        .await
        .map_err(|_| DriverError::Ollama)?
    }
    async fn prepare(
        &self,
        _permit: &Permit,
        model: &str,
        context_tokens: u32,
        events: &mpsc::UnboundedSender<DriverEvent>,
    ) -> Result<u64, DriverError> {
        if let Some(supervisor) = &self.prism_supervisor {
            supervisor.stop().await?;
        }
        let response = self
            .client
            .get(format!(
                "{}/api/ps",
                self.config.ollama_url.trim_end_matches('/')
            ))
            .send()
            .await
            .map_err(|_| DriverError::Ollama)?;
        if !response.status().is_success() {
            return Err(DriverError::Ollama);
        }
        let loaded: RunningModels = response.json().await.map_err(|_| DriverError::Ollama)?;
        if loaded.models.len() == 1
            && loaded.models[0].name == model
            && loaded.models[0].context_length == Some(u64::from(context_tokens))
        {
            let _ = events.send(DriverEvent::ModelReady {
                model: model.into(),
                load_ms: 0,
            });
            return Ok(0);
        }
        let start = Instant::now();
        let _ = events.send(DriverEvent::ModelLoading {
            model: model.into(),
        });
        for previous in loaded.models {
            self.post(
                "/api/generate",
                &json!({"model":previous.name,"keep_alive":0,"stream":false}),
            )
            .await?;
        }
        let response=self.post("/api/generate",&json!({"model":model,"keep_alive":self.config.ollama_keep_alive,"stream":false,"options":{"num_ctx":context_tokens}})).await?;
        let value: Value = response.json().await.map_err(|_| DriverError::Ollama)?;
        if value.get("error").is_some_and(|v| !v.is_null()) {
            return Err(DriverError::Ollama);
        }
        let load_ms = start.elapsed().as_millis() as u64;
        let _ = events.send(DriverEvent::ModelReady {
            model: model.into(),
            load_ms,
        });
        Ok(load_ms)
    }
    pub async fn release_models_for_local_job(&self, _permit: &Permit) -> Result<(), DriverError> {
        if let Some(supervisor) = &self.prism_supervisor {
            supervisor.stop().await?;
        }
        self.unload_models().await
    }
    /// Unload local Ollama models while preserving a same-profile Prism server.
    pub async fn release_models_for_prism_transition(
        &self,
        _permit: &Permit,
    ) -> Result<(), DriverError> {
        self.unload_models().await
    }
    async fn unload_models(&self) -> Result<(), DriverError> {
        let models = self.running_models().await?;
        for model in models.models {
            tokio::time::timeout(
                std::time::Duration::from_secs(30),
                self.post(
                    "/api/generate",
                    &json!({"model":model.name,"keep_alive":0,"stream":false}),
                ),
            )
            .await
            .map_err(|_| DriverError::Ollama)??;
        }
        let remaining = self.running_models().await?;
        if !remaining.models.is_empty() {
            return Err(DriverError::Ollama);
        }
        Ok(())
    }
    fn frame(
        &self,
        line: &[u8],
        collected: &mut Collected,
        role: &str,
        events: &mpsc::UnboundedSender<DriverEvent>,
        start: Instant,
        allow_tools: bool,
    ) -> Result<bool, DriverError> {
        let frame: Frame = serde_json::from_slice(line).map_err(|_| DriverError::Ollama)?;
        if role == "answer" {
            collected
                .inspection
                .push(frame.logprobs.as_ref(), collected.inspection_limit);
        }
        if let Some(error) = frame.error {
            collected.option_rejected =
                collected.text.is_empty() && probability_option_error(&error);
            return Err(DriverError::Ollama);
        }
        let content = if let Some(message) = frame.message {
            if !message.tool_calls.is_empty() {
                if !allow_tools || message.tool_calls.len() != 1 || collected.tool_request.is_some()
                {
                    return Err(DriverError::InvalidInput);
                }
                let call = message
                    .tool_calls
                    .into_iter()
                    .next()
                    .expect("one tool call");
                collected.tool_request = Some(
                    crate::domain::models::tools::ToolRequest::from_provider(
                        &call.function.name,
                        call.function.arguments,
                    )
                    .map_err(|_| DriverError::InvalidInput)?,
                );
            }
            Some(message.content)
        } else {
            frame.response
        };
        if let Some(content) = content
            && !content.is_empty()
        {
            if collected.first_ms.is_none() {
                collected.first_ms = Some(start.elapsed().as_millis() as u64);
            }
            collected.text.push_str(&content);
            let _ = events.send(DriverEvent::Token {
                role: role.into(),
                text: content,
            });
        }
        if frame.prompt_eval_count.is_some() {
            collected.prompt = frame.prompt_eval_count;
        }
        if frame.eval_count.is_some() {
            collected.tokens = frame.eval_count;
        }
        if frame.eval_duration.is_some() {
            collected.eval_ns = frame.eval_duration;
        }
        Ok(frame.done)
    }
    async fn generate(
        &self,
        base: &str,
        model: &str,
        input: &CallInput,
        events: &mpsc::UnboundedSender<DriverEvent>,
        collected: &mut Collected,
        start: Instant,
    ) -> Result<(), DriverError> {
        let result = self
            .generate_attempt(base, model, input, events, collected, start)
            .await;
        if result.is_err()
            && collected.option_rejected
            && collected.text.is_empty()
            && collected.probabilities
            && input.role == "answer"
        {
            collected.probabilities = false;
            self.generate_attempt(base, model, input, events, collected, start)
                .await
        } else {
            result
        }
    }
    async fn generate_attempt(
        &self,
        base: &str,
        model: &str,
        input: &CallInput,
        events: &mpsc::UnboundedSender<DriverEvent>,
        collected: &mut Collected,
        start: Instant,
    ) -> Result<(), DriverError> {
        collected.sent = true;
        let options = json!({"num_ctx":input.context_tokens,"temperature":input.temperature,"num_predict":input.max_tokens});
        // A continuation (cortex_api#125) must extend the saved prefix. Chat
        // templates close the assistant turn, so a raw ChatML prompt is used.
        let (path, mut body) = match &input.prefix {
            Some(prefix) => (
                "/api/generate",
                json!({"model":model,"prompt":chatml(&input.system,&input.prompt,prefix),"raw":true,"stream":true,"keep_alive":self.config.ollama_keep_alive,"options":options}),
            ),
            None => {
                let mut messages = vec![];
                if !input.system.is_empty() {
                    messages.push(json!({"role":"system","content":input.system}));
                }
                messages.push(json!({"role":"user","content":input.prompt,"images":input.images}));
                for exchange in &input.tool_history {
                    messages.push(json!({"role":"assistant","content":"","tool_calls":[{"function":{"name":exchange.request.name.as_str(),"arguments":exchange.request.arguments}}]}));
                    messages.push(json!({"role":"tool","tool_name":exchange.request.name.as_str(),"content":exchange.output}));
                }
                (
                    "/api/chat",
                    json!({"model":model,"messages":messages,"stream":true,"keep_alive":self.config.ollama_keep_alive,"options":options}),
                )
            }
        };
        if input.allow_tools && input.role == "answer" && input.prefix.is_none() {
            // Tool-call argument objects are provider-complete, never streamed fragments.
            body["stream"] = json!(false);
            body["tools"] = crate::domain::models::tools::provider_definitions();
            collected.probabilities = false;
        }
        if input.role == "answer" && collected.probabilities {
            body["logprobs"] = json!(true);
            body["top_logprobs"] = json!(crate::domain::models::inspection::ALTERNATIVES);
        }
        let url = format!("{}{path}", base.trim_end_matches('/'));
        let response = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|_| DriverError::Ollama)?;
        let response = if !response.status().is_success() {
            let status = response.status();
            let error = response
                .json::<Value>()
                .await
                .ok()
                .and_then(|v| {
                    v.get("error")
                        .and_then(Value::as_str)
                        .map(str::to_lowercase)
                })
                .unwrap_or_default();
            if input.role == "answer"
                && matches!(status.as_u16(), 400 | 422 | 500 | 501)
                && error.contains("logprob")
                && [
                    "unsupported",
                    "not supported",
                    "unknown",
                    "invalid",
                    "not implemented",
                ]
                .iter()
                .any(|w| error.contains(w))
            {
                body.as_object_mut().unwrap().remove("logprobs");
                body.as_object_mut().unwrap().remove("top_logprobs");
                collected.probabilities = false;
                self.post_to(base, path, &body).await?
            } else {
                return Err(DriverError::Ollama);
            }
        } else {
            response
        };
        let mut stream = response.bytes_stream();
        let mut buffer = vec![];
        while let Some(chunk) = stream.next().await {
            buffer.extend_from_slice(&chunk.map_err(|_| DriverError::Ollama)?);
            if buffer.len() > self.config.ollama_max_frame_bytes {
                return Err(DriverError::Ollama);
            }
            while let Some(end) = buffer.iter().position(|b| *b == b'\n') {
                let line: Vec<_> = buffer.drain(..=end).collect();
                if line.iter().all(u8::is_ascii_whitespace) {
                    continue;
                }
                if self.frame(
                    &line,
                    collected,
                    &input.role,
                    events,
                    start,
                    input.allow_tools,
                )? {
                    return Ok(());
                }
            }
        }
        if !buffer.iter().all(u8::is_ascii_whitespace)
            && self.frame(
                &buffer,
                collected,
                &input.role,
                events,
                start,
                input.allow_tools,
            )?
        {
            return Ok(());
        }
        Err(DriverError::Ollama)
    }
}
/// ChatML prompt that ends inside the assistant turn, so the model continues
/// `prefix`. The configured Qwen text and vision models use ChatML.
fn chatml(system: &str, prompt: &str, prefix: &str) -> String {
    let mut text = String::new();
    if !system.is_empty() {
        text.push_str(&format!("<|im_start|>system\n{system}<|im_end|>\n"));
    }
    text.push_str(&format!(
        "<|im_start|>user\n{prompt}<|im_end|>\n<|im_start|>assistant\n{prefix}"
    ));
    text
}
fn probability_option_error(error: &str) -> bool {
    let error = error.to_lowercase();
    error.contains("logprob")
        && [
            "unsupported",
            "not supported",
            "unknown",
            "invalid",
            "not implemented",
        ]
        .iter()
        .any(|w| error.contains(w))
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
impl ModelDriver for Ollama {
    fn call<'a>(
        &'a self,
        output: &'a OutputModel,
        input: CallInput,
        events: mpsc::UnboundedSender<DriverEvent>,
        mut cancel: watch::Receiver<bool>,
    ) -> Pending<'a, Result<CallOutcome, DriverError>> {
        Box::pin(async move {
            if !input.temperature.is_finite()
                || input.temperature < 0.0
                || input.max_tokens == 0
                || u64::from(input.max_tokens) > self.config.model_context_tokens
                || (!output.is_vision() && !input.images.is_empty())
                || (input.allow_tools && input.role != "answer")
            {
                return Err(DriverError::InvalidInput);
            }
            let _ = events.send(DriverEvent::Waiting);
            // Remote vision is another Ollama URL; it does not use the MSI queue (7.10.5).
            let remote = match (&self.config.vision_location, output.is_vision()) {
                (crate::core::config::JobLocation::Remote(url), true) => Some(url.clone()),
                _ => None,
            };
            let permit = if remote.is_none() {
                Some(
                    tokio::select! {biased;_=cancelled(&mut cancel)=>return Err(DriverError::Cancelled),permit=self.queue.acquire(output.actor.clone(),output.output_id)=>permit.map_err(DriverError::Queue)?},
                )
            } else {
                None
            };
            let base = remote.unwrap_or_else(|| self.config.ollama_url.clone());
            let started_at = Utc::now();
            let start = Instant::now();
            let model = output.name(&self.config);
            let mut load_ms = 0;
            let mut collected = Collected {
                text: String::new(),
                prompt: None,
                tokens: None,
                eval_ns: None,
                first_ms: None,
                sent: false,
                inspection: Default::default(),
                inspection_limit: input.max_tokens as usize,
                probabilities: true,
                option_rejected: false,
                tool_request: None,
            };
            let result = tokio::select! {biased;_=cancelled(&mut cancel)=>Err(DriverError::Cancelled),result=async{if let Some(permit)=&permit{load_ms=self.prepare(permit,model,input.context_tokens,&events).await?;}self.generate(&base,model,&input,&events,&mut collected,start).await}=>result};
            let finished_at = Utc::now();
            let input_tokens = collected.prompt.unwrap_or_else(|| {
                if collected.sent {
                    self.config.estimate_tokens(&input.system)
                        + self.config.estimate_tokens(&input.prompt)
                        + self.config.chat_template_token_reserve
                } else {
                    0
                }
            });
            let usage_measured = collected.prompt.is_some() && collected.tokens.is_some();
            let output_tokens = collected
                .tokens
                .unwrap_or_else(|| self.config.estimate_tokens(&collected.text));
            let images = if collected.sent {
                input.images.len() as u32
            } else {
                0
            };
            let usage = ModelCall {
                images: Some(images),
                audio: None,
                role: input.role.clone(),
                model: model.into(),
                input_tokens,
                output_tokens,
                time_to_first_token_ms: collected.first_ms,
                total_ms: start.elapsed().as_millis() as u64,
                started_at,
                finished_at,
            };
            let used = Usage {
                input: input_tokens,
                output: output_tokens,
                images: u64::from(images),
                audio: 0,
            };
            let _ = match permit {
                Some(permit) => permit.finish(used, finished_at).await,
                None => {
                    self.queue
                        .record_remote(output.actor.clone(), output.output_id, used, finished_at)
                        .await
                }
            };
            let tokens_per_second = collected
                .eval_ns
                .filter(|ns| *ns > 0)
                .map(|ns| output_tokens as f64 * 1_000_000_000.0 / ns as f64);
            let inspection = (input.role == "answer").then(|| {
                collected
                    .inspection
                    .finish(model, &collected.text, collected.probabilities)
            });
            Ok(CallOutcome {
                inspection,
                text: collected.text,
                usage,
                error: result.err(),
                load_ms,
                tokens_per_second,
                tool_request: collected.tool_request,
                usage_measured,
            })
        })
    }
}

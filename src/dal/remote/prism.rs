//! Thin adapter for the loopback Prism/llama.cpp OpenAI-compatible endpoint.
//! The runtime owns one loaded Bonsai model; Cortex chooses only an allowlisted profile.
use crate::{
    core::config::Config,
    domain::{
        models::{
            driver::*,
            run_record::ModelCall,
            usage::{Pending, Usage},
        },
        services::balancer::Scheduler,
    },
};
use chrono::Utc;
use futures_util::StreamExt;
use serde_json::{Value, json};
use std::{collections::BTreeMap, time::Instant};
use tokio::sync::{mpsc, watch};

const MAX_FRAME_BYTES: usize = 1024 * 1024;

pub trait PrismProcessControl: Send + Sync {
    fn ensure<'a>(
        &'a self,
        config: &'a Config,
        profile: crate::domain::models::model_profile::ModelProfile,
        cancel: &'a mut watch::Receiver<bool>,
    ) -> Pending<'a, Result<(), DriverError>>;
    fn stop<'a>(&'a self) -> Pending<'a, Result<(), DriverError>>;
    fn is_running<'a>(
        &'a self,
        profile: crate::domain::models::model_profile::ModelProfile,
    ) -> Pending<'a, Result<bool, DriverError>>;
}

#[derive(Clone)]
pub struct Prism {
    client: reqwest::Client,
    queue: Scheduler,
    config: Config,
    supervisor: Option<std::sync::Arc<dyn PrismProcessControl>>,
}

struct GeneratedAnswer {
    text: String,
    first_ms: Option<u64>,
    prompt_tokens: Option<u64>,
    completion_tokens: Option<u64>,
    inspection: crate::domain::models::inspection::InspectionCapture,
    tool_request: Option<crate::domain::models::tools::ToolRequest>,
    probabilities: bool,
    error: Option<DriverError>,
}

impl Prism {
    pub fn new(config: Config, queue: Scheduler) -> Self {
        Self {
            client: reqwest::Client::builder()
                .use_rustls_tls()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .connect_timeout(std::time::Duration::from_secs(5))
                .timeout(std::time::Duration::from_secs(600))
                .build()
                .expect("Prism client"),
            queue,
            config,
            supervisor: None,
        }
    }

    pub fn managed(
        config: Config,
        queue: Scheduler,
        supervisor: std::sync::Arc<dyn PrismProcessControl>,
    ) -> Self {
        let mut prism = Self::new(config, queue);
        prism.supervisor = Some(supervisor);
        prism
    }
}

fn messages(input: &CallInput) -> Result<Vec<Value>, DriverError> {
    if !input.images.is_empty() || input.prefix.is_some() && !input.tool_history.is_empty() {
        return Err(DriverError::InvalidInput);
    }
    let mut messages = Vec::new();
    if !input.system.is_empty() {
        messages.push(json!({"role":"system","content":input.system}));
    }
    messages.push(json!({"role":"user","content":input.prompt}));
    if let Some(prefix) = &input.prefix {
        messages.push(json!({"role":"assistant","content":prefix}));
    }
    for exchange in &input.tool_history {
        messages.push(json!({"role":"assistant","content":"","tool_calls":[{"id":exchange.request.call_id.to_string(),"type":"function","function":{"name":exchange.request.name.as_str(),"arguments":exchange.request.arguments.to_string()}}]}));
        messages.push(json!({"role":"tool","tool_call_id":exchange.request.call_id.to_string(),"content":exchange.output}));
    }
    Ok(messages)
}

impl Prism {
    pub fn call_with_permit<'a>(
        &'a self,
        output: &'a OutputModel,
        input: CallInput,
        events: mpsc::UnboundedSender<DriverEvent>,
        cancel: watch::Receiver<bool>,
        permit: crate::domain::services::balancer::Permit,
    ) -> Pending<'a, Result<CallOutcome, DriverError>> {
        self.call_inner(output, input, events, cancel, Some(permit))
    }

    fn call_inner<'a>(
        &'a self,
        output: &'a OutputModel,
        input: CallInput,
        events: mpsc::UnboundedSender<DriverEvent>,
        mut cancel: watch::Receiver<bool>,
        existing_permit: Option<crate::domain::services::balancer::Permit>,
    ) -> Pending<'a, Result<CallOutcome, DriverError>> {
        Box::pin(async move {
            if !self.config.bonsai_enabled
                || !self.config.prism_url_is_loopback()
                || !output.profile().uses_prism()
                || !input.temperature.is_finite()
                || input.temperature < 0.0
                || input.max_tokens == 0
                || input.context_tokens > 8192
                || input.max_tokens > input.context_tokens
                || input.allow_tools && input.role != "answer"
            {
                return Err(DriverError::InvalidInput);
            }
            let messages = messages(&input)?;
            let mut body = json!({
                "model": output.profile().runtime_alias().ok_or(DriverError::InvalidInput)?,
                "messages": messages,
                "temperature": input.temperature,
                "max_tokens": input.max_tokens,
                "stream": true,
                "stream_options": {"include_usage": true},
                "chat_template_kwargs": {"enable_thinking": false},
                "continue_final_message": input.prefix.is_some(),
                "add_generation_prompt": input.prefix.is_none(),
            });
            if input.allow_tools {
                body["tools"] = crate::domain::models::tools::provider_definitions();
            }
            let probabilities = input.role == "answer";
            if probabilities {
                body["logprobs"] = json!(true);
                body["top_logprobs"] = json!(crate::domain::models::inspection::ALTERNATIVES);
            }
            let _ = events.send(DriverEvent::Waiting);
            let started_at = Utc::now();
            let start = Instant::now();
            let permit = match existing_permit {
                Some(permit) => permit,
                None => tokio::select! {
                    biased;
                    _ = cancelled(&mut cancel) => return Err(DriverError::Cancelled),
                    p = self.queue.acquire(output.actor.clone(), output.output_id) => p.map_err(DriverError::Queue)?,
                },
            };
            let profile = output.profile();
            if let Some(supervisor) = &self.supervisor {
                let _ = events.send(DriverEvent::ModelLoading {
                    model: profile
                        .runtime_alias()
                        .ok_or(DriverError::InvalidInput)?
                        .into(),
                });
                supervisor
                    .ensure(&self.config, profile, &mut cancel)
                    .await?;
            }
            let expected_alias = profile.runtime_alias().ok_or(DriverError::InvalidInput)?;
            let expected_file = profile
                .artifact_filename()
                .ok_or(DriverError::InvalidInput)?;
            let ready = tokio::time::timeout(
                std::time::Duration::from_secs(120),
                wait_for_ready(
                    &self.client,
                    &self.config.prism_url,
                    self.supervisor.as_deref(),
                    profile,
                    expected_alias,
                    expected_file,
                    input.context_tokens,
                    &mut cancel,
                ),
            )
            .await
            .unwrap_or(Err(DriverError::Ollama));
            if let Err(error) = ready {
                if let Some(supervisor) = &self.supervisor {
                    supervisor.stop().await?;
                }
                return Err(error);
            }
            let ready_ms = start.elapsed().as_millis() as u64;
            let _ = events.send(DriverEvent::ModelReady {
                model: expected_alias.into(),
                load_ms: ready_ms,
            });
            let mut body = body;
            let generated_result: Result<GeneratedAnswer, DriverError> = async {
                let mut request_probabilities = probabilities;
                let mut response = tokio::select! {
                    biased;
                    _ = cancelled(&mut cancel) => return Err(DriverError::Cancelled),
                    response = self.client.post(format!("{}/v1/chat/completions", self.config.prism_url.trim_end_matches('/'))).json(&body).send() => response.map_err(|_| DriverError::Ollama)?,
                };
                if !response.status().is_success() {
                    let status = response.status();
                    let error_value = match read_limited_json(response, 16 * 1024, &mut cancel).await {
                        Ok(value) => Some(value),
                        Err(DriverError::Cancelled) => return Err(DriverError::Cancelled),
                        Err(_) => None,
                    };
                    let error = error_value
                        .and_then(|v| {
                            v.pointer("/error/message")
                                .and_then(Value::as_str)
                                .map(str::to_ascii_lowercase)
                        })
                        .unwrap_or_default();
                    let unsupported_logprobs = status.is_client_error()
                        && error.contains("logprob")
                        && [
                            "unsupported",
                            "not supported",
                            "unknown",
                            "invalid",
                            "not implemented",
                        ]
                        .iter()
                        .any(|w| error.contains(w));
                    if !request_probabilities || !unsupported_logprobs {
                        return Err(DriverError::Ollama);
                    }
                    request_probabilities = false;
                    body.as_object_mut()
                        .expect("request object")
                        .remove("logprobs");
                    body.as_object_mut()
                        .expect("request object")
                        .remove("top_logprobs");
                    response = tokio::select! {
                        biased;
                        _ = cancelled(&mut cancel) => return Err(DriverError::Cancelled),
                        response = self.client.post(format!("{}/v1/chat/completions", self.config.prism_url.trim_end_matches('/'))).json(&body).send() => response.map_err(|_| DriverError::Ollama)?,
                    };
                    if !response.status().is_success() {
                        return Err(DriverError::Ollama);
                    }
                }
                let mut stream = response.bytes_stream();
                let mut buffer = Vec::new();
                let mut text = String::new();
                let mut first_ms = None;
                let mut prompt_tokens = None;
                let mut completion_tokens = None;
                let mut inspection = crate::domain::models::inspection::InspectionCapture::default();
                let mut calls = BTreeMap::<u64, (String, String, String)>::new();
                let mut done = false;
                let mut finish_seen = false;
                let mut stream_error = None;
                while !done && stream_error.is_none() {
                    let next = tokio::select! {
                        biased;
                        _ = cancelled(&mut cancel) => {
                            stream_error = Some(DriverError::Cancelled);
                            break;
                        },
                        next = stream.next() => next,
                    };
                    let Some(chunk) = next else { break };
                    let chunk = match chunk {
                        Ok(chunk) => chunk,
                        Err(_) => {
                            stream_error = Some(DriverError::Ollama);
                            break;
                        }
                    };
                    buffer.extend_from_slice(&chunk);
                    if buffer.len() > MAX_FRAME_BYTES {
                        stream_error = Some(DriverError::Ollama);
                        break;
                    }
                    while let Some(end) = buffer.iter().position(|b| *b == b'\n') {
                        let line: Vec<u8> = buffer.drain(..=end).collect();
                        let line = line.strip_prefix(b"data: ").unwrap_or(&line);
                        let line = line.trim_ascii();
                        if line.is_empty() {
                            continue;
                        }
                        if line == b"[DONE]" {
                            done = true;
                            break;
                        }
                        let frame: Value = match serde_json::from_slice(line) {
                            Ok(frame) => frame,
                            Err(_) => {
                                stream_error = Some(DriverError::Ollama);
                                break;
                            }
                        };
                        if let Some(usage) = frame.get("usage") {
                            prompt_tokens = usage
                                .get("prompt_tokens")
                                .and_then(Value::as_u64)
                                .or(prompt_tokens);
                            completion_tokens = usage
                                .get("completion_tokens")
                                .and_then(Value::as_u64)
                                .or(completion_tokens);
                        }
                        if let Some(choice) = frame
                            .get("choices")
                            .and_then(Value::as_array)
                            .and_then(|c| c.first())
                        {
                            let delta = choice.get("delta").unwrap_or(&Value::Null);
                            if input.role == "answer" {
                                inspection.push(
                                    choice.pointer("/logprobs/content"),
                                    input.max_tokens as usize,
                                );
                            }
                            if let Some(content) = delta
                                .get("content")
                                .and_then(Value::as_str)
                                .filter(|s| !s.is_empty())
                            {
                                first_ms.get_or_insert(start.elapsed().as_millis() as u64);
                                text.push_str(content);
                                let _ = events.send(DriverEvent::Token {
                                    role: input.role.clone(),
                                    text: content.into(),
                                });
                            }
                            if let Some(fragments) = delta.get("tool_calls").and_then(Value::as_array) {
                                for fragment in fragments {
                                    let index =
                                        fragment.get("index").and_then(Value::as_u64).unwrap_or(0);
                                    let entry = calls.entry(index).or_default();
                                    first_ms.get_or_insert(start.elapsed().as_millis() as u64);
                                    if let Some(id) = fragment.get("id").and_then(Value::as_str) {
                                        entry.0.push_str(id);
                                    }
                                    if let Some(name) =
                                        fragment.pointer("/function/name").and_then(Value::as_str)
                                    {
                                        entry.1.push_str(name);
                                    }
                                    if let Some(args) = fragment
                                        .pointer("/function/arguments")
                                        .and_then(Value::as_str)
                                    {
                                        entry.2.push_str(args);
                                    }
                                }
                            }
                            finish_seen |= choice.get("finish_reason").is_some_and(|r| !r.is_null());
                        }
                    }
                }
                if stream_error.is_none()
                    && (!done || !finish_seen || !buffer.iter().all(u8::is_ascii_whitespace))
                {
                    stream_error = Some(DriverError::Ollama);
                }
                let (tool_request, tool_error) = if stream_error.is_some() || calls.is_empty() {
                    (None, None)
                } else if !input.allow_tools || calls.len() != 1 {
                    (None, Some(DriverError::InvalidInput))
                } else {
                    let (_, (id, name, arguments)) = calls.into_iter().next().expect("one call");
                    let _provider_call_id = id;
                    match serde_json::from_str::<Value>(&arguments)
                        .map_err(|_| DriverError::InvalidInput)
                        .and_then(|arguments| {
                            crate::domain::models::tools::ToolRequest::from_provider(
                                &name, arguments,
                            )
                            .map_err(|_| DriverError::InvalidInput)
                        })
                    {
                        Ok(request) => (Some(request), None),
                        Err(error) => (None, Some(error)),
                    }
                };
                Ok(GeneratedAnswer {
                    text,
                    first_ms,
                    prompt_tokens,
                    completion_tokens,
                    inspection,
                    tool_request,
                    probabilities: request_probabilities,
                    error: stream_error.or(tool_error),
                })
            }.await;
            let GeneratedAnswer {
                text,
                first_ms,
                prompt_tokens,
                completion_tokens,
                inspection,
                tool_request,
                probabilities,
                error,
            } = match generated_result {
                Ok(generated) => generated,
                Err(error) => {
                    if let Some(supervisor) = &self.supervisor {
                        supervisor.stop().await?;
                    }
                    return Err(error);
                }
            };
            let mut error = error;
            if error.is_some()
                && let Some(supervisor) = &self.supervisor
                && supervisor.stop().await.is_err()
            {
                error = Some(DriverError::Ollama);
            }
            let finished_at = Utc::now();
            let tool_text = tool_request
                .as_ref()
                .map(|request| request.arguments.to_string())
                .unwrap_or_default();
            let in_count = prompt_tokens.unwrap_or_else(|| {
                self.config
                    .estimate_tokens(&input.system)
                    .saturating_add(self.config.estimate_tokens(&input.prompt))
                    .saturating_add(self.config.chat_template_token_reserve)
            });
            let out_count = completion_tokens.unwrap_or_else(|| {
                self.config
                    .estimate_tokens(&text)
                    .saturating_add(self.config.estimate_tokens(&tool_text))
            });
            let usage_measured = prompt_tokens.is_some() && completion_tokens.is_some();
            let usage = ModelCall {
                images: Some(0),
                audio: None,
                role: input.role.clone(),
                model: output.model_id().into(),
                input_tokens: in_count,
                output_tokens: out_count,
                time_to_first_token_ms: first_ms,
                total_ms: start.elapsed().as_millis() as u64,
                started_at,
                finished_at,
            };
            permit
                .finish(
                    Usage {
                        input: in_count,
                        output: out_count,
                        images: 0,
                        audio: 0,
                    },
                    finished_at,
                )
                .await
                .map_err(|_| {
                    DriverError::Queue(crate::domain::models::usage::QueueError::ServerFault)
                })?;
            let inspection = (input.role == "answer")
                .then(|| inspection.finish(output.model_id(), &text, probabilities));
            Ok(CallOutcome {
                inspection,
                text,
                usage,
                usage_measured,
                error,
                load_ms: start.elapsed().as_millis() as u64,
                tokens_per_second: completion_tokens
                    .filter(|_| start.elapsed().as_micros() > 0)
                    .map(|tokens| tokens as f64 * 1_000_000.0 / start.elapsed().as_micros() as f64),
                tool_request,
            })
        })
    }
}

impl ModelDriver for Prism {
    fn call<'a>(
        &'a self,
        output: &'a OutputModel,
        input: CallInput,
        events: mpsc::UnboundedSender<DriverEvent>,
        cancel: watch::Receiver<bool>,
    ) -> Pending<'a, Result<CallOutcome, DriverError>> {
        self.call_inner(output, input, events, cancel, None)
    }
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

async fn read_limited_json(
    response: reqwest::Response,
    limit: usize,
    cancel: &mut watch::Receiver<bool>,
) -> Result<Value, DriverError> {
    let mut stream = response.bytes_stream();
    let mut body = Vec::new();
    loop {
        let chunk = tokio::select! {
            biased;
            _ = cancelled(cancel) => return Err(DriverError::Cancelled),
            next = stream.next() => next,
        };
        let Some(chunk) = chunk else { break };
        let chunk = chunk.map_err(|_| DriverError::Ollama)?;
        if body.len().saturating_add(chunk.len()) > limit {
            return Err(DriverError::Ollama);
        }
        body.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&body).map_err(|_| DriverError::Ollama)
}

async fn readiness_json(
    client: &reqwest::Client,
    url: String,
    cancel: &mut watch::Receiver<bool>,
) -> Result<Option<Value>, DriverError> {
    let response = tokio::select! {
        biased;
        _ = cancelled(cancel) => return Err(DriverError::Cancelled),
        response = client.get(url).send() => match response {
            Ok(response) => response,
            Err(error) if error.is_connect() || error.is_timeout() => return Ok(None),
            Err(_) => return Err(DriverError::Ollama),
        },
    };
    if response.status() == reqwest::StatusCode::SERVICE_UNAVAILABLE {
        return Ok(None);
    }
    if !response.status().is_success() {
        return Err(DriverError::Ollama);
    }
    read_limited_json(response, 64 * 1024, cancel)
        .await
        .map(Some)
}

async fn wait_for_ready(
    client: &reqwest::Client,
    base: &str,
    supervisor: Option<&dyn PrismProcessControl>,
    profile: crate::domain::models::model_profile::ModelProfile,
    expected_alias: &str,
    expected_file: &str,
    context_tokens: u32,
    cancel: &mut watch::Receiver<bool>,
) -> Result<(), DriverError> {
    let base = base.trim_end_matches('/');
    loop {
        if let Some(supervisor) = supervisor
            && !supervisor.is_running(profile).await?
        {
            return Err(DriverError::Ollama);
        }
        let Some(props) = readiness_json(client, format!("{base}/props"), cancel).await? else {
            tokio::select! {
                biased;
                _ = cancelled(cancel) => return Err(DriverError::Cancelled),
                _ = tokio::time::sleep(std::time::Duration::from_millis(150)) => continue,
            }
        };
        let model_path = props.get("model_path").and_then(Value::as_str);
        if !model_path.is_some_and(|path| path.ends_with(expected_file)) {
            return Err(DriverError::Ollama);
        }
        let context_ready = props
            .pointer("/default_generation_settings/n_ctx")
            .and_then(Value::as_u64)
            .is_some_and(|ctx| ctx >= u64::from(context_tokens));
        if !context_ready {
            tokio::select! {
                biased;
                _ = cancelled(cancel) => return Err(DriverError::Cancelled),
                _ = tokio::time::sleep(std::time::Duration::from_millis(150)) => continue,
            }
        }
        let Some(models) = readiness_json(client, format!("{base}/v1/models"), cancel).await?
        else {
            tokio::select! {
                biased;
                _ = cancelled(cancel) => return Err(DriverError::Cancelled),
                _ = tokio::time::sleep(std::time::Duration::from_millis(150)) => continue,
            }
        };
        let data = models
            .pointer("/data")
            .and_then(Value::as_array)
            .ok_or(DriverError::Ollama)?;
        if data.is_empty() {
            tokio::select! {
                biased;
                _ = cancelled(cancel) => return Err(DriverError::Cancelled),
                _ = tokio::time::sleep(std::time::Duration::from_millis(150)) => continue,
            }
        }
        if !data
            .iter()
            .any(|model| model.get("id").and_then(Value::as_str) == Some(expected_alias))
        {
            return Err(DriverError::Ollama);
        }
        if let Some(supervisor) = supervisor
            && !supervisor.is_running(profile).await?
        {
            return Err(DriverError::Ollama);
        }
        return Ok(());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::models::{
        identity::Caller,
        model_profile::ModelProfile,
        usage::{CallUsage, LedgerUnavailable, Share, WaitingCall},
    };
    use chrono::{DateTime, Utc};
    use std::sync::Arc;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::{TcpListener, TcpStream},
    };

    struct EmptyLedger;
    impl crate::domain::models::usage::UsageLedger for EmptyLedger {
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

    struct FakeProcessControl {
        stops: std::sync::atomic::AtomicUsize,
    }
    impl PrismProcessControl for FakeProcessControl {
        fn ensure<'a>(
            &'a self,
            _: &'a Config,
            _: ModelProfile,
            _: &'a mut watch::Receiver<bool>,
        ) -> Pending<'a, Result<(), DriverError>> {
            Box::pin(async { Ok(()) })
        }
        fn stop<'a>(&'a self) -> Pending<'a, Result<(), DriverError>> {
            Box::pin(async move {
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                self.stops.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(())
            })
        }
        fn is_running<'a>(&'a self, _: ModelProfile) -> Pending<'a, Result<bool, DriverError>> {
            Box::pin(async { Ok(true) })
        }
    }

    async fn request(stream: &mut TcpStream) -> (String, Vec<u8>) {
        let mut bytes = Vec::new();
        let mut buf = [0; 2048];
        let header_end = loop {
            let n = stream.read(&mut buf).await.unwrap();
            assert!(n > 0);
            bytes.extend_from_slice(&buf[..n]);
            if let Some(pos) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                break pos + 4;
            }
        };
        let headers = String::from_utf8_lossy(&bytes[..header_end]).to_string();
        let content_length = headers
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().unwrap())
            })
            .unwrap_or(0);
        while bytes.len() < header_end + content_length {
            let n = stream.read(&mut buf).await.unwrap();
            assert!(n > 0);
            bytes.extend_from_slice(&buf[..n]);
        }
        (
            headers,
            bytes[header_end..header_end + content_length].to_vec(),
        )
    }

    async fn reply(stream: &mut TcpStream, status: &str, content_type: &str, body: &[u8]) {
        let head = format!(
            "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        stream.write_all(head.as_bytes()).await.unwrap();
        stream.write_all(body).await.unwrap();
        stream.shutdown().await.unwrap();
    }

    fn input() -> CallInput {
        CallInput {
            role: "answer".into(),
            system: "system".into(),
            prompt: "prompt".into(),
            images: vec![],
            temperature: 0.0,
            max_tokens: 128,
            context_tokens: 8192,
            prefix: None,
            allow_tools: false,
            tool_history: vec![],
        }
    }

    fn test_output() -> OutputModel {
        OutputModel::for_profile(
            &Caller {
                app_id: "test".into(),
                user_id: Some("user".into()),
                is_admin: false,
            },
            uuid::Uuid::now_v7(),
            ModelProfile::Bonsai25Official,
            false,
        )
    }

    async fn driver(base: String) -> (Prism, Arc<EmptyLedger>) {
        let ledger = Arc::new(EmptyLedger);
        let queue = Scheduler::new(ledger.clone());
        let mut config = Config::default();
        config.prism_url = base;
        config.bonsai_enabled = true;
        (Prism::new(config, queue), ledger)
    }

    #[tokio::test]
    async fn waits_for_usage_after_finish_reason_and_reassembles_sse_frames() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let (prism, _ledger) = driver(base).await;
        let (tx, rx) = tokio::sync::oneshot::channel();
        let mut tx = Some(tx);
        let server = tokio::spawn(async move {
            for attempt in 0..3 {
                for request_index in 0..3 {
                    let (mut stream, _) = listener.accept().await.unwrap();
                    let (headers, request_body) = request(&mut stream).await;
                    let path = headers.lines().next().unwrap().to_string();
                    if request_index == 0 {
                        assert!(path.contains("/props"));
                        reply(&mut stream, "200 OK", "application/json", br#"{"model_path":"/weights/bonsai25-official-PTQ1_0-mtp.gguf","default_generation_settings":{"n_ctx":8192}}"#).await;
                    } else if request_index == 1 {
                        assert!(path.contains("/v1/models"));
                        reply(
                            &mut stream,
                            "200 OK",
                            "application/json",
                            br#"{"data":[{"id":"bonsai25-official"}]}"#,
                        )
                        .await;
                    } else {
                        let body: Value = serde_json::from_slice(&request_body).unwrap();
                        let assertions = body["chat_template_kwargs"]["enable_thinking"] == false
                            && body["logprobs"] == true
                            && body["continue_final_message"] == (attempt == 0)
                            && body["add_generation_prompt"] == (attempt != 0)
                            && body["messages"][0]["role"] == "system"
                            && if attempt == 0 {
                                body["messages"][2]["role"] == "assistant"
                                    && body["messages"][2]["content"] == "saved prefix"
                            } else {
                                true
                            };
                        if let Some(tx) = tx.take() {
                            let _ = tx.send(assertions);
                        }
                        let logprobs = if attempt == 1 {
                            json!({"content":"malformed"})
                        } else {
                            json!({"content":[{"token":if attempt==0{"ok"}else{"x"},"logprob":-0.01,"top_logprobs":[{"token":if attempt==0{"ok"}else{"x"},"logprob":-0.01}]}]})
                        };
                        let content = if attempt == 0 { "ok" } else { "x" };
                        let frames = [
                            format!("data: {}\n\n", json!({"choices":[{"delta":{"content":content},"logprobs":logprobs,"finish_reason":null}]})),
                            format!("data: {}\n\n", json!({"choices":[{"delta":{},"finish_reason":"stop"}]})),
                            "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":13,\"completion_tokens\":2}}\n\n".to_owned(),
                        ];
                        let head = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n";
                        stream.write_all(head.as_bytes()).await.unwrap();
                        for frame in frames {
                            stream.write_all(frame.as_bytes()).await.unwrap();
                            stream.flush().await.unwrap();
                        }
                        if attempt != 2 {
                            stream.write_all(b"data: [DONE]\n\n").await.unwrap();
                        }
                        stream.shutdown().await.unwrap();
                    }
                }
            }
        });
        let output = test_output();
        let (events, _rx) = mpsc::unbounded_channel();
        let (_cancel_tx, cancel) = watch::channel(false);
        let mut prefixed_input = input();
        prefixed_input.prefix = Some("saved prefix".into());
        let outcome = prism
            .call(&output, prefixed_input, events, cancel)
            .await
            .unwrap();
        assert!(rx.await.unwrap());
        assert_eq!(outcome.text, "ok");
        assert_eq!(outcome.usage.input_tokens, 13);
        assert_eq!(outcome.usage.output_tokens, 2);
        assert!(outcome.usage_measured);
        let inspection = outcome.inspection.unwrap();
        assert!(inspection.matches("ok"));
        assert_eq!(inspection.tokens.len(), 1);
        assert_eq!(inspection.tokens[0].logprob, -0.01);
        let output = test_output();
        let (events, _rx) = mpsc::unbounded_channel();
        let (_cancel_tx, cancel) = watch::channel(false);
        let malformed = prism.call(&output, input(), events, cancel).await.unwrap();
        assert_eq!(
            malformed.inspection.unwrap().reason.as_deref(),
            Some("invalid_data")
        );
        let output = test_output();
        let (events, _rx) = mpsc::unbounded_channel();
        let (_cancel_tx, cancel) = watch::channel(false);
        let truncated = prism.call(&output, input(), events, cancel).await.unwrap();
        assert_eq!(truncated.text, "x");
        assert_eq!(truncated.error, Some(DriverError::Ollama));
        assert_eq!(truncated.usage.input_tokens, 13);
        assert_eq!(truncated.usage.output_tokens, 2);
        assert!(truncated.usage_measured);
        assert!(truncated.tool_request.is_none());
        let partial_inspection = truncated.inspection.unwrap();
        assert_eq!(partial_inspection.tokens.len(), 1);
        assert_eq!(partial_inspection.tokens[0].bytes, b"x");
        server.await.unwrap();
    }

    #[tokio::test]
    async fn readiness_retries_delayed_bind_and_loading_503() {
        let reserved = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = reserved.local_addr().unwrap();
        drop(reserved);
        let base = format!("http://{address}");
        let (prism, _ledger) = driver(base.clone()).await;
        let server = tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(80)).await;
            let listener = TcpListener::bind(address).await.unwrap();
            let (mut first, _) = listener.accept().await.unwrap();
            let (headers, _) = request(&mut first).await;
            assert!(headers.lines().next().unwrap().contains("/props"));
            reply(
                &mut first,
                "503 Service Unavailable",
                "application/json",
                b"{}",
            )
            .await;

            let (mut props, _) = listener.accept().await.unwrap();
            let (headers, _) = request(&mut props).await;
            assert!(headers.lines().next().unwrap().contains("/props"));
            reply(&mut props, "200 OK", "application/json", br#"{"model_path":"/weights/bonsai25-official-PTQ1_0-mtp.gguf","default_generation_settings":{"n_ctx":8192}}"#).await;

            let (mut models, _) = listener.accept().await.unwrap();
            let (headers, _) = request(&mut models).await;
            assert!(headers.lines().next().unwrap().contains("/v1/models"));
            reply(
                &mut models,
                "200 OK",
                "application/json",
                br#"{"data":[{"id":"bonsai25-official"}]}"#,
            )
            .await;
        });
        let (_cancel_tx, mut cancel) = watch::channel(false);
        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            wait_for_ready(
                &prism.client,
                &base,
                None,
                ModelProfile::Bonsai25Official,
                "bonsai25-official",
                "bonsai25-official-PTQ1_0-mtp.gguf",
                8192,
                &mut cancel,
            ),
        )
        .await
        .expect("bounded readiness")
        .unwrap();
        server.await.unwrap();
    }

    #[tokio::test]
    async fn cancellation_holds_scheduler_until_owned_runtime_stop_finishes() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let ledger = Arc::new(EmptyLedger);
        let queue = Scheduler::new(ledger);
        let mut config = Config::default();
        config.prism_url = base;
        config.bonsai_enabled = true;
        let process = Arc::new(FakeProcessControl {
            stops: std::sync::atomic::AtomicUsize::new(0),
        });
        let prism = Arc::new(Prism::managed(config, queue.clone(), process.clone()));
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let mut started_tx = Some(started_tx);
        let server = tokio::spawn(async move {
            for index in 0..3 {
                let (mut stream, _) = listener.accept().await.unwrap();
                let (headers, _) = request(&mut stream).await;
                let path = headers.lines().next().unwrap().to_string();
                if index == 0 {
                    assert!(path.contains("/props"));
                    reply(&mut stream, "200 OK", "application/json", br#"{"model_path":"/weights/bonsai25-official-PTQ1_0-mtp.gguf","default_generation_settings":{"n_ctx":8192}}"#).await;
                } else if index == 1 {
                    assert!(path.contains("/v1/models"));
                    reply(
                        &mut stream,
                        "200 OK",
                        "application/json",
                        br#"{"data":[{"id":"bonsai25-official"}]}"#,
                    )
                    .await;
                } else {
                    assert!(path.contains("/v1/chat/completions"));
                    if let Some(started_tx) = started_tx.take() {
                        let _ = started_tx.send(());
                    }
                    stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\ndata: {\"choices\":[{\"delta\":{\"content\":\"partial\"},\"logprobs\":{\"content\":[{\"token\":\"partial\",\"logprob\":-0.1,\"top_logprobs\":[]}]},\"finish_reason\":null}]}\n\n").await.unwrap();
                    stream.flush().await.unwrap();
                    let mut buffer = [0; 16];
                    let _ = tokio::time::timeout(
                        std::time::Duration::from_secs(2),
                        stream.read(&mut buffer),
                    )
                    .await;
                }
            }
        });
        let output = test_output();
        let permit = queue
            .acquire(output.actor.clone(), output.output_id)
            .await
            .unwrap();
        let (events, mut event_rx) = mpsc::unbounded_channel();
        let (cancel_tx, cancel) = watch::channel(false);
        let call = {
            let prism = prism.clone();
            tokio::spawn(async move {
                prism
                    .call_with_permit(&output, input(), events, cancel, permit)
                    .await
            })
        };
        started_rx.await.unwrap();
        assert!(tokio::time::timeout(std::time::Duration::from_secs(1), async {
            loop {
                if matches!(event_rx.recv().await, Some(DriverEvent::Token { .. })) {
                    break;
                }
            }
        })
        .await
        .is_ok());
        let next_queue = queue.clone();
        let mut next = tokio::spawn(async move {
            next_queue
                .acquire("next-job".into(), uuid::Uuid::now_v7())
                .await
        });
        cancel_tx.send(true).unwrap();
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(50), &mut next)
                .await
                .is_err()
        );
        let partial = tokio::time::timeout(std::time::Duration::from_secs(2), call)
            .await
            .unwrap()
            .unwrap()
            .expect("partial output is preserved on cancellation");
        assert_eq!(partial.text, "partial");
        assert_eq!(partial.error, Some(DriverError::Cancelled));
        assert!(!partial.usage_measured);
        assert!(partial.tool_request.is_none());
        assert_eq!(partial.inspection.unwrap().tokens.len(), 1);
        assert_eq!(process.stops.load(std::sync::atomic::Ordering::SeqCst), 1);
        let next_permit = tokio::time::timeout(std::time::Duration::from_secs(1), &mut next)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        drop(next_permit);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn refuses_redirects_from_the_loopback_endpoint() {
        let target = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let target_addr = target.local_addr().unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let _ = request(&mut stream).await;
            let response = format!(
                "HTTP/1.1 302 Found\r\nLocation: http://{target_addr}/props\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            );
            stream.write_all(response.as_bytes()).await.unwrap();
        });
        let (prism, _ledger) = driver(base).await;
        let output = test_output();
        let (events, _rx) = mpsc::unbounded_channel();
        let (_cancel_tx, cancel) = watch::channel(false);
        assert!(matches!(
            prism.call(&output, input(), events, cancel).await,
            Err(DriverError::Ollama)
        ));
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(50), target.accept())
                .await
                .is_err()
        );
        server.await.unwrap();
    }

    #[tokio::test]
    async fn cancellation_interrupts_a_stalled_readiness_body() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let _ = request(&mut stream).await;
            stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 100\r\nConnection: close\r\n\r\n{}").await.unwrap();
            stream.flush().await.unwrap();
            let mut buffer = [0; 8];
            let _ =
                tokio::time::timeout(std::time::Duration::from_secs(2), stream.read(&mut buffer))
                    .await;
        });
        let (prism, _ledger) = driver(base).await;
        let prism = Arc::new(prism);
        let output = test_output();
        let (events, _rx) = mpsc::unbounded_channel();
        let (cancel_tx, cancel) = watch::channel(false);
        let task = {
            let prism = prism.clone();
            tokio::spawn(async move { prism.call(&output, input(), events, cancel).await })
        };
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        cancel_tx.send(true).unwrap();
        let result = tokio::time::timeout(std::time::Duration::from_secs(1), task)
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(result, Err(DriverError::Cancelled)));
        server.await.unwrap();
    }
}

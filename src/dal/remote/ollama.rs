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
}
#[derive(Default, Deserialize)]
struct Frame {
    message: Option<Message>,
    #[serde(default)]
    done: bool,
    error: Option<String>,
    prompt_eval_count: Option<u64>,
    eval_count: Option<u64>,
    eval_duration: Option<u64>,
}
#[derive(Clone)]
pub struct Ollama {
    client: reqwest::Client,
    config: Config,
    queue: Scheduler,
}
struct Collected {
    text: String,
    prompt: Option<u64>,
    tokens: Option<u64>,
    eval_ns: Option<u64>,
    first_ms: Option<u64>,
    sent: bool,
}
impl Ollama {
    pub fn new(config: Config, queue: Scheduler) -> Self {
        Self {
            client: reqwest::Client::builder()
                .connect_timeout(config.ollama_connect_timeout)
                .build()
                .expect("Ollama client"),
            config,
            queue,
        }
    }
    async fn post(&self, path: &str, body: &Value) -> Result<reqwest::Response, DriverError> {
        let response = self
            .client
            .post(format!(
                "{}{path}",
                self.config.ollama_url.trim_end_matches('/')
            ))
            .json(body)
            .send()
            .await
            .map_err(|_| DriverError::Ollama)?;
        if !response.status().is_success() {
            return Err(DriverError::Ollama);
        }
        Ok(response)
    }
    async fn prepare(
        &self,
        _permit: &Permit,
        model: &str,
        events: &mpsc::UnboundedSender<DriverEvent>,
    ) -> Result<u64, DriverError> {
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
            && loaded.models[0].context_length == Some(self.config.model_context_tokens)
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
        let response=self.post("/api/generate",&json!({"model":model,"keep_alive":self.config.ollama_keep_alive,"stream":false,"options":{"num_ctx":self.config.model_context_tokens}})).await?;
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
        let models: RunningModels = response.json().await.map_err(|_| DriverError::Ollama)?;
        for model in models.models {
            self.post(
                "/api/generate",
                &json!({"model":model.name,"keep_alive":0,"stream":false}),
            )
            .await?;
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
    ) -> Result<bool, DriverError> {
        let frame: Frame = serde_json::from_slice(line).map_err(|_| DriverError::Ollama)?;
        if frame.error.is_some() {
            return Err(DriverError::Ollama);
        }
        if let Some(message) = frame.message
            && !message.content.is_empty()
        {
            if collected.first_ms.is_none() {
                collected.first_ms = Some(start.elapsed().as_millis() as u64);
            }
            collected.text.push_str(&message.content);
            let _ = events.send(DriverEvent::Token {
                role: role.into(),
                text: message.content,
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
        model: &str,
        input: &CallInput,
        events: &mpsc::UnboundedSender<DriverEvent>,
        collected: &mut Collected,
        start: Instant,
    ) -> Result<(), DriverError> {
        let mut messages = vec![];
        if !input.system.is_empty() {
            messages.push(json!({"role":"system","content":input.system}));
        }
        messages.push(json!({"role":"user","content":input.prompt,"images":input.images}));
        collected.sent = true;
        let response=self.post("/api/chat",&json!({"model":model,"messages":messages,"stream":true,"keep_alive":self.config.ollama_keep_alive,"options":{"num_ctx":self.config.model_context_tokens,"temperature":input.temperature,"num_predict":input.max_tokens}})).await?;
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
                if self.frame(&line, collected, &input.role, events, start)? {
                    return Ok(());
                }
            }
        }
        if !buffer.iter().all(u8::is_ascii_whitespace)
            && self.frame(&buffer, collected, &input.role, events, start)?
        {
            return Ok(());
        }
        Err(DriverError::Ollama)
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
            {
                return Err(DriverError::InvalidInput);
            }
            let _ = events.send(DriverEvent::Waiting);
            let permit = tokio::select! {biased;_=cancelled(&mut cancel)=>return Err(DriverError::Cancelled),permit=self.queue.acquire(output.actor.clone(),output.output_id)=>permit.map_err(DriverError::Queue)?};
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
            };
            let result = tokio::select! {biased;_=cancelled(&mut cancel)=>Err(DriverError::Cancelled),result=async{load_ms=self.prepare(&permit,model,&events).await?;self.generate(model,&input,&events,&mut collected,start).await}=>result};
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
                role: input.role,
                model: model.into(),
                input_tokens,
                output_tokens,
                time_to_first_token_ms: collected.first_ms,
                total_ms: start.elapsed().as_millis() as u64,
                started_at,
                finished_at,
            };
            let _ = permit
                .finish(
                    Usage {
                        input: input_tokens,
                        output: output_tokens,
                        images: u64::from(images),
                        audio: 0,
                    },
                    finished_at,
                )
                .await;
            let tokens_per_second = collected
                .eval_ns
                .filter(|ns| *ns > 0)
                .map(|ns| output_tokens as f64 * 1_000_000_000.0 / ns as f64);
            Ok(CallOutcome {
                text: collected.text,
                usage,
                error: result.err(),
                load_ms,
                tokens_per_second,
            })
        })
    }
}

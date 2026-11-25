use crate::{
    core::{config::Config, process_numbers},
    domain::{
        models::{
            context::ContextText,
            driver::{CallInput, CallOutcome, DriverError, ModelDriver},
            events::{CallStreamRole, NativeEvent, StepName, StepProgress, ThinkingRole},
            pipeline::StageName,
            thinking::*,
            tools::{PendingTool, ToolControl, ToolExchange, ToolResult},
        },
        services::{context::encode, crawler::cancelled, events::EventSink},
    },
};
use std::sync::Arc;
use tokio::{
    sync::{mpsc, watch},
    time::{Duration, Instant},
};

#[derive(Clone)]
pub struct Thinking {
    driver: Arc<dyn ModelDriver>,
    config: Config,
}
impl Thinking {
    pub fn new(config: &Config, driver: Arc<dyn ModelDriver>) -> Self {
        Self {
            driver,
            config: config.clone(),
        }
    }
    /// Keep every earlier role label, trimming note text to the fixed reserve.
    fn call_input(
        &self,
        input: &ThinkingInput<'_>,
        notes: &[ContextText],
        role: &str,
        instruction: &str,
        max_tokens: u32,
    ) -> Result<CallInput, DriverError> {
        let system = format!("{}\n{instruction}", input.context.system);
        let render = |cap: usize| {
            let notes: Vec<_> = notes
                .iter()
                .map(|n| ContextText {
                    label: n.label.clone(),
                    source: None,
                    text: n.text.chars().take(cap).collect(),
                })
                .collect();
            let text = if notes.is_empty() {
                String::new()
            } else {
                format!("\nNotes:\n{}", encode(self.config.context_format, &notes))
            };
            let question = format!("\nQuestion: {}", input.context.question);
            let prefix = input
                .context
                .prompt
                .strip_suffix(&question)
                .unwrap_or(&input.context.prompt);
            let prompt = format!("{prefix}{text}{question}");
            let fits = self.config.estimate_tokens(&text)
                <= self.config.thinking_notes_token_reserve
                && self
                    .config
                    .estimate_tokens(&system)
                    .saturating_add(self.config.estimate_tokens(&prompt))
                    .saturating_add(self.config.chat_template_token_reserve)
                    .saturating_add(u64::from(max_tokens))
                    <= self.config.model_context_tokens;
            (prompt, fits)
        };
        if !render(0).1 {
            return Err(DriverError::InvalidInput);
        }
        let (mut low, mut high) = (
            0,
            notes
                .iter()
                .map(|n| n.text.chars().count())
                .max()
                .unwrap_or(0)
                + 1,
        );
        while low + 1 < high {
            let mid = low + (high - low) / 2;
            if render(mid).1 {
                low = mid;
            } else {
                high = mid;
            }
        }
        let prompt = render(low).0;
        let remaining = input
            .budget
            .as_ref()
            .map_or(u64::MAX, |budget| budget.remaining());
        let prompt_tokens = self
            .config
            .estimate_tokens(&system)
            .saturating_add(self.config.estimate_tokens(&prompt))
            .saturating_add(self.config.chat_template_token_reserve);
        let output_allowance = remaining.saturating_sub(prompt_tokens).min(
            self.config
                .model_context_tokens
                .saturating_sub(prompt_tokens),
        );
        if output_allowance == 0 {
            return Err(DriverError::InvalidInput);
        }
        Ok(CallInput {
            role: role.into(),
            system,
            prompt,
            images: input.images.clone(),
            temperature: self.config.query_temperature,
            max_tokens: u64::from(max_tokens)
                .min(output_allowance)
                .min(u64::from(u32::MAX)) as u32,
            context_tokens: self
                .config
                .model_context_tokens
                .min(remaining)
                .min(u64::from(u32::MAX)) as u32,
            prefix: None,
            allow_tools: role == "answer" && input.tools_enabled,
            tool_history: vec![],
        })
    }
    async fn call(
        &self,
        input: &ThinkingInput<'_>,
        call: CallInput,
        role: CallStreamRole,
        events: &EventSink,
        signal: watch::Receiver<bool>,
    ) -> Result<CallOutcome, DriverError> {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let sink = events.clone();
        let stage = if matches!(role, CallStreamRole::Answer) {
            StageName::Answer
        } else {
            StageName::Think
        };
        let forward = tokio::spawn(async move {
            while let Some(event) = rx.recv().await {
                sink.driver(stage, role.clone(), event);
            }
        });
        let outcome = self
            .driver
            .call(input.model, call, tx, signal)
            .await
            .and_then(|outcome| {
                if let Some(budget) = &input.budget {
                    let actual = outcome
                        .usage
                        .input_tokens
                        .saturating_add(outcome.usage.output_tokens);
                    if !outcome.usage_measured || !budget.charge(actual) {
                        return Err(DriverError::InvalidInput);
                    }
                }
                Ok(outcome)
            });
        let _ = forward.await;
        outcome
    }
    pub async fn think(
        &self,
        input: &ThinkingInput<'_>,
        events: &EventSink,
        stop: watch::Receiver<bool>,
        cancel: watch::Receiver<bool>,
    ) -> ThinkingResult {
        let mut result = ThinkingResult::default();
        let deadline = Instant::now() + self.config.thinking_timeout;
        for (index, (role, kind, round, instruction)) in [
            (
                "plan",
                ThinkingRole::Plan,
                1,
                &self.config.thinking_plan_instruction,
            ),
            (
                "work",
                ThinkingRole::Work,
                1,
                &self.config.thinking_work_instruction,
            ),
            (
                "check",
                ThinkingRole::Check,
                1,
                &self.config.thinking_check_instruction,
            ),
            (
                "work",
                ThinkingRole::Work,
                2,
                &self.config.thinking_work_instruction,
            ),
            (
                "check",
                ThinkingRole::Check,
                2,
                &self.config.thinking_check_instruction,
            ),
        ]
        .into_iter()
        .enumerate()
        {
            if *cancel.borrow() {
                result.error = Some(DriverError::Cancelled);
                break;
            }
            if *stop.borrow() {
                events.send(NativeEvent::Step(StepProgress::new(
                    StageName::Think,
                    StepName::ThinkingStopped,
                )));
                break;
            }
            if Instant::now() >= deadline {
                result.notice = Some(process_numbers::THINKING_TIME_LIMIT);
                break;
            }
            let call = match self.call_input(
                input,
                &result.notes,
                role,
                instruction,
                self.config.thinking_output_tokens,
            ) {
                Ok(call) => call,
                Err(error) => {
                    result.error = Some(error);
                    break;
                }
            };
            let mut step = StepProgress::new(StageName::Think, StepName::ThinkingStep);
            step.params.insert("role".into(), role.into());
            step.params.insert("round".into(), round.into());
            step.progress = Some([index as u64 + 1, if round == 1 { 4 } else { 6 }]);
            events.send(NativeEvent::Step(step));
            let (control, signal) = watch::channel(false);
            let call = self.call(
                input,
                call,
                CallStreamRole::Thinking { role: kind, round },
                events,
                signal,
            );
            tokio::pin!(call);
            let (outcome, reason) = tokio::select! {biased;
                _=cancelled(cancel.clone())=>{let _=control.send(true);(call.await,3)},
                _=cancelled(stop.clone())=>{let _=control.send(true);(call.await,2)},
                _=tokio::time::sleep_until(deadline)=>{let _=control.send(true);(call.await,1)},
                outcome=&mut call=>(outcome,0),
            };
            let mut complete = None;
            match outcome {
                Ok(outcome) => {
                    result.calls.push(outcome.usage);
                    result
                        .text
                        .push_str(&format!("[{role} round {round}]\n{}\n", outcome.text));
                    let note = if role == "plan" {
                        outcome
                            .text
                            .lines()
                            .filter(|l| !l.trim().is_empty())
                            .take(self.config.thinking_plan_steps)
                            .collect::<Vec<_>>()
                            .join("\n")
                    } else {
                        outcome.text
                    };
                    let none = role == "check" && note.trim() == "NONE";
                    result.notes.push(ContextText {
                        label: format!("{role} round {round}"),
                        source: None,
                        text: note,
                    });
                    complete = Some(none);
                    if reason == 0 {
                        result.error = outcome.error;
                    }
                }
                Err(error) if reason == 0 => result.error = Some(error),
                Err(_) => {}
            }
            match reason {
                1 => result.notice = Some(process_numbers::THINKING_TIME_LIMIT),
                2 => events.send(NativeEvent::Step(StepProgress::new(
                    StageName::Think,
                    StepName::ThinkingStopped,
                ))),
                3 => result.error = Some(DriverError::Cancelled),
                _ => {}
            }
            if reason != 0 || result.error.is_some() || complete == Some(true) {
                break;
            }
        }
        result
    }
    pub async fn answer(
        &self,
        input: &ThinkingInput<'_>,
        thinking: &ThinkingResult,
        events: &EventSink,
        cancel: watch::Receiver<bool>,
    ) -> Result<CallOutcome, DriverError> {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let control = ToolControl::new(tx);
        self.answer_with_tools(input, thinking, events, cancel, &control, &mut rx)
            .await
    }

    pub async fn answer_with_tools(
        &self,
        input: &ThinkingInput<'_>,
        thinking: &ThinkingResult,
        events: &EventSink,
        cancel: watch::Receiver<bool>,
        tool_control: &ToolControl,
        tool_results: &mut mpsc::UnboundedReceiver<ToolResult>,
    ) -> Result<CallOutcome, DriverError> {
        if let Some(error) = thinking.error {
            return Err(error);
        }
        if *cancel.borrow() {
            return Err(DriverError::Cancelled);
        }
        let mut call = self.call_input(
            input,
            &thinking.notes,
            "answer",
            &self.config.thinking_final_instruction,
            self.config.answer_token_reserve as u32,
        )?;
        call.prefix = input.prefix.clone();
        call.allow_tools = input.tools_enabled && input.prefix.is_none();
        if !thinking.calls.is_empty() {
            let mut step = StepProgress::new(StageName::Answer, StepName::ThinkingStep);
            step.params.insert("role".into(), "answer".into());
            step.progress = Some([
                thinking.calls.len() as u64 + 1,
                thinking.calls.len() as u64 + 1,
            ]);
            events.send(NativeEvent::Step(step));
        }
        let mut exchanges = Vec::new();
        let mut accumulated_text = String::new();
        let mut accumulated_usage: Option<crate::domain::models::run_record::ModelCall> = None;
        let mut had_tool = false;
        for _ in 0..8 {
            call.tool_history = exchanges.clone();
            let outcome = self
                .call(
                    input,
                    call.clone(),
                    CallStreamRole::Answer,
                    events,
                    cancel.clone(),
                )
                .await?;
            if let Some(request) = outcome.tool_request {
                accumulated_text.push_str(&outcome.text);
                if !input.tools_enabled {
                    return Err(DriverError::InvalidInput);
                }
                had_tool = true;
                accumulated_usage = Some(match accumulated_usage.take() {
                    Some(mut prior) => {
                        prior.input_tokens = prior
                            .input_tokens
                            .saturating_add(outcome.usage.input_tokens);
                        prior.output_tokens = prior
                            .output_tokens
                            .saturating_add(outcome.usage.output_tokens);
                        prior.total_ms = prior.total_ms.saturating_add(outcome.usage.total_ms);
                        prior.finished_at = outcome.usage.finished_at;
                        prior
                    }
                    None => outcome.usage,
                });
                if !tool_control.set_pending(PendingTool {
                    call_id: request.call_id,
                    name: request.name,
                    arguments: request.arguments.clone(),
                }) {
                    return Err(DriverError::InvalidInput);
                }
                events.send(NativeEvent::ToolCall(request.clone()));
                let received = tokio::select! {
                    _ = cancelled(cancel.clone()) => None,
                    result = tokio::time::timeout(Duration::from_secs(120), tool_results.recv()) => result.ok().flatten(),
                };
                tool_control.clear_pending(request.call_id);
                let Some(result) = received else {
                    return Err(if *cancel.borrow() {
                        DriverError::Cancelled
                    } else {
                        DriverError::InvalidInput
                    });
                };
                if result.call_id != request.call_id || result.name != request.name {
                    return Err(DriverError::InvalidInput);
                }
                exchanges.push(ToolExchange {
                    request,
                    output: result.output,
                });
                continue;
            }
            let mut outcome = outcome;
            outcome.text = format!("{accumulated_text}{}", outcome.text);
            if let Some(mut prior) = accumulated_usage {
                prior.input_tokens = prior
                    .input_tokens
                    .saturating_add(outcome.usage.input_tokens);
                prior.output_tokens = prior
                    .output_tokens
                    .saturating_add(outcome.usage.output_tokens);
                prior.total_ms = prior.total_ms.saturating_add(outcome.usage.total_ms);
                prior.finished_at = outcome.usage.finished_at;
                outcome.usage = prior;
            }
            if had_tool {
                outcome.inspection = None;
            }
            return Ok(outcome);
        }
        Err(DriverError::InvalidInput)
    }
}

#[cfg(test)]
mod child_budget_tests {
    use super::*;
    use crate::{
        core::config::Config,
        domain::models::{
            context::AssembledContext,
            driver::{DriverEvent, OutputModel},
            identity::Caller,
            run_record::ModelCall,
        },
    };
    use chrono::Utc;
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicBool, Ordering};

    struct FakeDriver {
        outcomes: std::sync::Mutex<VecDeque<CallOutcome>>,
        calls: std::sync::Mutex<Vec<CallInput>>,
    }
    impl ModelDriver for FakeDriver {
        fn call<'a>(
            &'a self,
            _output: &'a OutputModel,
            input: CallInput,
            _events: mpsc::UnboundedSender<DriverEvent>,
            _cancel: watch::Receiver<bool>,
        ) -> crate::domain::models::usage::Pending<'a, Result<CallOutcome, DriverError>> {
            Box::pin(async move {
                self.calls.lock().unwrap().push(input);
                self.outcomes
                    .lock()
                    .unwrap()
                    .pop_front()
                    .ok_or(DriverError::InvalidInput)
            })
        }
    }

    struct PausedDriver {
        stopped: Arc<AtomicBool>,
    }
    impl ModelDriver for PausedDriver {
        fn call<'a>(
            &'a self,
            _output: &'a OutputModel,
            _input: CallInput,
            _events: mpsc::UnboundedSender<DriverEvent>,
            mut cancel: watch::Receiver<bool>,
        ) -> crate::domain::models::usage::Pending<'a, Result<CallOutcome, DriverError>> {
            Box::pin(async move {
                if !*cancel.borrow() {
                    let _ = cancel.changed().await;
                }
                self.stopped.store(true, Ordering::Release);
                Err(DriverError::Cancelled)
            })
        }
    }

    fn outcome(measured: bool, input_tokens: u64, output_tokens: u64) -> CallOutcome {
        let now = Utc::now();
        CallOutcome {
            inspection: None,
            text: "bounded reply".into(),
            usage: ModelCall {
                images: Some(0),
                audio: None,
                role: "answer".into(),
                model: "fake".into(),
                input_tokens,
                output_tokens,
                time_to_first_token_ms: None,
                total_ms: 1,
                started_at: now,
                finished_at: now,
            },
            usage_measured: measured,
            error: None,
            load_ms: 0,
            tokens_per_second: None,
            tool_request: None,
        }
    }

    fn make_thinking_input<'a>(
        model: &'a OutputModel,
        context: &'a AssembledContext,
        budget: Arc<TokenBudget>,
    ) -> ThinkingInput<'a> {
        ThinkingInput {
            model,
            context,
            images: vec![],
            prefix: None,
            tools_enabled: false,
            budget: Some(budget),
        }
    }

    #[tokio::test]
    async fn provider_calls_clamp_context_and_charge_measured_usage_only() {
        let config = Config::default();
        let caller = Caller {
            app_id: "cortex".into(),
            user_id: Some("owner".into()),
            is_admin: false,
        };
        let model = OutputModel::new(&caller, uuid::Uuid::now_v7(), false);
        let context = AssembledContext {
            question: "Explain briefly".into(),
            system: "Answer with evidence.".into(),
            prompt: "Question: Explain briefly".into(),
            system_personas: vec![],
            texts: vec![],
            history: vec![],
            estimated_input_tokens: 0,
            reserved_output_tokens: 0,
            reserved_notes_tokens: 0,
        };
        let budget = Arc::new(TokenBudget::new(512));
        let input = make_thinking_input(&model, &context, budget.clone());
        let fake = Arc::new(FakeDriver {
            outcomes: std::sync::Mutex::new(VecDeque::from([outcome(true, 180, 80)])),
            calls: std::sync::Mutex::new(Vec::new()),
        });
        let thinking = Thinking::new(&config, fake.clone());
        let call_input = thinking
            .call_input(&input, &[], "answer", "Short answer", 1024)
            .unwrap();
        let estimated_prompt = config.estimate_tokens(&call_input.system)
            + config.estimate_tokens(&call_input.prompt)
            + config.chat_template_token_reserve;
        assert!(u64::from(call_input.context_tokens) <= budget.remaining());
        assert!(estimated_prompt + u64::from(call_input.max_tokens) <= budget.remaining());
        let (events, _receiver, forwarder) =
            crate::domain::services::events::channel(&config, None);
        let (_cancel_tx, cancel) = watch::channel(false);
        thinking
            .call(&input, call_input, CallStreamRole::Answer, &events, cancel)
            .await
            .unwrap();
        drop(events);
        forwarder.await.unwrap();
        assert_eq!(budget.used(), 260);
        let remaining_input = thinking
            .call_input(&input, &[], "answer", "Next call", 1024)
            .unwrap();
        let remaining_prompt = config.estimate_tokens(&remaining_input.system)
            + config.estimate_tokens(&remaining_input.prompt)
            + config.chat_template_token_reserve;
        assert!(u64::from(remaining_input.context_tokens) <= 252);
        assert!(remaining_prompt + u64::from(remaining_input.max_tokens) <= 252);
        let nested_id = uuid::Uuid::now_v7();
        assert!(budget.reserve(nested_id, 100));
        assert_eq!(budget.remaining(), 152);
        assert!(budget.settle_reservation(nested_id, 70));
        assert_eq!(budget.used(), 330);
        assert_eq!(budget.remaining(), 182);
        assert!(
            thinking
                .call_input(
                    &make_thinking_input(&model, &context, Arc::new(TokenBudget::new(1))),
                    &[],
                    "answer",
                    "No room",
                    32
                )
                .is_err()
        );

        let unmeasured_budget = Arc::new(TokenBudget::new(512));
        let unmeasured_input = make_thinking_input(&model, &context, unmeasured_budget.clone());
        let unmeasured_fake = Arc::new(FakeDriver {
            outcomes: std::sync::Mutex::new(VecDeque::from([outcome(false, 12, 8)])),
            calls: std::sync::Mutex::new(Vec::new()),
        });
        let unmeasured = Thinking::new(&config, unmeasured_fake);
        let request = unmeasured
            .call_input(&unmeasured_input, &[], "answer", "Short answer", 32)
            .unwrap();
        let (events, _receiver, forwarder) =
            crate::domain::services::events::channel(&config, None);
        let (_cancel_tx, cancel) = watch::channel(false);
        assert_eq!(
            unmeasured
                .call(
                    &unmeasured_input,
                    request,
                    CallStreamRole::Answer,
                    &events,
                    cancel
                )
                .await
                .err(),
            Some(DriverError::InvalidInput)
        );
        drop(events);
        forwarder.await.unwrap();
        assert_eq!(unmeasured_budget.used(), 0);

        let excess_budget = Arc::new(TokenBudget::new(512));
        let excess_input = make_thinking_input(&model, &context, excess_budget.clone());
        let excess_fake = Arc::new(FakeDriver {
            outcomes: std::sync::Mutex::new(VecDeque::from([outcome(true, 500, 20)])),
            calls: std::sync::Mutex::new(Vec::new()),
        });
        let excess = Thinking::new(&config, excess_fake);
        let request = excess
            .call_input(&excess_input, &[], "answer", "Short answer", 32)
            .unwrap();
        let (events, _receiver, forwarder) =
            crate::domain::services::events::channel(&config, None);
        let (_cancel_tx, cancel) = watch::channel(false);
        assert_eq!(
            excess
                .call(
                    &excess_input,
                    request,
                    CallStreamRole::Answer,
                    &events,
                    cancel
                )
                .await
                .err(),
            Some(DriverError::InvalidInput)
        );
        drop(events);
        forwarder.await.unwrap();
        assert_eq!(excess_budget.used(), 0);
    }

    #[tokio::test]
    async fn cancelled_provider_call_is_awaited_until_the_driver_stops() {
        let config = Config::default();
        let caller = Caller {
            app_id: "cortex".into(),
            user_id: Some("owner".into()),
            is_admin: false,
        };
        let model = OutputModel::new(&caller, uuid::Uuid::now_v7(), false);
        let context = AssembledContext {
            question: "wait".into(),
            system: String::new(),
            prompt: "Question: wait".into(),
            system_personas: vec![],
            texts: vec![],
            history: vec![],
            estimated_input_tokens: 0,
            reserved_output_tokens: 0,
            reserved_notes_tokens: 0,
        };
        let budget = Arc::new(TokenBudget::new(256));
        let input = make_thinking_input(&model, &context, budget.clone());
        let stopped = Arc::new(AtomicBool::new(false));
        let thinking = Thinking::new(
            &config,
            Arc::new(PausedDriver {
                stopped: stopped.clone(),
            }),
        );
        let request = thinking
            .call_input(&input, &[], "answer", "Stop safely", 64)
            .unwrap();
        let (events, _receiver, forwarder) =
            crate::domain::services::events::channel(&config, None);
        let call_events = events.clone();
        let call_budget = budget.clone();
        let (cancel_tx, cancel) = watch::channel(false);
        let call = tokio::spawn(async move {
            let input = make_thinking_input(&model, &context, call_budget);
            thinking
                .call(
                    &input,
                    request,
                    CallStreamRole::Answer,
                    &call_events,
                    cancel,
                )
                .await
        });
        tokio::task::yield_now().await;
        cancel_tx.send(true).unwrap();
        assert_eq!(call.await.unwrap().err(), Some(DriverError::Cancelled));
        assert!(stopped.load(Ordering::Acquire));
        assert_eq!(budget.used(), 0);
        drop(events);
        forwarder.await.unwrap();
    }
}

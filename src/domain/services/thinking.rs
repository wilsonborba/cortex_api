use crate::{
    core::{config::Config, process_numbers},
    domain::{
        models::{
            context::ContextText,
            driver::{CallInput, CallOutcome, DriverError, ModelDriver},
            events::{CallStreamRole, NativeEvent, StepName, StepProgress, ThinkingRole},
            pipeline::StageName,
            thinking::*,
        },
        services::{context::encode, crawler::cancelled, events::EventSink},
    },
};
use std::sync::Arc;
use tokio::{
    sync::{mpsc, watch},
    time::Instant,
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
        Ok(CallInput {
            role: role.into(),
            system,
            prompt,
            images: input.images.clone(),
            temperature: self.config.query_temperature,
            max_tokens,
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
        let outcome = self.driver.call(input.model, call, tx, signal).await;
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
        if let Some(error) = thinking.error {
            return Err(error);
        }
        if *cancel.borrow() {
            return Err(DriverError::Cancelled);
        }
        let call = self.call_input(
            input,
            &thinking.notes,
            "answer",
            &self.config.thinking_final_instruction,
            self.config.answer_token_reserve as u32,
        )?;
        if !thinking.calls.is_empty() {
            let mut step = StepProgress::new(StageName::Answer, StepName::ThinkingStep);
            step.params.insert("role".into(), "answer".into());
            step.progress = Some([
                thinking.calls.len() as u64 + 1,
                thinking.calls.len() as u64 + 1,
            ]);
            events.send(NativeEvent::Step(step));
        }
        self.call(input, call, CallStreamRole::Answer, events, cancel)
            .await
    }
}

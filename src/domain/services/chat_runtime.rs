use crate::{
    core::{
        config::Config,
        process_numbers::{self, ProcessNumber},
    },
    dal::{local::chats::Chats, remote::event_journal::EventJournal},
    domain::{
        models::{
            chat_runtime::*,
            chat_session::{InputIds, SessionFile},
            context::{ContextInput, ContextText, Exchange},
            driver::{DriverError, OutputModel},
            events::{Memory, NativeEvent, OutputEvent, StepName, StepProgress},
            history::{FinishedOutput, FinishedStatus},
            personas::PersonaError,
            pipeline::*,
            replay::StoredEvent,
            run_evidence::RunIdentity,
            run_record::{RunCounts, RunMode, WebMode},
            thinking::{ThinkingInput, ThinkingResult},
            usage::{Pending, Usage},
            web::WebInput,
        },
        services::{
            chat_session, chunks, context, events::EventSink, exchange_memory::ExchangeMemories,
            files::FileProxy, history::History, input_guard, pipeline, recall::MemoryRecall,
            run_evidence::RunEvidence, security::SecurityShield, telemetry_reads::TelemetryReads,
            temporary::TemporaryChats, thinking::Thinking,
        },
    },
};
use chrono::Utc;
use std::{sync::Arc, time::Instant};
use tokio::sync::{mpsc, oneshot, watch};

/// Every service that one chat output uses, from Accept to Save.
pub struct ChatRuntime {
    pub config: Config,
    pub chats: Chats,
    pub history: History,
    pub temporary: TemporaryChats,
    pub files: FileProxy,
    pub shield: Arc<SecurityShield>,
    pub recall: MemoryRecall,
    pub web: crate::domain::services::crawler::WebRetriever,
    pub thinking: Thinking,
    pub memories: ExchangeMemories,
    pub runs: RunEvidence,
    pub telemetry: TelemetryReads,
    pub journal: EventJournal,
    pub personas: crate::domain::services::personas::Personas,
    pub controls: crate::domain::services::output_controls::OutputControls,
}
/// The final state of one output, for an input sent without `stream`.
pub struct Finished {
    pub ids: Option<InputIds>,
    pub error: Option<ProcessNumber>,
    pub output: Option<FinishedOutput>,
}
fn failed<T>(data: T, process: ProcessNumber) -> StageValue<T> {
    let mut value = StageValue::ok(data);
    value.status = StageStatus::Failed(process);
    value
}
fn driver_status(error: DriverError) -> StageStatus {
    match crate::presentation::model_error::number(error) {
        Some(process) => StageStatus::Failed(process),
        None => StageStatus::Cancelled,
    }
}
fn web_mode(name: &str) -> WebMode {
    match name {
        "medium" => WebMode::Medium,
        "deep" => WebMode::Deep,
        _ => WebMode::Fast,
    }
}
struct Run {
    runtime: Arc<ChatRuntime>,
    events: EventSink,
    cancel: watch::Receiver<bool>,
    stop: watch::Receiver<bool>,
    ids: watch::Sender<Option<(InputIds, bool)>>,
    control: crate::domain::services::output_controls::Control,
    finished: std::sync::Mutex<Option<FinishedOutput>>,
}
impl Run {
    async fn accept_turn(&self, turn: &Turn) -> Result<AcceptedTurn, ProcessNumber> {
        let rt = &self.runtime;
        let caller = &turn.caller;
        let (text, file_ids) = match turn.request.as_ref() {
            TurnRequest::New(body) => (&body.text, &body.file_ids),
            TurnRequest::Next(_, body) => (&body.text, &body.file_ids),
        };
        input_guard::check(&rt.config, text).map_err(|_| process_numbers::INPUT_TOKEN_LIMIT)?;
        // Security runs before anything is saved (9.1).
        rt.shield.check(text).await.map_err(|e| e.process())?;
        self.events.send(NativeEvent::Step(StepProgress::new(
            StageName::Accept,
            StepName::SecurityOk,
        )));
        if let TurnRequest::New(body) = turn.request.as_ref()
            && let Some(id) = &body.persona_id
        {
            rt.personas
                .check_bot(caller, id)
                .await
                .map_err(|e| match e {
                    PersonaError::Process(process) => process,
                    PersonaError::Invalid(_) => process_numbers::PERSONA_NOT_FOUND,
                })?;
        }
        let temporary = match turn.request.as_ref() {
            TurnRequest::New(body) => body.temporary,
            TurnRequest::Next(session, _) => rt
                .chats
                .owned_chat(caller.user_id.as_deref().unwrap_or_default(), *session)
                .await
                .map_err(|_| process_numbers::HISTORY_FAILED)?
                .is_none(),
        };
        let now = Utc::now();
        if temporary {
            let chat = match turn.request.as_ref() {
                TurnRequest::New(body) => rt.temporary.start(caller, body.clone(), now).await?,
                TurnRequest::Next(session, body) => {
                    rt.temporary
                        .append(caller, *session, body.clone(), now)
                        .await?
                }
            };
            let input = chat.inputs.last().ok_or(process_numbers::CHAT_NOT_FOUND)?;
            let output = &chat
                .outputs
                .last()
                .ok_or(process_numbers::CHAT_NOT_FOUND)?
                .output;
            let history = chat
                .outputs
                .iter()
                .rev()
                .filter(|o| o.output.status == "done")
                .filter_map(|o| {
                    chat.inputs
                        .iter()
                        .find(|i| i.input_id == o.output.input_id)
                        .map(|i| Exchange {
                            input: i.text.clone(),
                            output: o.output.text.clone(),
                        })
                })
                .collect();
            let ids = InputIds {
                session_id: chat.session_id,
                input_id: input.input_id,
                output_id: output.output_id,
            };
            return Ok(self
                .accepted(
                    turn,
                    ids,
                    true,
                    input,
                    history,
                    chat.files.iter().flat_map(|f| f.pages.clone()).collect(),
                )
                .with_persona(chat.persona_id.clone()));
        }
        let mut files = vec![];
        let mut images = vec![];
        for id in file_ids {
            let file = rt.files.file(caller, id).await?;
            if file.content_type.to_ascii_lowercase().starts_with("image/") {
                use base64::Engine;
                let bytes = rt.files.download(caller, id).await?.bytes;
                images.push(base64::engine::general_purpose::STANDARD.encode(bytes));
            }
            files.push(SessionFile {
                file_id: id.clone(),
                memory_id: file.memory_id,
                name: file.name,
            });
        }
        let (prepared, persona_id) = match turn.request.as_ref() {
            TurnRequest::New(body) => (
                chat_session::new_permanent(&rt.config, caller, body.clone(), &files, now)?,
                body.persona_id.clone(),
            ),
            TurnRequest::Next(session, body) => {
                let chat = rt
                    .chats
                    .owned_chat(caller.user_id.as_deref().unwrap_or_default(), *session)
                    .await
                    .map_err(|_| process_numbers::HISTORY_FAILED)?
                    .ok_or(process_numbers::CHAT_NOT_FOUND)?;
                (
                    chat_session::continue_permanent(caller, &chat, body.clone(), &files, now)?,
                    chat.persona_id.clone(),
                )
            }
        };
        let history = match &prepared.new_chat {
            Some(_) => vec![],
            None => rt
                .history
                .recent(&prepared.scope, prepared.input.session_id)
                .await
                .map_err(|_| process_numbers::HISTORY_FAILED)?
                .iter()
                .map(|e| e.context())
                .collect(),
        };
        rt.chats
            .save_accepted(&prepared)
            .await
            .map_err(|_| process_numbers::HISTORY_FAILED)?;
        Ok(self
            .accepted(
                turn,
                prepared.ids(),
                false,
                &prepared.input,
                history,
                vec![],
            )
            .with_persona(persona_id)
            .with_images(caller, images))
    }
    fn accepted(
        &self,
        turn: &Turn,
        ids: InputIds,
        temporary: bool,
        input: &crate::domain::models::storage::ChatInput,
        history: Vec<Exchange>,
        temporary_files: Vec<ContextText>,
    ) -> AcceptedTurn {
        AcceptedTurn {
            model: OutputModel::new(&turn.caller, ids.output_id, false),
            ids,
            temporary,
            question: input.text.clone(),
            needs_web: input.needs_web,
            web_mode: web_mode(&input.web_mode),
            use_memory: input.use_memory,
            memory_topic: input.memory_topic.clone(),
            thinking: input.thinking,
            persona_id: None,
            images: vec![],
            history,
            temporary_files,
        }
    }
    /// All files of the chat are candidates, ranked by BM25 (14.3, 7.2).
    async fn file_chunks(&self, turn: &Turn) -> (Vec<ContextText>, Vec<SourceNotice>) {
        let rt = &self.runtime;
        let accepted = turn.accepted();
        let mut candidates = accepted.temporary_files.clone();
        let mut notices = vec![];
        if !accepted.temporary {
            let scope = turn.caller.storage_scope();
            match rt.history.files(&scope, accepted.ids.session_id).await {
                Ok(files) => {
                    for file in files {
                        match rt.files.file(&turn.caller, &file.file_id).await {
                            Ok(memory) => candidates.extend(
                                chunks::split(&rt.config, &memory.text)
                                    .into_iter()
                                    .enumerate()
                                    .map(|(i, text)| ContextText {
                                        label: memory.name.clone(),
                                        source: Some(format!("{}#chunk={}", file.file_id, i + 1)),
                                        text,
                                    }),
                            ),
                            Err(process) => notices.push(SourceNotice {
                                process,
                                label: "files".into(),
                                source: None,
                            }),
                        }
                    }
                }
                Err(_) => notices.push(SourceNotice {
                    process: process_numbers::HISTORY_FAILED,
                    label: "files".into(),
                    source: None,
                }),
            }
        }
        let ranked: Vec<_> = chunks::rank(&accepted.question, candidates)
            .into_iter()
            .take(rt.config.context_chunk_count)
            .map(|r| r.chunk)
            .collect();
        // A flagged file chunk is dropped; the request continues (9.5).
        let (safe, flagged) = rt.shield.filter_chunks(ranked).await;
        notices.extend(
            flagged
                .into_iter()
                .filter(|p| p.number != process_numbers::SECURITY_BLOCK.number)
                .map(|process| SourceNotice {
                    process,
                    label: "files".into(),
                    source: None,
                }),
        );
        (safe, notices)
    }
    fn finish(&self, turn: &Turn, status: FinishedStatus) -> FinishedOutput {
        FinishedOutput {
            session_id: turn.accepted().ids.session_id,
            output_id: turn.accepted().ids.output_id,
            status,
            text: turn.answer.clone(),
            thinking_text: turn
                .thinking
                .as_ref()
                .filter(|t| !t.text.is_empty())
                .map(|t| t.text.clone()),
            memory_id: None,
            sources: turn.sources.clone(),
            memories: turn.used_memories.clone(),
            finished_at: Utc::now(),
        }
    }
}
impl StageServices<Turn> for Run {
    fn accept<'a>(&'a self, mut turn: Turn) -> Pending<'a, StageValue<Turn>> {
        Box::pin(async move {
            match self.accept_turn(&turn).await {
                Ok(accepted) => {
                    let _ = self
                        .ids
                        .send(Some((accepted.ids.clone(), accepted.temporary)));
                    // Temporary chats have no reconnect, but can be cancelled.
                    self.runtime
                        .controls
                        .open(accepted.ids.output_id, self.control.clone());
                    turn.accepted = Some(Arc::new(accepted));
                    StageValue::ok(turn)
                }
                Err(process) => failed(turn, process),
            }
        })
    }
    fn files<'a>(&'a self, previous: &'a StageResult<Turn>) -> Pending<'a, StageValue<Turn>> {
        Box::pin(async move {
            let mut turn = previous.output.data.clone();
            let (files, notices) = self.file_chunks(&turn).await;
            turn.files = files;
            let mut value = StageValue::ok(turn);
            value.output.notices = notices;
            value
        })
    }
    fn memory<'a>(&'a self, previous: &'a StageResult<Turn>) -> Pending<'a, StageValue<Turn>> {
        Box::pin(async move {
            let mut turn = previous.output.data.clone();
            let accepted = turn.accepted.clone().expect("accepted");
            let recalled = self
                .runtime
                .recall
                .gather(
                    &turn.caller.storage_scope(),
                    &accepted.question,
                    accepted.use_memory,
                    accepted.memory_topic.as_deref(),
                    &self.events,
                )
                .await;
            turn.files = recalled.files;
            turn.memories = recalled.memories;
            turn.references = recalled.references;
            let mut notices = recalled.notices;
            // Personas are always on, with no flag (23). A failure is a notice.
            match self
                .runtime
                .personas
                .for_context(
                    &turn.caller,
                    &accepted.question,
                    accepted.persona_id.as_deref(),
                    self.runtime.config.named_persona_count,
                )
                .await
            {
                Ok(personas) => turn.personas = personas,
                Err(_) => notices.push(SourceNotice {
                    process: process_numbers::PERSONA_UNAVAILABLE,
                    label: "personas".into(),
                    source: None,
                }),
            }
            let mut value = StageValue::ok(turn);
            value.output.notices = notices;
            value
        })
    }
    fn web<'a>(&'a self, previous: &'a StageResult<Turn>) -> Pending<'a, StageValue<Turn>> {
        Box::pin(async move {
            let mut turn = previous.output.data.clone();
            let accepted = turn.accepted.clone().expect("accepted");
            let result = self
                .runtime
                .web
                .gather(
                    WebInput {
                        mode: accepted.web_mode,
                        question: &accepted.question,
                        needs_web: accepted.needs_web,
                        model: &accepted.model,
                        images: vec![],
                    },
                    &self.events,
                    self.cancel.clone(),
                )
                .await;
            turn.web = result.gathered.texts;
            turn.sources = result.gathered.sources;
            turn.pages_read = result.gathered.pages_read;
            let mut value = StageValue::ok(turn);
            if let Some(error) = result.error {
                value.status = driver_status(error);
            }
            value.output.calls = result.calls;
            value.output.notices = result.gathered.notices;
            value
        })
    }
    fn assemble<'a>(&'a self, previous: &'a StageResult<Turn>) -> Pending<'a, StageValue<Turn>> {
        Box::pin(async move {
            let mut turn = previous.output.data.clone();
            let accepted = turn.accepted.clone().expect("accepted");
            let assembled = context::assemble(
                &self.runtime.config,
                ContextInput {
                    question: accepted.question.clone(),
                    thinking: accepted.thinking,
                    history: accepted.history.clone(),
                    files: std::mem::take(&mut turn.files),
                    memories: std::mem::take(&mut turn.memories),
                    web: std::mem::take(&mut turn.web),
                    self_persona: turn.personas.self_persona.clone(),
                    bot_persona: turn.personas.bot_persona.clone(),
                    named_personas: turn.personas.named.clone(),
                },
            );
            let Ok(assembled) = assembled else {
                return failed(turn, process_numbers::INPUT_TOKEN_LIMIT);
            };
            // Only memories that survive the final context are reported (6.7).
            let used: Vec<Memory> = turn
                .references
                .iter()
                .filter(|m| {
                    assembled
                        .texts
                        .iter()
                        .any(|t| t.source.as_deref() == Some(&m.id))
                })
                .cloned()
                .collect();
            if accepted.use_memory {
                self.events.send(NativeEvent::Memories(used.clone()));
            }
            turn.used_memories = used;
            turn.context = Some(Arc::new(assembled));
            StageValue::ok(turn)
        })
    }
    fn think<'a>(&'a self, previous: &'a StageResult<Turn>) -> Pending<'a, StageValue<Turn>> {
        Box::pin(async move {
            let mut turn = previous.output.data.clone();
            let accepted = turn.accepted.clone().expect("accepted");
            let context = turn.context.clone().expect("assembled");
            let result = self
                .runtime
                .thinking
                .think(
                    &ThinkingInput {
                        model: &accepted.model,
                        context: &context,
                        images: accepted.images.clone(),
                    },
                    &self.events,
                    self.stop.clone(),
                    self.cancel.clone(),
                )
                .await;
            let calls = result.calls.clone();
            let status = match (result.error, result.notice) {
                (Some(error), _) => driver_status(error),
                (None, Some(notice)) => StageStatus::Notice(notice),
                (None, None) => StageStatus::Ok,
            };
            turn.thinking = Some(Arc::new(result));
            let mut value = StageValue::ok(turn);
            value.status = status;
            value.output.calls = calls;
            value
        })
    }
    fn answer<'a>(&'a self, previous: &'a StageResult<Turn>) -> Pending<'a, StageValue<Turn>> {
        Box::pin(async move {
            let mut turn = previous.output.data.clone();
            let accepted = turn.accepted.clone().expect("accepted");
            let context = turn.context.clone().expect("assembled");
            let empty = ThinkingResult::default();
            let thinking = turn.thinking.clone();
            let outcome = self
                .runtime
                .thinking
                .answer(
                    &ThinkingInput {
                        model: &accepted.model,
                        context: &context,
                        images: accepted.images.clone(),
                    },
                    thinking.as_deref().unwrap_or(&empty),
                    &self.events,
                    self.cancel.clone(),
                )
                .await;
            match outcome {
                Ok(outcome) => {
                    // A failed or cancelled answer keeps its partial text (14.1).
                    turn.answer = outcome.text;
                    let mut value = StageValue::ok(turn);
                    if let Some(error) = outcome.error {
                        value.status = driver_status(error);
                    }
                    value.output.calls = vec![outcome.usage];
                    value
                }
                Err(error) => {
                    let mut value = StageValue::ok(turn);
                    value.status = driver_status(error);
                    value
                }
            }
        })
    }
    fn save<'a>(&'a self, previous: &'a StageResult<Turn>) -> Pending<'a, StageValue<Turn>> {
        Box::pin(async move {
            let turn = previous.output.data.clone();
            let accepted = turn.accepted.clone().expect("accepted");
            let status = match previous.status {
                StageStatus::Failed(_) => FinishedStatus::Failed,
                StageStatus::Cancelled => FinishedStatus::Cancelled,
                _ => FinishedStatus::Done,
            };
            let mut done = self.finish(&turn, status);
            let rt = &self.runtime;
            let scope = turn.caller.storage_scope();
            let mut value = StageValue::ok(turn.clone());
            if accepted.temporary {
                if let Err(process) = rt.temporary.finish(&turn.caller, &done).await {
                    value.status = StageStatus::Failed(process);
                }
            } else if rt.history.save(&scope, &done).await.is_err() {
                value.status = StageStatus::Failed(process_numbers::HISTORY_FAILED);
            } else if matches!(status, FinishedStatus::Done) {
                let saved = rt
                    .memories
                    .remember(&scope, done.session_id, done.output_id, false)
                    .await;
                done.memory_id = saved.memory_id;
                if let Some(notice) = saved.notice {
                    value.status = StageStatus::Notice(notice);
                    value.output.notices.push(SourceNotice {
                        process: notice,
                        label: "save".into(),
                        source: None,
                    });
                }
            }
            *self.finished.lock().unwrap_or_else(|e| e.into_inner()) = Some(done);
            value
        })
    }
}
impl ChatRuntime {
    /// Start one output. It runs on the server even if the client goes away
    /// (10.4). Stream events go to `stream`; `done` gets the final state.
    pub fn start(
        self: &Arc<Self>,
        turn: Turn,
        stream: Option<mpsc::UnboundedSender<OutputEvent>>,
    ) -> oneshot::Receiver<Finished> {
        let runtime = self.clone();
        let (done_tx, done_rx) = oneshot::channel();
        tokio::spawn(async move {
            let started = Instant::now();
            let created_at = Utc::now();
            let caller = turn.caller.clone();
            // The first stage event carries the average of recent completed runs of
            // this kind (12.4); null when none exists.
            let (web, thinking) = match turn.request.as_ref() {
                TurnRequest::New(body) => (body.web_mode, body.thinking),
                TurnRequest::Next(_, body) => (body.web_mode, body.thinking),
            };
            let expected = runtime
                .telemetry
                .expected(&RunMode {
                    web_mode: web,
                    thinking,
                })
                .await;
            let (events, mut output, events_task) =
                crate::domain::services::events::channel(&runtime.config, expected);
            let (stage_tx, mut stage_rx) = mpsc::unbounded_channel();
            let sink = events.clone();
            let forward = tokio::spawn(async move {
                while let Some(event) = stage_rx.recv().await {
                    sink.pipeline(event);
                }
            });
            // The output id is known only after Accept. Events wait until then.
            let (ids_tx, ids_rx) = watch::channel::<Option<(InputIds, bool)>>(None);
            let (cancel_tx, cancel) = watch::channel(false);
            let (stop_tx, stop) = watch::channel(false);
            let control = crate::domain::services::output_controls::Control {
                cancel: Arc::new(cancel_tx),
                stop_thinking: Arc::new(stop_tx),
                live: tokio::sync::broadcast::channel(runtime.config.event_read_batch.max(1)).0,
            };
            let live = control.live.clone();
            let journal = runtime.journal.clone();
            let deliver = tokio::spawn(async move {
                let mut ids_rx = ids_rx;
                let mut pending = vec![];
                while let Some(event) = output.recv().await {
                    if let Some(stream) = &stream {
                        let _ = stream.send(event.clone());
                    }
                    pending.push(event);
                    let ids = ids_rx.borrow_and_update().clone();
                    // Temporary chats have no reconnect (10.4.8).
                    if let Some((ids, false)) = ids {
                        for event in pending.drain(..) {
                            let stored = StoredEvent::from(&event);
                            let _ = journal.append(ids.output_id, &stored).await;
                            // Live after the journal, so a reconnect misses nothing.
                            let _ = live.send(stored);
                        }
                    }
                }
            });
            let run = Run {
                runtime: runtime.clone(),
                events: events.clone(),
                cancel,
                stop,
                ids: ids_tx,
                control,
                finished: Default::default(),
            };
            let reporter = pipeline::StageReporter::new(stage_tx);
            let result = pipeline::run(turn, &run, &reporter).await;
            let accepted = result
                .stages
                .first()
                .and_then(|s| s.output.data.accepted.clone())
                .map(|a| (a.ids.clone(), a.temporary, a.web_mode, a.thinking));
            let total_ms = started.elapsed().as_millis() as u64;
            let error = result.error();
            let finished = run
                .finished
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .take();
            match (error, &accepted) {
                (Some(process), _) => events.send(NativeEvent::Error(process)),
                (None, Some((ids, ..))) => {
                    let tokens = result
                        .stages
                        .last()
                        .map(|s| {
                            s.output.calls.iter().fold(Usage::default(), |mut u, c| {
                                u.input = u.input.saturating_add(c.input_tokens);
                                u.output = u.output.saturating_add(c.output_tokens);
                                u
                            })
                        })
                        .unwrap_or_default();
                    events.send(NativeEvent::Done {
                        session_id: ids.session_id,
                        output_id: ids.output_id,
                        tokens,
                        total_ms,
                    });
                }
                (None, None) => {}
            }
            if let Some((ids, _, web_mode, thinking)) = &accepted {
                let mode = RunMode {
                    web_mode: *web_mode,
                    thinking: *thinking,
                };
                let last = result.stages.last().map(|s| &s.output.data);
                let counts = RunCounts {
                    pages_read: last.map(|t| t.pages_read).unwrap_or(0),
                    chunks_used: last
                        .and_then(|t| t.context.as_ref())
                        .map(|c| c.texts.len() as u32)
                        .unwrap_or(0),
                    files: 0,
                    file_pages: 0,
                    audio_seconds: 0.0,
                };
                if runtime
                    .runs
                    .save(
                        RunIdentity {
                            session_id: ids.session_id,
                            output_id: ids.output_id,
                            app_id: caller.app_id.clone(),
                            user_id: caller.user_id.clone().unwrap_or_default(),
                            created_at,
                            mode: mode.clone(),
                        },
                        &result,
                        counts,
                        total_ms,
                    )
                    .await
                    .is_err()
                {
                    tracing::warn!("Run record save failed");
                }
                if let Some(done) = &finished {
                    runtime
                        .telemetry
                        .completed(&mode, ids.output_id, total_ms, done.status)
                        .await;
                }
            }
            if let Some((ids, ..)) = &accepted {
                runtime.controls.close(ids.output_id);
            }
            drop(run);
            drop(reporter);
            let _ = forward.await;
            drop(events);
            let _ = events_task.await;
            let _ = deliver.await;
            let _ = done_tx.send(Finished {
                ids: accepted.map(|a| a.0),
                error,
                output: finished,
            });
        });
        done_rx
    }
}

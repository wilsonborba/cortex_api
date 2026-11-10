use crate::{
    core::config::Config,
    domain::models::{
        driver::DriverEvent,
        events::*,
        pipeline::{PipelineEvent, StageName},
    },
};
use tokio::{
    sync::mpsc,
    task::JoinHandle,
    time::{Instant, sleep_until},
};
#[derive(Clone)]
pub struct EventSink {
    tx: mpsc::UnboundedSender<NativeEvent>,
}
impl EventSink {
    pub fn send(&self, event: NativeEvent) {
        if event.public() {
            let _ = self.tx.send(event);
        }
    }
    pub fn pipeline(&self, event: PipelineEvent) {
        match event {
            PipelineEvent::StageStarted(stage) => {
                self.send(NativeEvent::Stage(StageProgress::start(stage)))
            }
            PipelineEvent::Stage(record) => {
                let stage = match record.name.as_str() {
                    "accept" => StageName::Accept,
                    "gather" => StageName::Gather,
                    "assemble" => StageName::Assemble,
                    "think" => StageName::Think,
                    "answer" => StageName::Answer,
                    "save" => StageName::Save,
                    _ => return,
                };
                self.send(NativeEvent::Stage(StageProgress::end(stage, record)));
            }
            PipelineEvent::Notice(notice) => self.send(NativeEvent::Notice(notice.process)),
        }
    }
    pub fn driver(&self, stage: StageName, role: CallStreamRole, event: DriverEvent) {
        let event = match event {
            DriverEvent::Waiting => {
                NativeEvent::Step(StepProgress::new(stage, StepName::QueueWait))
            }
            DriverEvent::ModelLoading { model } => {
                let mut step = StepProgress::new(stage, StepName::ModelLoad);
                step.params.insert("model".into(), model.into());
                NativeEvent::Step(step)
            }
            DriverEvent::ModelReady { model, load_ms } => {
                let mut step = StepProgress::new(stage, StepName::ModelReady);
                step.params.insert("model".into(), model.into());
                step.params.insert("load_ms".into(), load_ms.into());
                NativeEvent::Step(step)
            }
            DriverEvent::Token { text, .. } => match role {
                CallStreamRole::Answer => NativeEvent::Token(text),
                CallStreamRole::Thinking { role, round } => {
                    NativeEvent::Thinking { role, round, text }
                }
                CallStreamRole::Queries => return,
            },
        };
        self.send(event);
    }
}
/// The output task owns the sink. Dropping an HTTP receiver does not stop work.
pub fn channel(
    config: &Config,
    expected_ms: Option<u64>,
) -> (
    EventSink,
    mpsc::UnboundedReceiver<OutputEvent>,
    JoinHandle<()>,
) {
    let (tx, mut rx) = mpsc::unbounded_channel::<NativeEvent>();
    let (output, receiver) = mpsc::unbounded_channel();
    let heartbeat = config.stream_heartbeat;
    let task = tokio::spawn(async move {
        let mut id = 0u64;
        let mut first_stage = true;
        let mut step: Option<(StepProgress, Instant)> = None;
        let timer = sleep_until(Instant::now() + heartbeat);
        tokio::pin!(timer);
        loop {
            let event = tokio::select! {
                event=rx.recv()=>match event { Some(mut event)=>{
                    if !event.public(){continue;}
                    match &mut event {
                        NativeEvent::Stage(stage)=>{step=None;if first_stage{stage.expected_ms=Some(expected_ms);first_stage=false;}},
                        NativeEvent::Step(progress)=>step=Some((progress.clone(),Instant::now())),
                        NativeEvent::Done{..}=>{},_=>{}
                    }
                    timer.as_mut().reset(Instant::now()+heartbeat);event
                },None=>break},
                _=&mut timer,if step.is_some()=>{
                    let (progress,start)=step.as_ref().expect("active step");let mut progress=progress.clone();progress.elapsed_ms=progress.elapsed_ms.saturating_add(start.elapsed().as_millis() as u64);
                    timer.as_mut().reset(Instant::now()+heartbeat);NativeEvent::Step(progress)
                }
            };
            let done = matches!(event, NativeEvent::Done { .. });
            id += 1;
            let _ = output.send(OutputEvent { id, event });
            if done {
                break;
            }
        }
    });
    (EventSink { tx }, receiver, task)
}

use crate::{
    core::process_numbers,
    domain::models::{pipeline::*, run_record::StageRecord},
};
use std::{
    sync::{Arc, Mutex},
    time::Instant,
};
use tokio::sync::mpsc;
#[derive(Clone)]
pub struct StageReporter {
    events: mpsc::UnboundedSender<PipelineEvent>,
    records: Arc<Mutex<Vec<StageRecord>>>,
}
impl StageReporter {
    pub fn new(events: mpsc::UnboundedSender<PipelineEvent>) -> Self {
        Self {
            events,
            records: Default::default(),
        }
    }
    pub fn records(&self) -> Vec<StageRecord> {
        self.records
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
    fn start(&self, stage: StageName) {
        let _ = self.events.send(PipelineEvent::StageStarted(stage));
    }
    fn publish<T>(&self, result: &StageResult<T>, new_notices: &[SourceNotice]) {
        let record = result.record();
        tracing::info!(stage=%record.name,status=%record.status,time_ms=record.time_ms,error_number=?record.error_number,notice_number=?record.notice_number,"Stage completed");
        self.records
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(record.clone());
        let _ = self.events.send(PipelineEvent::Stage(record));
        for notice in new_notices {
            tracing::warn!(notice_number = notice.process.number, "Stage source notice");
            let _ = self.events.send(PipelineEvent::Notice(notice.clone()));
        }
    }
}
fn finish<T>(
    name: StageName,
    start: Instant,
    previous: Option<&StageResult<T>>,
    mut value: StageValue<T>,
    reporter: &StageReporter,
) -> StageResult<T> {
    let new_notices = value.output.notices.clone();
    if let Some(previous) = previous {
        value
            .output
            .calls
            .splice(0..0, previous.output.calls.clone());
        value
            .output
            .notices
            .splice(0..0, previous.output.notices.clone());
    }
    value.output.calls.sort_by_key(|c| c.started_at);
    let result = StageResult {
        stage: name,
        status: value.status,
        time_ms: start.elapsed().as_millis() as u64,
        output: value.output,
    };
    reporter.publish(&result, &new_notices);
    result
}
async fn save<T: PipelineData>(
    stages: &mut Vec<StageResult<T>>,
    services: &dyn StageServices<T>,
    reporter: &StageReporter,
) {
    let start = Instant::now();
    let previous = stages.last().expect("prior stage");
    reporter.start(StageName::Save);
    let value = services.save(previous).await;
    let result = finish(StageName::Save, start, Some(previous), value, reporter);
    stages.push(result);
}
pub async fn run<T: PipelineData>(
    input: T,
    services: &dyn StageServices<T>,
    reporter: &StageReporter,
) -> PipelineResult<T> {
    reporter.start(StageName::Accept);
    let start = Instant::now();
    let accepted = finish(
        StageName::Accept,
        start,
        None,
        services.accept(input).await,
        reporter,
    );
    let terminal = accepted.status.terminal();
    let mut stages = vec![accepted];
    // Rejected inputs stop here. The caller writes only a redacted run record.
    if terminal {
        return PipelineResult { stages };
    }
    let previous = stages.last().expect("accepted");
    let start = Instant::now();
    reporter.start(StageName::Gather);
    let (mut files, mut memory, mut web) = tokio::join!(
        services.files(previous),
        services.memory(previous),
        services.web(previous)
    );
    let mut notices = vec![];
    let mut cancelled = false;
    let mut fatal = None;
    for (label, value) in [
        ("files", &mut files),
        ("memory", &mut memory),
        ("web", &mut web),
    ] {
        match value.status {
            StageStatus::Failed(process)
                if [
                    process_numbers::HEAVY_LOAD.number,
                    process_numbers::QUEUE_FAULT.number,
                    process_numbers::OLLAMA_FAILED.number,
                ]
                .contains(&process.number) =>
            {
                fatal.get_or_insert(process);
            }
            StageStatus::Failed(process) | StageStatus::Notice(process) => {
                value.output.notices.push(SourceNotice {
                    process,
                    label: label.into(),
                    source: None,
                });
            }
            StageStatus::Cancelled => cancelled = true,
            StageStatus::Ok => {}
        }
        notices.append(&mut value.output.notices);
    }
    let status = if let Some(process) = fatal {
        StageStatus::Failed(process)
    } else if cancelled {
        StageStatus::Cancelled
    } else if let Some(notice) = notices.first() {
        StageStatus::Notice(notice.process)
    } else {
        StageStatus::Ok
    };
    let data = T::merge_gather(
        &previous.output.data,
        files.output.data,
        memory.output.data,
        web.output.data,
    );
    let texts = files
        .output
        .texts
        .into_iter()
        .chain(memory.output.texts)
        .chain(web.output.texts)
        .collect();
    let calls = files
        .output
        .calls
        .into_iter()
        .chain(memory.output.calls)
        .chain(web.output.calls)
        .collect();
    let result = finish(
        StageName::Gather,
        start,
        Some(previous),
        StageValue {
            status,
            output: StageOutput {
                data,
                texts,
                calls,
                notices,
            },
        },
        reporter,
    );
    let terminal = result.status.terminal();
    stages.push(result);
    if terminal {
        save(&mut stages, services, reporter).await;
        return PipelineResult { stages };
    }
    for name in [StageName::Assemble, StageName::Think, StageName::Answer] {
        let previous = stages.last().expect("previous stage");
        if name == StageName::Think && !previous.output.data.thinking() {
            continue;
        }
        let start = Instant::now();
        reporter.start(name);
        let value = match name {
            StageName::Assemble => services.assemble(previous).await,
            StageName::Think => services.think(previous).await,
            StageName::Answer => services.answer(previous).await,
            _ => unreachable!(),
        };
        let result = finish(name, start, Some(previous), value, reporter);
        let terminal = result.status.terminal();
        stages.push(result);
        if terminal {
            break;
        }
    }
    save(&mut stages, services, reporter).await;
    PipelineResult { stages }
}

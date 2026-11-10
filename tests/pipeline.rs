use cortex_api::{
    core::process_numbers as numbers,
    domain::{
        models::{pipeline::*, usage::Pending},
        services::pipeline::{StageReporter, run},
    },
};
use std::sync::{Arc, Mutex};
use tokio::sync::{Barrier, mpsc};
#[derive(Clone)]
struct Data {
    thinking: bool,
    trace: Vec<&'static str>,
}
impl PipelineData for Data {
    fn thinking(&self) -> bool {
        self.thinking
    }
    fn merge_gather(previous: &Self, files: Self, memory: Self, web: Self) -> Self {
        assert_eq!(files.trace.last(), Some(&"files"));
        assert_eq!(memory.trace.last(), Some(&"memory"));
        assert_eq!(web.trace.last(), Some(&"web"));
        let mut data = previous.clone();
        data.trace.push("gather");
        data
    }
}
struct Services {
    barrier: Arc<Barrier>,
    seen: Mutex<Vec<&'static str>>,
    fail: Option<&'static str>,
    cancel: bool,
}
impl Services {
    fn value(&self, name: &'static str, mut data: Data) -> StageValue<Data> {
        self.seen.lock().unwrap().push(name);
        data.trace.push(name);
        let mut value = StageValue::ok(data);
        if self.fail == Some(name) {
            value.status = StageStatus::Failed(numbers::SERVER_ERROR);
        }
        if self.cancel && name == "answer" {
            value.status = StageStatus::Cancelled;
        }
        value
    }
    fn next<'a>(
        &'a self,
        name: &'static str,
        previous: &'a StageResult<Data>,
    ) -> Pending<'a, StageValue<Data>> {
        Box::pin(async move {
            let expected = match name {
                "assemble" => StageName::Gather,
                "think" => StageName::Assemble,
                "answer" if previous.output.data.thinking => StageName::Think,
                "answer" => StageName::Assemble,
                "save" => StageName::Answer,
                _ => unreachable!(),
            };
            assert_eq!(previous.stage, expected);
            self.value(name, previous.output.data.clone())
        })
    }
    fn gather<'a>(
        &'a self,
        name: &'static str,
        previous: &'a StageResult<Data>,
    ) -> Pending<'a, StageValue<Data>> {
        Box::pin(async move {
            assert_eq!(previous.stage, StageName::Accept);
            // All three futures must be polled before any can complete.
            self.barrier.wait().await;
            let mut value = self.value(name, previous.output.data.clone());
            value.output.texts.push(LabelledText {
                label: name.into(),
                text: "private input".into(),
                source: Some("private URL".into()),
            });
            value
        })
    }
}
impl StageServices<Data> for Services {
    fn accept<'a>(&'a self, data: Data) -> Pending<'a, StageValue<Data>> {
        Box::pin(async move { self.value("accept", data) })
    }
    fn files<'a>(&'a self, p: &'a StageResult<Data>) -> Pending<'a, StageValue<Data>> {
        self.gather("files", p)
    }
    fn memory<'a>(&'a self, p: &'a StageResult<Data>) -> Pending<'a, StageValue<Data>> {
        self.gather("memory", p)
    }
    fn web<'a>(&'a self, p: &'a StageResult<Data>) -> Pending<'a, StageValue<Data>> {
        self.gather("web", p)
    }
    fn assemble<'a>(&'a self, p: &'a StageResult<Data>) -> Pending<'a, StageValue<Data>> {
        self.next("assemble", p)
    }
    fn think<'a>(&'a self, p: &'a StageResult<Data>) -> Pending<'a, StageValue<Data>> {
        self.next("think", p)
    }
    fn answer<'a>(&'a self, p: &'a StageResult<Data>) -> Pending<'a, StageValue<Data>> {
        self.next("answer", p)
    }
    fn save<'a>(&'a self, p: &'a StageResult<Data>) -> Pending<'a, StageValue<Data>> {
        self.next("save", p)
    }
}
async fn check(thinking: bool, fail: Option<&'static str>, cancel: bool) {
    let services = Services {
        barrier: Arc::new(Barrier::new(3)),
        seen: Default::default(),
        fail,
        cancel,
    };
    let (tx, mut rx) = mpsc::unbounded_channel();
    let reporter = StageReporter::new(tx);
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        run(
            Data {
                thinking,
                trace: vec![],
            },
            &services,
            &reporter,
        ),
    )
    .await
    .unwrap();
    let mut expected = vec!["accept"];
    if fail != Some("accept") {
        expected.extend(["gather", "assemble"]);
        if thinking {
            expected.push("think");
        }
        expected.extend(["answer", "save"]);
    }
    assert_eq!(
        result
            .stages
            .iter()
            .map(|s| s.stage.name())
            .collect::<Vec<_>>(),
        expected
    );
    let records = reporter.records();
    assert_eq!(records.len(), expected.len());
    let mut started = vec![];
    let mut streamed = vec![];
    let mut notices = 0;
    while let Ok(event) = rx.try_recv() {
        match event {
            PipelineEvent::Stage(record) => streamed.push(record),
            PipelineEvent::Notice(_) => notices += 1,
            PipelineEvent::StageStarted(stage) => started.push(stage.name()),
        }
    }
    assert_eq!(started, expected);
    assert_eq!(streamed, records);
    assert_eq!(
        records,
        result
            .stages
            .iter()
            .map(StageResult::record)
            .collect::<Vec<_>>()
    );
    let serialized = serde_json::to_string(&records).unwrap();
    assert!(!serialized.contains("private"));
    assert_eq!(notices, usize::from(fail == Some("web")));
    assert_eq!(
        result.error().is_some(),
        matches!(fail, Some("accept" | "answer"))
    );
    assert_eq!(result.cancelled(), cancel);
    if fail != Some("accept") {
        let gather = &result.stages[1];
        assert_eq!(gather.output.texts.len(), 3);
        assert_eq!(
            gather
                .output
                .texts
                .iter()
                .map(|t| t.label.as_str())
                .collect::<Vec<_>>(),
            ["files", "memory", "web"]
        );
        assert_eq!(
            services
                .seen
                .lock()
                .unwrap()
                .iter()
                .filter(|s| **s == "answer")
                .count(),
            1
        );
    } else {
        assert_eq!(*services.seen.lock().unwrap(), ["accept"]);
    }
}
#[tokio::test]
async fn order_parallel_gather_and_one_shared_result() {
    check(false, None, false).await;
    check(true, None, false).await;
}
#[tokio::test]
async fn accept_failure_stops() {
    check(false, Some("accept"), false).await;
}
#[tokio::test]
async fn source_failure_is_notice_and_answer_continues() {
    check(false, Some("web"), false).await;
}
#[tokio::test]
async fn failed_or_cancelled_answer_still_saves() {
    check(false, Some("answer"), false).await;
    check(false, None, true).await;
}

struct FatalServices(Services);
impl StageServices<Data> for FatalServices {
    fn accept<'a>(&'a self, data: Data) -> Pending<'a, StageValue<Data>> {
        self.0.accept(data)
    }
    fn files<'a>(&'a self, p: &'a StageResult<Data>) -> Pending<'a, StageValue<Data>> {
        self.0.files(p)
    }
    fn memory<'a>(&'a self, p: &'a StageResult<Data>) -> Pending<'a, StageValue<Data>> {
        self.0.memory(p)
    }
    fn web<'a>(&'a self, p: &'a StageResult<Data>) -> Pending<'a, StageValue<Data>> {
        Box::pin(async move {
            let mut value = self.0.web(p).await;
            value.status = StageStatus::Failed(numbers::HEAVY_LOAD);
            value
        })
    }
    fn assemble<'a>(&'a self, p: &'a StageResult<Data>) -> Pending<'a, StageValue<Data>> {
        self.0.assemble(p)
    }
    fn think<'a>(&'a self, p: &'a StageResult<Data>) -> Pending<'a, StageValue<Data>> {
        self.0.think(p)
    }
    fn answer<'a>(&'a self, p: &'a StageResult<Data>) -> Pending<'a, StageValue<Data>> {
        self.0.answer(p)
    }
    fn save<'a>(&'a self, p: &'a StageResult<Data>) -> Pending<'a, StageValue<Data>> {
        Box::pin(async move {
            assert_eq!(p.stage, StageName::Gather);
            self.0.value("save", p.output.data.clone())
        })
    }
}
#[tokio::test]
async fn global_model_queue_failure_stops_gather_before_answer_and_still_saves() {
    let services = FatalServices(Services {
        barrier: Arc::new(Barrier::new(3)),
        seen: Default::default(),
        fail: None,
        cancel: false,
    });
    let (tx, _rx) = mpsc::unbounded_channel();
    let reporter = StageReporter::new(tx);
    let result = run(
        Data {
            thinking: false,
            trace: vec![],
        },
        &services,
        &reporter,
    )
    .await;
    assert_eq!(result.error().unwrap().number, numbers::HEAVY_LOAD.number);
    assert_eq!(
        result
            .stages
            .iter()
            .map(|s| s.stage.name())
            .collect::<Vec<_>>(),
        ["accept", "gather", "save"]
    );
    assert!(!services.0.seen.lock().unwrap().contains(&"answer"));
}

use axum::{
    Router,
    body::{Body, to_bytes},
    http::Request,
    routing::get,
};
use cortex_api::{
    core::{config::Config, process_numbers},
    domain::{
        models::{
            driver::DriverEvent,
            events::*,
            pipeline::{PipelineEvent, StageName},
            run_record::StageRecord,
            usage::Usage,
        },
        services::events::channel,
    },
    presentation::events::sse,
};
use std::sync::{Arc, Mutex};
use tokio::time::{Duration, advance};
use tower::ServiceExt;
use uuid::Uuid;
fn done() -> NativeEvent {
    NativeEvent::Done {
        session_id: Uuid::now_v7(),
        output_id: Uuid::now_v7(),
        tokens: Usage {
            input: 10,
            output: 2,
            ..Default::default()
        },
        total_ms: 123,
    }
}
#[tokio::test]
async fn doubled_output_has_native_sse_order_ids_envelope_and_no_internal_work() {
    let (sink, receiver, task) = channel(&Config::default(), Some(40000));
    sink.pipeline(PipelineEvent::StageStarted(StageName::Accept));
    sink.send(NativeEvent::Step(StepProgress::new(
        StageName::Accept,
        StepName::SecurityOk,
    )));
    sink.pipeline(PipelineEvent::Stage(StageRecord {
        name: "accept".into(),
        status: "ok".into(),
        error_number: None,
        notice_number: None,
        time_ms: 3,
    }));
    sink.send(NativeEvent::Sources(vec![Source {
        title: "Page".into(),
        url: "https://example.org/page".into(),
    }]));
    sink.send(NativeEvent::Memories(vec![Memory {
        id: "memory-id".into(),
        title: "Fact".into(),
    }]));
    sink.driver(
        StageName::Gather,
        CallStreamRole::Queries,
        DriverEvent::Token {
            role: "queries".into(),
            text: "private query generation".into(),
        },
    );
    sink.driver(
        StageName::Think,
        CallStreamRole::Thinking {
            role: ThinkingRole::Plan,
            round: 1,
        },
        DriverEvent::Token {
            role: "plan".into(),
            text: "One step".into(),
        },
    );
    sink.driver(
        StageName::Answer,
        CallStreamRole::Answer,
        DriverEvent::Token {
            role: "answer".into(),
            text: "ภาษาไทย😀".into(),
        },
    );
    sink.send(NativeEvent::Notice(process_numbers::USAGE_UNAVAILABLE));
    sink.send(NativeEvent::Error(process_numbers::OLLAMA_FAILED));
    sink.pipeline(PipelineEvent::StageStarted(StageName::Save));
    sink.send(NativeEvent::Step(StepProgress::new(
        StageName::Save,
        StepName::ChunksSelected,
    )));
    sink.pipeline(PipelineEvent::Stage(StageRecord {
        name: "save".into(),
        status: "ok".into(),
        error_number: None,
        notice_number: None,
        time_ms: 4,
    }));
    sink.send(done());
    task.await.unwrap();
    let receiver = Arc::new(Mutex::new(Some(receiver)));
    let app = Router::new().route(
        "/double",
        get(move || {
            let receiver = receiver.lock().unwrap().take().unwrap();
            async move { sse(receiver) }
        }),
    );
    let response = app
        .oneshot(Request::get("/double").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.headers()["content-type"], "text/event-stream");
    let body = String::from_utf8(
        to_bytes(response.into_body(), 100000)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    let blocks: Vec<_> = body.trim().split("\n\n").collect();
    let expected = [
        "stage", "step", "stage", "sources", "memories", "thinking", "token", "notice", "error",
        "done",
    ];
    assert_eq!(blocks.len(), expected.len());
    for (index, (block, event)) in blocks.iter().zip(expected).enumerate() {
        let lines: Vec<_> = block.lines().collect();
        assert!(lines.contains(&format!("id: {}", index + 1).as_str()));
        assert!(lines.contains(&format!("event: {event}").as_str()));
        let data = lines.iter().find_map(|l| l.strip_prefix("data: ")).unwrap();
        let value: serde_json::Value = serde_json::from_str(data).unwrap();
        assert_eq!(value.as_object().unwrap().len(), 2);
        assert!(value["message"].is_string());
        assert!(value["data"].is_object());
        if index == 0 {
            assert_eq!(value["data"]["expected_ms"], 40000);
        }
        if event == "token" {
            assert_eq!(value["data"]["text"], "ภาษาไทย😀");
        }
        if event == "done" {
            assert_eq!(value["data"]["tokens"]["total"], 12);
        }
    }
    for internal in [
        "save",
        "redis",
        "usage record",
        "security.rule",
        "private query generation",
        "chat.completion",
    ] {
        assert!(!body.contains(internal));
    }
}
#[tokio::test(start_paused = true)]
async fn heartbeat_repeats_active_step_after_five_seconds_of_silence() {
    let (sink, mut receiver, task) = channel(&Config::default(), None);
    sink.send(NativeEvent::Step(StepProgress::new(
        StageName::Answer,
        StepName::ModelLoad,
    )));
    assert_eq!(receiver.recv().await.unwrap().id, 1);
    advance(Duration::from_secs(4)).await;
    tokio::task::yield_now().await;
    assert!(receiver.try_recv().is_err());
    advance(Duration::from_secs(1)).await;
    let output = receiver.recv().await.unwrap();
    assert_eq!(output.id, 2);
    if let NativeEvent::Step(step) = output.event {
        assert_eq!(step.elapsed_ms, 5000);
        assert_eq!(step.step, StepName::ModelLoad);
    } else {
        panic!("step heartbeat expected");
    }
    sink.send(NativeEvent::Token("piece".into()));
    assert_eq!(receiver.recv().await.unwrap().id, 3);
    advance(Duration::from_secs(4)).await;
    tokio::task::yield_now().await;
    assert!(receiver.try_recv().is_err());
    advance(Duration::from_secs(1)).await;
    let output = receiver.recv().await.unwrap();
    if let NativeEvent::Step(step) = output.event {
        assert_eq!(step.elapsed_ms, 10000);
    } else {
        panic!("step expected");
    }
    sink.send(done());
    assert_eq!(receiver.recv().await.unwrap().event.name(), "done");
    task.await.unwrap();
    assert!(receiver.recv().await.is_none());
}
#[tokio::test]
async fn first_stage_has_null_expected_time_without_completed_runs() {
    let (sink, mut receiver, task) = channel(&Config::default(), None);
    sink.pipeline(PipelineEvent::StageStarted(StageName::Accept));
    sink.send(done());
    let event = receiver.recv().await.unwrap();
    let data = event.event.data();
    assert!(data.get("expected_ms").unwrap().is_null());
    task.await.unwrap();
}

use cortex_api::{
    core::config::Config,
    domain::{
        models::{
            driver::*,
            events::NativeEvent,
            identity::Caller,
            queries::*,
            run_record::{ModelCall, WebMode},
            usage::{Pending, QueueError},
        },
        services::{
            events::channel,
            queries::{QueryRewriter, parse},
        },
    },
};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use tokio::sync::{mpsc, watch};
use uuid::Uuid;
#[test]
fn plain_lines_limits_and_empty_or_json_fallback() {
    assert_eq!(
        parse("one\ntwo\nthree\nfour\nfive", "question", 3),
        ["one", "two", "three"]
    );
    assert_eq!(
        parse("one\ntwo\nthree\nfour\nfive", "question", 4),
        ["one", "two", "three", "four"]
    );
    assert_eq!(
        parse(" \n\t", "original question", 3),
        ["original question"]
    );
    assert_eq!(
        parse(" {\"query\":\"unwanted JSON\"}", "original question", 3),
        ["original question"]
    );
    assert_eq!(
        parse("```\n", "original question", 3),
        ["original question"]
    );
    assert_eq!(
        parse("ไทย\nportuguês\n English  ", "question", 3),
        ["ไทย", "português", "English"]
    );
}
struct Driver {
    calls: AtomicUsize,
    before: bool,
    after: bool,
}
impl ModelDriver for Driver {
    fn call<'a>(
        &'a self,
        output: &'a OutputModel,
        input: CallInput,
        events: mpsc::UnboundedSender<DriverEvent>,
        _: watch::Receiver<bool>,
    ) -> Pending<'a, Result<CallOutcome, DriverError>> {
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::SeqCst);
            assert_eq!(input.role, "queries");
            assert!(input.system.contains("Write one query per line"));
            assert!(input.prompt.contains("Question: user question"));
            assert_eq!(input.prompt.matches("Question:").count(), 2);
            if output.is_vision() {
                assert_eq!(input.images, ["image-fixture"]);
            }
            events.send(DriverEvent::Waiting).unwrap();
            events
                .send(DriverEvent::Token {
                    role: "queries".into(),
                    text: "private query tokens".into(),
                })
                .unwrap();
            if self.before {
                return Err(DriverError::Queue(QueueError::HeavyLoad));
            }
            let now = chrono::Utc::now();
            Ok(CallOutcome {
                text: "first\nsecond\nthird\nfourth\nfifth".into(),
                usage: ModelCall {
                    images: None,
                    audio: None,
                    role: "queries".into(),
                    model: output.name(&Config::default()).into(),
                    input_tokens: 12,
                    output_tokens: 6,
                    time_to_first_token_ms: Some(1),
                    total_ms: 2,
                    started_at: now,
                    finished_at: now,
                },
                error: if self.after {
                    Some(DriverError::Ollama)
                } else {
                    None
                },
                load_ms: 0,
                tokens_per_second: None,
            })
        })
    }
}
#[tokio::test]
async fn fast_has_no_call_medium_and_deep_have_one_with_usage_and_private_tokens() {
    let config = Config::default();
    let driver = Arc::new(Driver {
        calls: AtomicUsize::new(0),
        before: false,
        after: false,
    });
    let service = QueryRewriter::new(&config, driver.clone());
    let actor = Caller {
        app_id: "cortex".into(),
        user_id: Some("fixture".into()),
        is_admin: false,
    };
    let model = OutputModel::new(&actor, Uuid::now_v7(), true);
    let (sink, mut receiver, events) = channel(&config, None);
    let (_tx, cancel) = watch::channel(false);
    for (mode, count) in [(WebMode::Fast, 1), (WebMode::Medium, 3), (WebMode::Deep, 4)] {
        let result = service
            .rewrite(
                QueryInput {
                    mode,
                    question: "user question",
                    model: &model,
                    images: vec!["image-fixture".into()],
                },
                &sink,
                cancel.clone(),
            )
            .await;
        assert_eq!(result.queries.len(), count);
        assert!(result.error.is_none());
        assert_eq!(result.call.is_some(), mode != WebMode::Fast);
        if let Some(call) = result.call {
            assert_eq!(call.input_tokens, 12);
            assert_eq!(call.output_tokens, 6);
            assert_eq!(call.model, config.vision_model);
        }
    }
    assert_eq!(driver.calls.load(Ordering::SeqCst), 2);
    drop(sink);
    events.await.unwrap();
    let mut query_steps = 0;
    while let Some(event) = receiver.recv().await {
        assert!(
            !event
                .event
                .data()
                .to_string()
                .contains("private query tokens")
        );
        if let NativeEvent::Step(step) = event.event
            && step.step == cortex_api::domain::models::events::StepName::WebQueries
        {
            query_steps += 1;
        }
    }
    assert_eq!(query_steps, 2);
}
#[tokio::test]
async fn errors_keep_attempt_usage_and_cancel_before_call() {
    let config = Config::default();
    let actor = Caller {
        app_id: "cortex".into(),
        user_id: Some("fixture".into()),
        is_admin: false,
    };
    let model = OutputModel::new(&actor, Uuid::now_v7(), false);
    for (before, after) in [(true, false), (false, true)] {
        let driver = Arc::new(Driver {
            calls: AtomicUsize::new(0),
            before,
            after,
        });
        let service = QueryRewriter::new(&config, driver.clone());
        let (sink, _rx, events) = channel(&config, None);
        let (_tx, cancel) = watch::channel(false);
        let result = service
            .rewrite(
                QueryInput {
                    mode: WebMode::Medium,
                    question: "user question",
                    model: &model,
                    images: vec![],
                },
                &sink,
                cancel,
            )
            .await;
        assert!(result.queries.is_empty());
        assert!(result.error.is_some());
        assert_eq!(result.call.is_some(), after);
        let (_tx, cancel) = watch::channel(true);
        let result = service
            .rewrite(
                QueryInput {
                    mode: WebMode::Deep,
                    question: "user question",
                    model: &model,
                    images: vec![],
                },
                &sink,
                cancel,
            )
            .await;
        assert_eq!(result.error, Some(DriverError::Cancelled));
        assert_eq!(driver.calls.load(Ordering::SeqCst), 1);
        drop(sink);
        events.await.unwrap();
    }
}

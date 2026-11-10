use cortex_api::{
    core::{config::Config, process_numbers},
    domain::{
        models::{
            context::{ContextInput, ContextText},
            driver::*,
            events::{NativeEvent, StepName},
            identity::Caller,
            run_record::ModelCall,
            thinking::ThinkingInput,
            usage::Pending,
        },
        services::{context::assemble, events::channel, thinking::Thinking},
    },
};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};
use tokio::sync::{Semaphore, mpsc, watch};
use uuid::Uuid;
struct Double {
    calls: Mutex<Vec<CallInput>>,
    checks: AtomicUsize,
    claims: bool,
    slow: bool,
    entered: Semaphore,
}
impl ModelDriver for Double {
    fn call<'a>(
        &'a self,
        output: &'a OutputModel,
        input: CallInput,
        tx: mpsc::UnboundedSender<DriverEvent>,
        mut cancel: watch::Receiver<bool>,
    ) -> Pending<'a, Result<CallOutcome, DriverError>> {
        Box::pin(async move {
            let role = input.role.clone();
            self.calls.lock().unwrap().push(input);
            let text = match role.as_str() {
                "plan" => "step one\nstep two\nstep three\nstep four\nEXTRA_PLAN".into(),
                "work" => "work notes ".repeat(400),
                "check" => {
                    self.checks.fetch_add(1, Ordering::SeqCst);
                    if self.claims {
                        "claim has no support".into()
                    } else {
                        "NONE".into()
                    }
                }
                _ => "Final answer".into(),
            };
            let _ = tx.send(DriverEvent::Waiting);
            let _ = tx.send(DriverEvent::Token {
                role: role.clone(),
                text: text.clone(),
            });
            self.entered.add_permits(1);
            let stopped = if self.slow && role != "answer" {
                while !*cancel.borrow() {
                    if cancel.changed().await.is_err() {
                        break;
                    }
                }
                true
            } else {
                false
            };
            let now = chrono::Utc::now();
            Ok(CallOutcome {
                text,
                usage: ModelCall {
                    images: Some(u32::from(output.is_vision())),
                    audio: None,
                    role,
                    model: output.name(&Config::default()).into(),
                    input_tokens: 30,
                    output_tokens: 20,
                    time_to_first_token_ms: Some(1),
                    total_ms: 2,
                    started_at: now,
                    finished_at: now,
                },
                error: stopped.then_some(DriverError::Cancelled),
                load_ms: 0,
                tokens_per_second: None,
            })
        })
    }
}
fn double(claims: bool, slow: bool) -> Arc<Double> {
    Arc::new(Double {
        calls: Mutex::new(vec![]),
        checks: AtomicUsize::new(0),
        claims,
        slow,
        entered: Semaphore::new(0),
    })
}
fn model() -> OutputModel {
    OutputModel::new(
        &Caller {
            app_id: "cortex".into(),
            user_id: Some("fixture".into()),
            is_admin: false,
        },
        Uuid::now_v7(),
        true,
    )
}
fn context(config: &Config) -> cortex_api::domain::models::context::AssembledContext {
    assemble(
        config,
        ContextInput {
            question: "Test question".into(),
            thinking: true,
            memories: vec![ContextText {
                label: "memory".into(),
                source: Some("m".into()),
                text: "SOURCE_MARKER".into(),
            }],
            ..Default::default()
        },
    )
    .unwrap()
}
#[tokio::test]
async fn none_path_and_second_round_cap_stream_every_role_with_one_model() {
    for (claims, total) in [(false, 4), (true, 6)] {
        let config = Config::default();
        let driver = double(claims, false);
        let service = Thinking::new(&config, driver.clone());
        let context = context(&config);
        let model = model();
        let input = ThinkingInput {
            model: &model,
            context: &context,
            images: vec!["fixture image".into()],
        };
        let (sink, mut rx, events) = channel(&config, None);
        let (_stop, stop) = watch::channel(false);
        let (_cancel, cancel) = watch::channel(false);
        let result = service.think(&input, &sink, stop, cancel.clone()).await;
        assert!(result.error.is_none() && result.notice.is_none());
        assert_eq!(result.calls.len(), total - 1);
        assert!(!result.notes[0].text.contains("EXTRA_PLAN"));
        assert!(result.text.contains("EXTRA_PLAN"));
        let answer = service
            .answer(&input, &result, &sink, cancel)
            .await
            .unwrap();
        assert_eq!(answer.text, "Final answer");
        {
            let calls = driver.calls.lock().unwrap();
            assert_eq!(calls.len(), total);
            assert_eq!(
                calls.iter().map(|c| c.role.as_str()).collect::<Vec<_>>(),
                if claims {
                    vec!["plan", "work", "check", "work", "check", "answer"]
                } else {
                    vec!["plan", "work", "check", "answer"]
                }
            );
            for call in calls.iter() {
                assert_eq!(call.images, ["fixture image"]);
                assert!(call.prompt.contains("SOURCE_MARKER"));
                assert_eq!(call.prompt.matches("Test question").count(), 2);
                assert!(call.prompt.ends_with("Question: Test question"));
                assert!(
                    config.estimate_tokens(&call.system)
                        + config.estimate_tokens(&call.prompt)
                        + config.chat_template_token_reserve
                        + u64::from(call.max_tokens)
                        <= 8192
                );
                assert_eq!(
                    call.max_tokens,
                    if call.role == "answer" { 1024 } else { 512 }
                );
            }
            assert!(!calls[1].prompt.contains("EXTRA_PLAN"));
        }
        drop(sink);
        events.await.unwrap();
        let mut thinking = 0;
        let mut tokens = 0;
        while let Ok(event) = rx.try_recv() {
            match event.event {
                NativeEvent::Thinking { .. } => thinking += 1,
                NativeEvent::Token(text) => {
                    tokens += 1;
                    assert_eq!(text, "Final answer");
                }
                _ => {}
            }
        }
        assert_eq!(thinking, total - 1);
        assert_eq!(tokens, 1);
    }
}
#[tokio::test(start_paused = true)]
async fn full_180_second_limit_is_a_notice_and_keeps_attempt_usage() {
    let config = Config::default();
    assert_eq!(config.thinking_timeout, std::time::Duration::from_secs(180));
    let driver = double(false, true);
    let service = Thinking::new(&config, driver);
    let context = context(&config);
    let model = model();
    let input = ThinkingInput {
        model: &model,
        context: &context,
        images: vec![],
    };
    let (sink, _rx, events) = channel(&config, None);
    let (_stop, stop) = watch::channel(false);
    let (_cancel, cancel) = watch::channel(false);
    let start = tokio::time::Instant::now();
    let result = service.think(&input, &sink, stop, cancel.clone()).await;
    assert_eq!(start.elapsed(), std::time::Duration::from_secs(180));
    assert_eq!(
        result.notice.unwrap().number,
        process_numbers::THINKING_TIME_LIMIT.number
    );
    assert!(result.error.is_none());
    assert_eq!(result.calls.len(), 1);
    assert!(!result.text.is_empty());
    assert!(service.answer(&input, &result, &sink, cancel).await.is_ok());
    drop(sink);
    events.await.unwrap();
}
#[tokio::test]
async fn stop_thinking_keeps_notes_and_continues_but_cancel_stops_output() {
    for whole in [false, true] {
        let config = Config::default();
        let driver = double(false, true);
        let service = Thinking::new(&config, driver.clone());
        let context = context(&config);
        let model = model();
        let input = ThinkingInput {
            model: &model,
            context: &context,
            images: vec![],
        };
        let (sink, mut rx, events) = channel(&config, None);
        let (stop_tx, stop) = watch::channel(false);
        let (cancel_tx, cancel) = watch::channel(false);
        let result = {
            let thinking = service.think(&input, &sink, stop, cancel.clone());
            tokio::pin!(thinking);
            tokio::select! {result=&mut thinking=>result,_=driver.entered.acquire()=>{if whole {cancel_tx.send(true).unwrap();}else{stop_tx.send(true).unwrap();}thinking.await}}
        };
        assert_eq!(result.calls.len(), 1);
        assert!(!result.text.is_empty());
        let answer = service.answer(&input, &result, &sink, cancel).await;
        if whole {
            assert_eq!(result.error, Some(DriverError::Cancelled));
            assert_eq!(answer.err(), Some(DriverError::Cancelled));
            assert_eq!(driver.calls.lock().unwrap().len(), 1);
        } else {
            assert!(result.error.is_none() && answer.is_ok());
            assert_eq!(driver.calls.lock().unwrap().len(), 2);
        }
        drop(sink);
        events.await.unwrap();
        let mut stopped = false;
        while let Ok(event) = rx.try_recv() {
            if let NativeEvent::Step(step) = event.event {
                stopped |= step.step == StepName::ThinkingStopped;
            }
        }
        assert_eq!(stopped, !whole);
    }
}

use chrono::{DateTime, Utc};
use cortex_api::{
    core::process_numbers,
    domain::{models::usage::*, services::balancer::Scheduler},
};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};
use uuid::Uuid;
struct Ledger {
    shares: HashMap<String, (u64, f64)>,
    down: bool,
    records: Mutex<Vec<CallUsage>>,
}
impl UsageLedger for Ledger {
    fn share<'a>(
        &'a self,
        actor: &'a str,
        _: DateTime<Utc>,
    ) -> Pending<'a, Result<Share, LedgerUnavailable>> {
        Box::pin(async move {
            if self.down {
                return Err(LedgerUnavailable);
            }
            let (input, weight) = self.shares.get(actor).copied().unwrap_or((0, 1.0));
            let recorded = self
                .records
                .lock()
                .unwrap()
                .iter()
                .filter(|c| c.actor == actor)
                .map(|c| c.usage.total())
                .sum::<u64>();
            Ok(Share {
                usage: Usage {
                    input: input + recorded,
                    ..Usage::default()
                },
                weight,
                loyalty: None,
            })
        })
    }
    fn record<'a>(&'a self, c: CallUsage) -> Pending<'a, Result<(), LedgerUnavailable>> {
        Box::pin(async move {
            self.records.lock().unwrap().push(c);
            Ok(())
        })
    }
    fn waiting<'a>(&'a self, _: Vec<WaitingCall>) -> Pending<'a, Result<(), LedgerUnavailable>> {
        Box::pin(async move {
            if self.down {
                Err(LedgerUnavailable)
            } else {
                Ok(())
            }
        })
    }
}
fn ledger(down: bool) -> Arc<Ledger> {
    Arc::new(Ledger {
        shares: HashMap::from([
            ("a".into(), (300, 1.0)),
            ("b".into(), (100, 1.0)),
            ("c".into(), (200, 3.0)),
        ]),
        down,
        records: Default::default(),
    })
}
async fn queued(s: &Scheduler, n: usize) {
    for _ in 0..100 {
        if s.waiting_count() == n {
            return;
        }
        tokio::task::yield_now().await;
    }
    panic!("The expected calls were not queued");
}
#[tokio::test]
async fn three_users_lowest_ratio_then_fifo_ties_no_overlap() {
    let data = ledger(false);
    let scheduler = Scheduler::new(data.clone());
    let active = scheduler
        .acquire("first".into(), Uuid::new_v4())
        .await
        .unwrap();
    let a = {
        let s = scheduler.clone();
        tokio::spawn(async move { s.acquire("a".into(), Uuid::new_v4()).await })
    };
    queued(&scheduler, 1).await;
    let b = {
        let s = scheduler.clone();
        tokio::spawn(async move { s.acquire("b".into(), Uuid::new_v4()).await })
    };
    queued(&scheduler, 2).await;
    let c = {
        let s = scheduler.clone();
        tokio::spawn(async move { s.acquire("c".into(), Uuid::new_v4()).await })
    };
    queued(&scheduler, 3).await;
    assert!(!a.is_finished() && !b.is_finished() && !c.is_finished());
    drop(active);
    let c = c.await.unwrap().unwrap();
    assert!(!a.is_finished() && !b.is_finished());
    c.finish(
        Usage {
            input: 5,
            output: 3,
            images: 1,
            audio: 0,
        },
        Utc::now(),
    )
    .await
    .unwrap();
    let b = b.await.unwrap().unwrap();
    assert!(!a.is_finished());
    drop(b);
    drop(a.await.unwrap().unwrap());
    let records = data.records.lock().unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].actor, "c");
    assert_eq!(records[0].usage.total(), 8);
}
#[tokio::test]
async fn redis_down_serves_arrival_order() {
    let scheduler = Scheduler::new(ledger(true));
    let active = scheduler
        .acquire("first".into(), Uuid::new_v4())
        .await
        .unwrap();
    let a = {
        let s = scheduler.clone();
        tokio::spawn(async move { s.acquire("a".into(), Uuid::new_v4()).await })
    };
    queued(&scheduler, 1).await;
    let c = {
        let s = scheduler.clone();
        tokio::spawn(async move { s.acquire("c".into(), Uuid::new_v4()).await })
    };
    queued(&scheduler, 2).await;
    drop(active);
    let a = a.await.unwrap().unwrap();
    assert!(!c.is_finished());
    drop(a);
    drop(c.await.unwrap().unwrap());
}
#[tokio::test(start_paused = true)]
async fn five_minute_deadline_has_its_own_error_number() {
    use axum::{body::to_bytes, http::StatusCode, response::IntoResponse};
    let scheduler = Scheduler::new(ledger(false));
    let active = scheduler
        .acquire("first".into(), Uuid::new_v4())
        .await
        .unwrap();
    let waiting = {
        let s = scheduler.clone();
        tokio::spawn(async move { s.acquire("a".into(), Uuid::new_v4()).await })
    };
    queued(&scheduler, 1).await;
    tokio::time::advance(std::time::Duration::from_secs(300)).await;
    let error = match waiting.await.unwrap() {
        Err(error) => error,
        Ok(_) => panic!("The call must time out"),
    };
    assert_eq!(error, QueueError::HeavyLoad);
    let response = error.into_response();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body: serde_json::Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
    assert_eq!(
        body["data"]["error_number"],
        process_numbers::HEAVY_LOAD.number
    );
    assert_ne!(
        process_numbers::HEAVY_LOAD.number,
        process_numbers::QUEUE_FAULT.number
    );
    drop(active);
}
#[tokio::test]
async fn cancelled_waiter_cannot_hold_the_slot() {
    let scheduler = Scheduler::new(ledger(false));
    let active = scheduler
        .acquire("first".into(), Uuid::new_v4())
        .await
        .unwrap();
    let waiting = {
        let s = scheduler.clone();
        tokio::spawn(async move { s.acquire("a".into(), Uuid::new_v4()).await })
    };
    queued(&scheduler, 1).await;
    waiting.abort();
    let _ = waiting.await;
    drop(active);
    let permit = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        scheduler.acquire("b".into(), Uuid::new_v4()),
    )
    .await
    .unwrap()
    .unwrap();
    drop(permit);
}

#[tokio::test]
async fn t1_token_then_queued_t0_waits_for_cancel_teardown() {
    use tokio::sync::{oneshot, watch};

    // CPU-only Scheduler contract fixture: the token and teardown handshakes
    // are simulated, so this does not replace the ModelRouter/runtime test.
    let scheduler = Scheduler::new(ledger(false));
    let (cancel_tx, cancel_rx) = watch::channel(false);
    let (token_tx, token_rx) = oneshot::channel();
    let (teardown_started_tx, teardown_started_rx) = oneshot::channel();
    let (teardown_done_tx, teardown_done_rx) = oneshot::channel();
    let t1_scheduler = scheduler.clone();
    let t1 = tokio::spawn(async move {
        let permit = t1_scheduler
            .acquire("bonsai-t1".into(), Uuid::new_v4())
            .await
            .expect("T1 obtains the shared permit");
        token_tx.send(()).expect("test observes the first T1 token");
        let mut cancel = cancel_rx;
        cancel.changed().await.expect("T1 cancellation signal");
        teardown_started_tx
            .send(())
            .expect("test observes teardown while T1 still owns the permit");
        teardown_done_rx
            .await
            .expect("test explicitly releases the completed teardown");
        drop(permit);
    });

    tokio::time::timeout(std::time::Duration::from_secs(1), token_rx)
        .await
        .expect("bounded first-token handshake")
        .expect("T1 token handshake sender");
    let t0_scheduler = scheduler.clone();
    let t0 = tokio::spawn(async move {
        t0_scheduler
            .acquire("ollama-t0".into(), Uuid::new_v4())
            .await
    });
    queued(&scheduler, 1).await;
    assert!(
        !t0.is_finished(),
        "T0 remains queued while T1 owns the permit"
    );

    cancel_tx.send(true).expect("send T1 cancellation");
    tokio::time::timeout(std::time::Duration::from_secs(1), teardown_started_rx)
        .await
        .expect("bounded T1 teardown handshake")
        .expect("T1 teardown started");
    assert!(
        !t0.is_finished(),
        "T0 cannot start while the cancelled T1 is still tearing down"
    );

    teardown_done_tx.send(()).expect("complete T1 teardown");
    let t0_permit = tokio::time::timeout(std::time::Duration::from_secs(1), t0)
        .await
        .expect("T0 starts after T1 releases its permit")
        .expect("T0 task")
        .expect("T0 queue permit");
    drop(t0_permit);
    tokio::time::timeout(std::time::Duration::from_secs(1), t1)
        .await
        .expect("T1 teardown task settles")
        .expect("T1 teardown task");
}

#[tokio::test]
async fn polling_t0_before_created_t1_controls_scheduler_arrival_order() {
    use std::{future::Future, task::Poll, time::Duration};

    let data = ledger(false);
    let scheduler = Scheduler::new(data.clone());
    let active = scheduler
        .acquire("holder".into(), Uuid::new_v4())
        .await
        .expect("reserve the scheduler slot");

    // Construct T1 first, then T0, but do not poll either future yet. Merely
    // creating the intended T1 future does not put it in the scheduler queue.
    let mut t1 = Box::pin(scheduler.acquire("bonsai-t1".into(), Uuid::new_v4()));
    let mut t0 = Box::pin(scheduler.acquire("ollama-t0".into(), Uuid::new_v4()));

    std::future::poll_fn(|cx| {
        assert!(t0.as_mut().poll(cx).is_pending());
        Poll::Ready(())
    })
    .await;
    queued(&scheduler, 1).await;
    std::future::poll_fn(|cx| {
        assert!(t1.as_mut().poll(cx).is_pending());
        Poll::Ready(())
    })
    .await;
    queued(&scheduler, 2).await;

    drop(active);
    let t0_permit = tokio::time::timeout(Duration::from_secs(1), t0.as_mut())
        .await
        .expect("first-poll T0 gets the first ticket")
        .expect("T0 permit");
    t0_permit
        .finish(
            Usage {
                input: 1,
                ..Usage::default()
            },
            Utc::now(),
        )
        .await
        .expect("record T0 completion");
    let t1_permit = tokio::time::timeout(Duration::from_secs(1), t1.as_mut())
        .await
        .expect("T1 proceeds after T0 releases the slot")
        .expect("T1 permit");
    t1_permit
        .finish(
            Usage {
                input: 1,
                ..Usage::default()
            },
            Utc::now(),
        )
        .await
        .expect("record T1 completion");

    let records = data.records.lock().unwrap();
    assert_eq!(records.len(), 2);
    assert_eq!(records[0].actor, "ollama-t0");
    assert_eq!(records[1].actor, "bonsai-t1");
}

#[tokio::test]
async fn a_long_output_rejoins_the_queue_between_calls() {
    let data = ledger(false);
    let scheduler = Scheduler::new(data);
    let active = scheduler
        .acquire("long".into(), Uuid::new_v4())
        .await
        .unwrap();
    let other = {
        let s = scheduler.clone();
        tokio::spawn(async move { s.acquire("other".into(), Uuid::new_v4()).await })
    };
    queued(&scheduler, 1).await;
    active
        .finish(
            Usage {
                input: 100,
                ..Usage::default()
            },
            Utc::now(),
        )
        .await
        .unwrap();
    let long = {
        let s = scheduler.clone();
        tokio::spawn(async move { s.acquire("long".into(), Uuid::new_v4()).await })
    };
    let other = other.await.unwrap().unwrap();
    assert!(!long.is_finished());
    drop(other);
    drop(long.await.unwrap().unwrap());
}

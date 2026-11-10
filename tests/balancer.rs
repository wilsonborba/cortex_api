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

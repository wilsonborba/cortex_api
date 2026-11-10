use crate::{core::config::Config, domain::models::usage::*};
use chrono::{DateTime, Utc};
use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::{Notify, oneshot};
struct Request {
    ticket: u64,
    actor: String,
    output_id: uuid::Uuid,
    sender: oneshot::Sender<Permit>,
}
#[derive(Default)]
struct State {
    next: u64,
    active: Option<u64>,
    waiting: VecDeque<Request>,
}
struct Inner {
    state: Mutex<State>,
    notify: Notify,
    ledger: Arc<dyn UsageLedger>,
}
struct Worker(tokio::task::AbortHandle);
impl Drop for Worker {
    fn drop(&mut self) {
        self.0.abort();
    }
}
#[derive(Clone)]
pub struct Scheduler {
    inner: Arc<Inner>,
    _worker: Arc<Worker>,
    timeout: Duration,
}
pub struct Permit {
    inner: Arc<Inner>,
    ticket: u64,
    actor: String,
    output_id: uuid::Uuid,
}
impl Drop for Permit {
    fn drop(&mut self) {
        let mut state = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.active == Some(self.ticket) {
            state.active = None;
        }
        drop(state);
        self.inner.notify.notify_one();
    }
}
impl Permit {
    pub async fn finish(
        self,
        usage: Usage,
        finished_at: DateTime<Utc>,
    ) -> Result<(), LedgerUnavailable> {
        self.inner
            .ledger
            .record(CallUsage {
                actor: self.actor.clone(),
                output_id: self.output_id,
                finished_at,
                usage,
            })
            .await
    }
}
impl Scheduler {
    pub fn new(ledger: Arc<dyn UsageLedger>) -> Self {
        Self::with_timeout(ledger, Config::default().queue_wait_timeout)
    }
    pub fn with_timeout(ledger: Arc<dyn UsageLedger>, timeout: Duration) -> Self {
        let inner = Arc::new(Inner {
            state: Default::default(),
            notify: Notify::new(),
            ledger,
        });
        let worker = tokio::spawn(dispatch(inner.clone()));
        Self {
            inner,
            _worker: Arc::new(Worker(worker.abort_handle())),
            timeout,
        }
    }
    pub async fn acquire(
        &self,
        actor: String,
        output_id: uuid::Uuid,
    ) -> Result<Permit, QueueError> {
        let (sender, receiver) = oneshot::channel();
        let ticket = {
            let mut state = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
            state.next = state.next.checked_add(1).ok_or(QueueError::ServerFault)?;
            let ticket = state.next;
            state.waiting.push_back(Request {
                ticket,
                actor,
                output_id,
                sender,
            });
            ticket
        };
        self.inner.notify.notify_one();
        match tokio::time::timeout(self.timeout, receiver).await {
            Ok(Ok(permit)) => Ok(permit),
            Ok(Err(_)) => Err(QueueError::ServerFault),
            Err(_) => {
                self.inner
                    .state
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .waiting
                    .retain(|r| r.ticket != ticket);
                self.inner.notify.notify_one();
                Err(QueueError::HeavyLoad)
            }
        }
    }
    pub fn waiting_count(&self) -> usize {
        self.inner
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .waiting
            .iter()
            .filter(|r| !r.sender.is_closed())
            .count()
    }
}
async fn dispatch(inner: Arc<Inner>) {
    loop {
        inner.notify.notified().await;
        let (snapshot, active) = {
            let mut state = inner.state.lock().unwrap_or_else(|e| e.into_inner());
            state.waiting.retain(|r| !r.sender.is_closed());
            (
                state
                    .waiting
                    .iter()
                    .map(|r| WaitingCall {
                        ticket: r.ticket,
                        actor: r.actor.clone(),
                    })
                    .collect::<Vec<_>>(),
                state.active.is_some(),
            )
        };
        let cache_up = inner.ledger.waiting(snapshot.clone()).await.is_ok();
        if active || snapshot.is_empty() {
            continue;
        }
        let mut scores = HashMap::new();
        let mut fair = cache_up;
        for request in &snapshot {
            if !fair {
                break;
            }
            if !scores.contains_key(&request.actor) {
                match inner.ledger.share(&request.actor, Utc::now()).await {
                    Ok(share) => {
                        let weight = if share.weight.is_finite() && share.weight > 0.0 {
                            share.weight
                        } else {
                            1.0
                        };
                        scores.insert(request.actor.clone(), share.usage.total() as f64 / weight);
                    }
                    Err(_) => {
                        fair = false;
                        break;
                    }
                }
            }
        }
        let chosen = if fair {
            snapshot
                .iter()
                .min_by(|a, b| {
                    scores[&a.actor]
                        .total_cmp(&scores[&b.actor])
                        .then(a.ticket.cmp(&b.ticket))
                })
                .expect("nonempty snapshot")
                .ticket
        } else {
            snapshot[0].ticket
        };
        let request = {
            let mut state = inner.state.lock().unwrap_or_else(|e| e.into_inner());
            state.waiting.retain(|r| !r.sender.is_closed());
            if state.active.is_some() {
                continue;
            }
            let Some(index) = state.waiting.iter().position(|r| r.ticket == chosen) else {
                inner.notify.notify_one();
                continue;
            };
            let request = state.waiting.remove(index).expect("pending request");
            state.active = Some(request.ticket);
            request
        };
        let permit = Permit {
            inner: inner.clone(),
            ticket: request.ticket,
            actor: request.actor,
            output_id: request.output_id,
        };
        let _ = request.sender.send(permit);
        inner.notify.notify_one();
    }
}

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::{future::Future, pin::Pin};
use uuid::Uuid;
#[derive(Debug, Default, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct Usage {
    pub input: u64,
    pub output: u64,
    pub images: u64,
    pub audio: u64,
}
impl Usage {
    pub fn total(&self) -> u64 {
        self.input.saturating_add(self.output)
    }
    pub fn add(&mut self, other: Self) {
        self.input = self.input.saturating_add(other.input);
        self.output = self.output.saturating_add(other.output);
        self.images = self.images.saturating_add(other.images);
        self.audio = self.audio.saturating_add(other.audio);
    }
}
#[derive(Debug, Clone, Copy)]
pub struct Share {
    pub usage: Usage,
    pub weight: f64,
}
#[derive(Debug, Clone)]
pub struct CallUsage {
    pub actor: String,
    pub output_id: Uuid,
    pub finished_at: DateTime<Utc>,
    pub usage: Usage,
}
#[derive(Debug, Clone, Serialize)]
pub struct WaitingCall {
    pub ticket: u64,
    pub actor: String,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LedgerUnavailable;
pub type Pending<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;
pub trait UsageLedger: Send + Sync {
    fn share<'a>(
        &'a self,
        actor: &'a str,
        now: DateTime<Utc>,
    ) -> Pending<'a, Result<Share, LedgerUnavailable>>;
    fn record<'a>(&'a self, call: CallUsage) -> Pending<'a, Result<(), LedgerUnavailable>>;
    fn waiting<'a>(&'a self, calls: Vec<WaitingCall>)
    -> Pending<'a, Result<(), LedgerUnavailable>>;
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueueError {
    HeavyLoad,
    ServerFault,
}

pub trait UsageSource: Send + Sync {
    fn records_since<'a>(
        &'a self,
        since: DateTime<Utc>,
    ) -> Pending<'a, Result<Vec<super::run_record::RunRecord>, LedgerUnavailable>>;
}

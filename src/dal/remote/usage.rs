use crate::{
    core::config::Config,
    dal::remote::{cache::RedisCache, runs::RunStore},
    domain::models::{
        cache::{CacheKey, Lifetime},
        usage::*,
    },
};
use chrono::{DateTime, Utc};
use serde_json::json;
use std::{
    collections::{BTreeMap, HashSet},
    sync::{Arc, Mutex},
    time::Duration,
};
impl UsageSource for RunStore {
    fn records_since<'a>(
        &'a self,
        since: DateTime<Utc>,
    ) -> Pending<'a, Result<Vec<crate::domain::models::run_record::RunRecord>, LedgerUnavailable>>
    {
        Box::pin(async move {
            self.records_since(since)
                .await
                .map_err(|_| LedgerUnavailable)
        })
    }
}
pub struct RedisLedger {
    cache: RedisCache,
    source: Arc<dyn UsageSource>,
    queue: String,
    repair: tokio::sync::Mutex<()>,
    recent: Mutex<Vec<CallUsage>>,
    dirty: Mutex<HashSet<String>>,
    config: Config,
}
impl RedisLedger {
    pub fn new(cache: RedisCache, source: Arc<dyn UsageSource>, queue: String) -> Self {
        Self {
            cache,
            source,
            queue,
            repair: Default::default(),
            recent: Default::default(),
            dirty: Default::default(),
            config: Config::default(),
        }
    }
    fn lifetime(&self, hour: i64, now: DateTime<Utc>) -> Result<Lifetime, LedgerUnavailable> {
        let end = hour * 3600 + self.config.usage_bucket_ttl.as_secs() as i64;
        Lifetime::new(Duration::from_secs((end - now.timestamp()).max(1) as u64))
            .map_err(|_| LedgerUnavailable)
    }
    fn hours(&self, now: DateTime<Utc>) -> std::ops::RangeInclusive<i64> {
        let hour = now.timestamp().div_euclid(3600);
        hour - self.config.usage_window_hours + 1..=hour
    }
    pub fn bucket_keys(&self, actor: &str, now: DateTime<Utc>) -> Vec<CacheKey> {
        self.hours(now)
            .map(|hour| CacheKey::Usage {
                user: actor.into(),
                hour: hour.to_string(),
            })
            .collect()
    }
    async fn read_share(
        &self,
        actor: &str,
        now: DateTime<Utc>,
    ) -> Result<Share, LedgerUnavailable> {
        let _guard = self.repair.lock().await;
        self.read_locked(actor, now).await.map(|(share, _)| share)
    }
    async fn read_locked(
        &self,
        actor: &str,
        now: DateTime<Utc>,
    ) -> Result<(Share, bool), LedgerUnavailable> {
        let keys = self.bucket_keys(actor, now);
        let values = self
            .cache
            .get_many(&keys)
            .await
            .map_err(|_| LedgerUnavailable)?;
        let mut buckets: BTreeMap<i64, Usage> = BTreeMap::new();
        let mut missing = vec![];
        for (hour, value) in self.hours(now).zip(values) {
            if let Some(value) = value {
                buckets.insert(
                    hour,
                    serde_json::from_value(value).map_err(|_| LedgerUnavailable)?,
                );
            } else {
                missing.push(hour);
            }
        }
        let repaired = !missing.is_empty()
            || self
                .dirty
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .contains(actor);
        if repaired {
            let first = *self.hours(now).start();
            let since = DateTime::from_timestamp(first * 3600, 0).ok_or(LedgerUnavailable)?;
            let records = self.source.records_since(since).await?;
            let mut rebuilt: BTreeMap<i64, Usage> =
                self.hours(now).map(|h| (h, Usage::default())).collect();
            let mut saved = HashSet::new();
            for record in records {
                let owner = if record.user_id.is_empty() {
                    format!("app:{}", record.app_id)
                } else {
                    record.user_id
                };
                if owner != actor {
                    continue;
                }
                saved.insert(record.output_id);
                for call in record.calls {
                    if let Some(bucket) =
                        rebuilt.get_mut(&call.finished_at.timestamp().div_euclid(3600))
                    {
                        bucket.add(Usage {
                            input: call.input_tokens,
                            output: call.output_tokens,
                            images: u64::from(call.images.unwrap_or(0)),
                            audio: u64::from(call.audio.unwrap_or(0)),
                        });
                    }
                }
            }
            {
                let mut recent = self.recent.lock().unwrap_or_else(|e| e.into_inner());
                recent.retain(|call| call.finished_at >= since);
                for call in recent
                    .iter()
                    .filter(|c| c.actor == actor && !saved.contains(&c.output_id))
                {
                    if let Some(bucket) =
                        rebuilt.get_mut(&call.finished_at.timestamp().div_euclid(3600))
                    {
                        bucket.add(call.usage);
                    }
                }
            }
            for hour in self.hours(now) {
                let value = rebuilt[&hour];
                self.cache
                    .put(
                        &CacheKey::Usage {
                            user: actor.into(),
                            hour: hour.to_string(),
                        },
                        &json!(value),
                        self.lifetime(hour, now)?,
                    )
                    .await
                    .map_err(|_| LedgerUnavailable)?;
                buckets.insert(hour, value);
            }
        }
        let mut usage = Usage::default();
        for bucket in buckets.into_values() {
            usage.add(bucket);
        }
        let loyalty = self
            .cache
            .get(&CacheKey::Loyalty(actor.into()))
            .await
            .map_err(|_| LedgerUnavailable)?
            .and_then(|value| {
                serde_json::from_value::<crate::domain::models::loyalty::LoyaltySnapshot>(value)
                    .ok()
            })
            .filter(|snapshot| snapshot.valid());
        if repaired {
            self.dirty
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(actor);
        }
        Ok((
            Share {
                usage,
                weight: loyalty.map_or(1.0, |s| s.weight),
                loyalty,
            },
            repaired,
        ))
    }
}
impl UsageLedger for RedisLedger {
    fn share<'a>(
        &'a self,
        actor: &'a str,
        now: DateTime<Utc>,
    ) -> Pending<'a, Result<Share, LedgerUnavailable>> {
        Box::pin(self.read_share(actor, now))
    }
    fn record<'a>(&'a self, call: CallUsage) -> Pending<'a, Result<(), LedgerUnavailable>> {
        Box::pin(async move {
            let _guard = self.repair.lock().await;
            let hour = call.finished_at.timestamp().div_euclid(3600);
            {
                let mut recent = self.recent.lock().unwrap_or_else(|e| e.into_inner());
                let since =
                    DateTime::from_timestamp(*self.hours(call.finished_at).start() * 3600, 0)
                        .ok_or(LedgerUnavailable)?;
                recent.retain(|r| r.finished_at >= since);
                recent.push(call.clone());
            }
            let result = async {
                let (_, rebuilt) = self.read_locked(&call.actor, call.finished_at).await?;
                if rebuilt {
                    return Ok(());
                }
                self.cache
                    .add_usage(
                        &call.actor,
                        hour,
                        call.usage,
                        self.lifetime(hour, call.finished_at)?,
                    )
                    .await
                    .map_err(|_| LedgerUnavailable)
            }
            .await;
            if result.is_err() {
                self.dirty
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .insert(call.actor);
            }
            result
        })
    }
    fn waiting<'a>(
        &'a self,
        calls: Vec<WaitingCall>,
    ) -> Pending<'a, Result<(), LedgerUnavailable>> {
        Box::pin(async move {
            self.cache
                .put(
                    &CacheKey::WaitQueue(self.queue.clone()),
                    &json!(calls),
                    Lifetime::new(self.config.queue_snapshot_ttl).map_err(|_| LedgerUnavailable)?,
                )
                .await
                .map_err(|_| LedgerUnavailable)
        })
    }
}

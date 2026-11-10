use crate::{
    core::config::Config,
    dal::remote::cache::RedisCache,
    domain::models::{
        cache::{CacheKey, Lifetime},
        loyalty::*,
    },
};
use chrono::{DateTime, Utc};
use std::sync::Arc;
pub async fn refresh(
    source: &dyn LoyaltySource,
    cache: &RedisCache,
    config: &Config,
    now: DateTime<Utc>,
) -> Result<usize, LoyaltyUnavailable> {
    let snapshots = source.fetch(now).await?;
    if snapshots.iter().any(|(_, s)| !s.valid()) {
        return Err(LoyaltyUnavailable);
    }
    for (actor, snapshot) in &snapshots {
        cache
            .put(
                &CacheKey::Loyalty(actor.clone()),
                &serde_json::to_value(snapshot).map_err(|_| LoyaltyUnavailable)?,
                Lifetime::new(config.loyalty_snapshot_ttl).map_err(|_| LoyaltyUnavailable)?,
            )
            .await
            .map_err(|_| LoyaltyUnavailable)?;
    }
    Ok(snapshots.len())
}
pub fn spawn(
    source: Arc<dyn LoyaltySource>,
    cache: RedisCache,
    config: Config,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut timer = tokio::time::interval(config.loyalty_refresh_interval);
        timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            timer.tick().await;
            match refresh(source.as_ref(), &cache, &config, Utc::now()).await {
                Ok(count) => tracing::info!(snapshots = count, "Loyalty refresh complete"),
                Err(_) => tracing::warn!("Loyalty refresh failed; previous snapshots retained"),
            }
        }
    })
}

use crate::{
    core::config::Config,
    dal::remote::runs::{RunStore, RunStoreError},
};
use chrono::{DateTime, Utc};
use std::sync::Arc;
pub async fn purge(
    store: &RunStore,
    config: &Config,
    now: DateTime<Utc>,
) -> Result<usize, RunStoreError> {
    if config.run_retention_days <= 0 {
        return Err(RunStoreError::InvalidRecord);
    }
    store
        .purge_before(now - chrono::Duration::days(config.run_retention_days))
        .await
}
pub fn spawn(store: Arc<RunStore>, config: Config) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let period = config.run_purge_interval;
        if period.is_zero() {
            tracing::error!("Run retention interval is not valid");
            return;
        }
        let mut timer = tokio::time::interval_at(tokio::time::Instant::now() + period, period);
        timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            timer.tick().await;
            match purge(&store, &config, Utc::now()).await {
                Ok(deleted) => tracing::info!(records = deleted, "Run retention complete"),
                Err(_) => tracing::warn!("Run retention failed"),
            }
        }
    })
}

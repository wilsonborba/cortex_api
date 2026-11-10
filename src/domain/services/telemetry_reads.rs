use crate::{
    core::config::Config,
    dal::remote::{runs::RunStore, timing::TimingStore},
    domain::models::{
        history::FinishedStatus,
        run_record::{RunMode, WebMode},
        telemetry_reads::*,
    },
};
use std::sync::Arc;
use uuid::Uuid;
#[derive(Clone)]
pub struct TelemetryReads {
    pub store: Arc<RunStore>,
    pub timing: TimingStore,
    pub config: Config,
}
impl TelemetryReads {
    pub async fn completed(
        &self,
        mode: &RunMode,
        output_id: Uuid,
        total_ms: u64,
        status: FinishedStatus,
    ) {
        if matches!(status, FinishedStatus::Done) {
            let _ = self.timing.completed(mode, output_id, total_ms).await;
        }
    }
    pub async fn expected(&self, mode: &RunMode) -> Option<u64> {
        self.timing.expected(mode).await.ok().flatten()
    }
    pub async fn timing(&self) -> Vec<TimingAverage> {
        let mut values = vec![];
        for web_mode in [WebMode::Fast, WebMode::Medium, WebMode::Deep] {
            for thinking in [false, true] {
                let kind = RunMode { web_mode, thinking };
                let expected_ms = self.expected(&kind).await;
                values.push(TimingAverage { kind, expected_ms });
            }
        }
        values
    }
    pub async fn stats(&self) -> Result<Vec<GroupStats>, TelemetryReadError> {
        let value = self
            .store
            .stats()
            .await
            .map_err(|_| TelemetryReadError::Unavailable)?;
        let rows = value["rows"]
            .as_array()
            .ok_or(TelemetryReadError::Unavailable)?;
        let mut groups = vec![];
        for row in rows {
            let mode = match row["key"][0].as_str() {
                Some("fast") => WebMode::Fast,
                Some("medium") => WebMode::Medium,
                Some("deep") => WebMode::Deep,
                _ => return Err(TelemetryReadError::Unavailable),
            };
            let thinking = row["key"][1]
                .as_bool()
                .ok_or(TelemetryReadError::Unavailable)?;
            let stats = row["value"]
                .as_array()
                .filter(|s| s.len() == 5)
                .ok_or(TelemetryReadError::Unavailable)?;
            let count = stats[0]["count"]
                .as_u64()
                .filter(|c| *c > 0)
                .ok_or(TelemetryReadError::Unavailable)?;
            let average = |index: usize| -> Result<f64, TelemetryReadError> {
                let sum = stats[index]["sum"]
                    .as_f64()
                    .filter(|v| v.is_finite() && *v >= 0.0)
                    .ok_or(TelemetryReadError::Unavailable)?;
                if stats[index]["count"].as_u64() != Some(count) {
                    return Err(TelemetryReadError::Unavailable);
                }
                Ok(sum / count as f64)
            };
            let latency = average(0)?;
            groups.push(GroupStats {
                kind: RunMode {
                    web_mode: mode,
                    thinking,
                },
                total_runs: count,
                avg_latency_seconds: latency / 1000.0,
                avg_latency_ms: latency,
                avg_input_tokens: average(1)?,
                avg_output_tokens: average(2)?,
                avg_total_tokens: average(3)?,
                success_rate: average(4)?,
            });
        }
        Ok(groups)
    }
    pub async fn events(
        &self,
        user: &str,
        admin: bool,
        page: EventPage,
    ) -> Result<Vec<CallEvent>, TelemetryReadError> {
        let limit = page.limit.unwrap_or(self.config.telemetry_page_default);
        if limit == 0 || limit > self.config.telemetry_page_max {
            return Err(TelemetryReadError::InvalidCursor);
        }
        self.store
            .call_events(
                if admin { None } else { Some(user) },
                page.before.as_deref(),
                limit,
            )
            .await
    }
}

use crate::{
    dal::remote::runs::{RunStore, RunStoreError},
    domain::models::{
        pipeline::PipelineResult,
        run_evidence::RunIdentity,
        run_record::{RunCounts, RunRecord},
    },
};
use std::sync::Arc;
#[derive(Clone)]
pub struct RunEvidence {
    store: Arc<RunStore>,
}
impl RunEvidence {
    pub fn new(store: Arc<RunStore>) -> Self {
        Self { store }
    }
    /// Save orchestration uses the same path for permanent and temporary outputs.
    /// Only typed metadata is copied; stage data and source text are never read.
    pub async fn save<T>(
        &self,
        identity: RunIdentity,
        result: &PipelineResult<T>,
        counts: RunCounts,
        total_ms: u64,
    ) -> Result<(), RunStoreError> {
        let last = result.stages.last().ok_or(RunStoreError::InvalidRecord)?;
        let record = RunRecord {
            session_id: identity.session_id,
            output_id: identity.output_id,
            app_id: identity.app_id,
            user_id: identity.user_id,
            created_at: identity.created_at,
            mode: identity.mode,
            total_ms,
            stages: result.stages.iter().map(|s| s.record()).collect(),
            calls: last.output.calls.clone(),
            counts,
        };
        self.store.put(&record).await
    }
}

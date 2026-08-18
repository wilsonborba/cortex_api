from __future__ import annotations

from datetime import datetime
from typing import Any, Dict, Optional

from pydantic import BaseModel

from lib.dal.models import TelemetryEvent


class TelemetryStatsOut(BaseModel):
    strategy_id: str
    task_type: str
    tier_requested: int
    total_runs: int
    avg_latency_seconds: float
    avg_latency_ms: float
    avg_total_tokens: float
    avg_input_tokens: float
    avg_output_tokens: float
    avg_cost_usd: float
    avg_quality_score: Optional[float] = None
    success_rate: float

    @classmethod
    def from_row(cls, row: Dict[str, Any]) -> "TelemetryStatsOut":
        return cls(**row)


class TelemetryEventOut(BaseModel):
    request_id: str
    execution_id: str
    strategy_id: str
    tier_requested: int
    tier_executed: int
    task_type: str
    provider: str
    model: str
    role: str
    input_tokens: int
    output_tokens: int
    total_tokens: int
    started_at: datetime
    finished_at: datetime
    latency_ms: int
    success: bool
    error_type: Optional[str] = None
    estimated_cost_usd: float

    @classmethod
    def from_event(cls, event: TelemetryEvent) -> "TelemetryEventOut":
        return cls(
            request_id=event.request_id, execution_id=event.execution_id, strategy_id=event.strategy_id,
            tier_requested=event.tier_requested, tier_executed=event.tier_executed, task_type=event.task_type,
            provider=event.provider, model=event.model, role=event.role, input_tokens=event.input_tokens,
            output_tokens=event.output_tokens, total_tokens=event.total_tokens, started_at=event.started_at,
            finished_at=event.finished_at, latency_ms=event.latency_ms, success=event.success,
            error_type=event.error_type, estimated_cost_usd=event.estimated_cost_usd,
        )

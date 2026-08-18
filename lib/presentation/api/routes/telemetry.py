from __future__ import annotations

from typing import List, Optional

from fastapi import APIRouter, Depends

from lib.dal.repositories.telemetry_repository import TelemetryRepository
from lib.presentation.api.deps import get_telemetry_repo
from lib.presentation.api.schemas.telemetry import TelemetryEventOut, TelemetryStatsOut

router = APIRouter(prefix="/telemetry", tags=["telemetry"])


@router.get("/stats", response_model=List[TelemetryStatsOut])
def telemetry_stats(
    tier: Optional[int] = None,
    task: Optional[str] = None,
    strategy_id: Optional[str] = None,
    repo: TelemetryRepository = Depends(get_telemetry_repo),
) -> List[TelemetryStatsOut]:
    rows = repo.get_stats(tier=tier, task_type=task, strategy_id=strategy_id)
    return [TelemetryStatsOut.from_row(row) for row in rows]


@router.get("/events", response_model=List[TelemetryEventOut])
def telemetry_events(
    limit: int = 50,
    offset: int = 0,
    strategy_id: Optional[str] = None,
    task: Optional[str] = None,
    tier: Optional[int] = None,
    repo: TelemetryRepository = Depends(get_telemetry_repo),
) -> List[TelemetryEventOut]:
    events = repo.list_events(limit=limit, offset=offset, strategy_id=strategy_id, task_type=task, tier=tier)
    return [TelemetryEventOut.from_event(e) for e in events]

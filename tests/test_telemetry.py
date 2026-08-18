from __future__ import annotations

import asyncio
from datetime import datetime, timezone

import pytest
from sqlalchemy.orm import Session

from lib.dal.models import TelemetryEvent
from lib.dal.repositories.telemetry_repository import TelemetryRepository
from lib.engine.telemetry import TelemetryLogger


@pytest.fixture
def telemetry_repo_(db_session: Session) -> TelemetryRepository:
    return TelemetryRepository(session_factory=lambda: db_session)


def _event(execution_id: str) -> TelemetryEvent:
    now = datetime.now(timezone.utc)
    return TelemetryEvent(
        request_id="req-telemetry", execution_id=execution_id, strategy_id="s", tier_requested=1,
        tier_executed=1, task_type="general", provider="ollama", model="ollama/dolphin3:8b", role="primary",
        input_tokens=10, output_tokens=5, total_tokens=15, started_at=now, finished_at=now,
        latency_seconds=0.5, latency_ms=500, success=True,
    )


def test_record_sync_persists_immediately(telemetry_repo_: TelemetryRepository):
    logger = TelemetryLogger(repo=telemetry_repo_)

    logger.record_sync(_event("sync-1"))

    events = telemetry_repo_.list_events(strategy_id="s")
    assert any(e.execution_id == "sync-1" for e in events)


@pytest.mark.asyncio
async def test_record_async_persists_without_being_awaited(telemetry_repo_: TelemetryRepository):
    logger = TelemetryLogger(repo=telemetry_repo_)

    future = logger.record_async(_event("async-1"))
    assert future is not None
    await asyncio.wrap_future(future)  # deterministic wait, not a sleep-and-hope

    events = telemetry_repo_.list_events(strategy_id="s")
    assert any(e.execution_id == "async-1" for e in events)


def test_record_async_without_running_loop_falls_back_to_sync(telemetry_repo_: TelemetryRepository):
    logger = TelemetryLogger(repo=telemetry_repo_)

    result = logger.record_async(_event("no-loop-1"))  # this test function is sync: no running loop

    assert result is None
    events = telemetry_repo_.list_events(strategy_id="s")
    assert any(e.execution_id == "no-loop-1" for e in events)


def test_record_sync_swallows_repository_errors():
    class _BrokenRepo:
        def record_event(self, event):
            raise RuntimeError("db is down")

    logger = TelemetryLogger(repo=_BrokenRepo())

    logger.record_sync(_event("broken-1"))  # must not raise

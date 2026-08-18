from __future__ import annotations

import asyncio
from concurrent.futures import Future
from typing import Optional

from lib.core.logs import get_logger
from lib.dal.models import TelemetryEvent
from lib.dal.repositories.telemetry_repository import TelemetryRepository

logger = get_logger(__name__)


class TelemetryLogger:
    """Writes `TelemetryEvent`s without making the caller wait on SQLite.

    The DAL is synchronous (plain SQLAlchemy, no async driver), so "async"
    here means: the write runs on the default thread pool executor and is
    never awaited by the caller. A dropped `asyncio.create_task()` can be
    garbage-collected mid-flight; a `run_in_executor()` future backed by a
    real OS thread can't, so that's the primitive used, not a bare task.
    """

    def __init__(self, repo: Optional[TelemetryRepository] = None) -> None:
        self._repo = repo or TelemetryRepository()

    def record_sync(self, event: TelemetryEvent) -> None:
        try:
            self._repo.record_event(event)
        except Exception:
            # Telemetry must never be able to break the request it's describing.
            logger.warning("failed to persist telemetry event %s", event.execution_id, exc_info=True)

    def record_async(self, event: TelemetryEvent) -> Optional[Future]:
        """Fire-and-forget from the caller's perspective. Returns the
        `concurrent.futures.Future` doing the write so tests (or a graceful
        shutdown path) can wait on it deterministically instead of guessing
        with a sleep; production callers are expected to ignore it."""
        try:
            loop = asyncio.get_running_loop()
        except RuntimeError:
            # No running loop (e.g. a synchronous CLI path): write inline
            # rather than silently dropping the event.
            self.record_sync(event)
            return None
        return loop.run_in_executor(None, self.record_sync, event)

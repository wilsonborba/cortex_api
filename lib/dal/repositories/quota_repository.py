from __future__ import annotations

from datetime import datetime, timedelta, timezone
from typing import Any, Dict, List, Optional
from sqlalchemy import desc, func, select
from sqlalchemy.orm import Session

from lib.dal.local.database import SessionLocal, session_scope
from lib.dal.models import SlidingWindowUsage


class QuotaRepository:
    def __init__(self, session_factory=SessionLocal) -> None:
        self._session_factory = session_factory

    def record_usage(
        self,
        provider: str,
        model: str,
        input_tokens: int,
        output_tokens: int,
        timestamp: Optional[datetime] = None,
        session: Optional[Session] = None,
    ) -> SlidingWindowUsage:
        ts = timestamp or datetime.now(timezone.utc)
        usage = SlidingWindowUsage(
            provider=provider,
            model=model,
            input_tokens=input_tokens,
            output_tokens=output_tokens,
            total_tokens=input_tokens + output_tokens,
            timestamp=ts,
        )
        if session:
            session.add(usage)
            return usage
        with session_scope(self._session_factory) as s:
            s.add(usage)
            return usage

    def get_consumed_tokens_in_window(
        self,
        provider: str,
        hours: int = 5,
        reference_time: Optional[datetime] = None,
        session: Optional[Session] = None,
    ) -> int:
        now = reference_time or datetime.now(timezone.utc)
        window_start = now - timedelta(hours=hours)

        def _query(s: Session) -> int:
            stmt = select(func.sum(SlidingWindowUsage.total_tokens)).where(
                SlidingWindowUsage.provider == provider,
                SlidingWindowUsage.timestamp >= window_start,
                SlidingWindowUsage.timestamp <= now,
            )
            result = s.scalar(stmt)
            return int(result or 0)

        if session:
            return _query(session)
        with session_scope(self._session_factory) as s:
            return _query(s)

    def get_usage_summary(
        self,
        hours: int = 5,
        reference_time: Optional[datetime] = None,
        session: Optional[Session] = None,
    ) -> List[Dict[str, Any]]:
        now = reference_time or datetime.now(timezone.utc)
        window_start = now - timedelta(hours=hours)

        def _query(s: Session) -> List[Dict[str, Any]]:
            stmt = (
                select(
                    SlidingWindowUsage.provider,
                    func.sum(SlidingWindowUsage.input_tokens).label("input_tokens"),
                    func.sum(SlidingWindowUsage.output_tokens).label("output_tokens"),
                    func.sum(SlidingWindowUsage.total_tokens).label("total_tokens"),
                    func.count(SlidingWindowUsage.id).label("requests_count"),
                )
                .where(
                    SlidingWindowUsage.timestamp >= window_start,
                    SlidingWindowUsage.timestamp <= now,
                )
                .group_by(SlidingWindowUsage.provider)
            )
            rows = s.execute(stmt).all()
            return [
                {
                    "provider": r.provider,
                    "input_tokens": int(r.input_tokens or 0),
                    "output_tokens": int(r.output_tokens or 0),
                    "total_tokens": int(r.total_tokens or 0),
                    "requests_count": int(r.requests_count or 0),
                }
                for r in rows
            ]

        if session:
            return _query(session)
        with session_scope(self._session_factory) as s:
            return _query(s)

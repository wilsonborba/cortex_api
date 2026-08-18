from __future__ import annotations

from typing import Any, Dict, List, Optional
from sqlalchemy import Integer, desc, func, select
from sqlalchemy.orm import Session

from lib.dal.local.database import SessionLocal, session_scope
from lib.dal.models import TelemetryEvent


class TelemetryRepository:
    def __init__(self, session_factory=SessionLocal) -> None:
        self._session_factory = session_factory

    def record_event(self, event: TelemetryEvent, session: Optional[Session] = None) -> TelemetryEvent:
        if session:
            session.add(event)
            return event
        with session_scope(self._session_factory) as s:
            s.add(event)
            return event

    def list_events(
        self,
        limit: int = 50,
        offset: int = 0,
        strategy_id: Optional[str] = None,
        task_type: Optional[str] = None,
        tier: Optional[int] = None,
        session: Optional[Session] = None,
    ) -> List[TelemetryEvent]:
        def _query(s: Session) -> List[TelemetryEvent]:
            stmt = select(TelemetryEvent).order_by(desc(TelemetryEvent.started_at))
            if strategy_id:
                stmt = stmt.where(TelemetryEvent.strategy_id == strategy_id)
            if task_type:
                stmt = stmt.where(TelemetryEvent.task_type == task_type)
            if tier is not None:
                stmt = stmt.where(TelemetryEvent.tier_requested == tier)
            stmt = stmt.limit(limit).offset(offset)
            return list(s.scalars(stmt).all())

        if session:
            return _query(session)
        with session_scope(self._session_factory) as s:
            return _query(s)

    def get_stats(
        self,
        tier: Optional[int] = None,
        task_type: Optional[str] = None,
        strategy_id: Optional[str] = None,
        session: Optional[Session] = None,
    ) -> List[Dict[str, Any]]:
        def _calc(s: Session) -> List[Dict[str, Any]]:
            stmt = select(
                TelemetryEvent.strategy_id,
                TelemetryEvent.task_type,
                TelemetryEvent.tier_requested,
                func.count(TelemetryEvent.id).label("total_runs"),
                func.avg(TelemetryEvent.latency_seconds).label("avg_latency_seconds"),
                func.avg(TelemetryEvent.latency_ms).label("avg_latency_ms"),
                func.avg(TelemetryEvent.total_tokens).label("avg_total_tokens"),
                func.avg(TelemetryEvent.input_tokens).label("avg_input_tokens"),
                func.avg(TelemetryEvent.output_tokens).label("avg_output_tokens"),
                func.avg(TelemetryEvent.estimated_cost_usd).label("avg_cost_usd"),
                func.avg(TelemetryEvent.quality_score).label("avg_quality_score"),
                (
                    func.sum(func.cast(TelemetryEvent.success, Integer)) * 100.0 / func.count(TelemetryEvent.id)
                ).label("success_rate"),
            ).group_by(
                TelemetryEvent.strategy_id,
                TelemetryEvent.task_type,
                TelemetryEvent.tier_requested,
            )

            if tier is not None:
                stmt = stmt.where(TelemetryEvent.tier_requested == tier)
            if task_type:
                stmt = stmt.where(TelemetryEvent.task_type == task_type)
            if strategy_id:
                stmt = stmt.where(TelemetryEvent.strategy_id == strategy_id)

            rows = s.execute(stmt).all()
            return [
                {
                    "strategy_id": r.strategy_id,
                    "task_type": r.task_type,
                    "tier_requested": r.tier_requested,
                    "total_runs": r.total_runs,
                    "avg_latency_seconds": round(float(r.avg_latency_seconds or 0.0), 3),
                    "avg_latency_ms": round(float(r.avg_latency_ms or 0.0), 1),
                    "avg_total_tokens": round(float(r.avg_total_tokens or 0.0), 1),
                    "avg_input_tokens": round(float(r.avg_input_tokens or 0.0), 1),
                    "avg_output_tokens": round(float(r.avg_output_tokens or 0.0), 1),
                    "avg_cost_usd": round(float(r.avg_cost_usd or 0.0), 5),
                    "avg_quality_score": round(float(r.avg_quality_score or 0.0), 2)
                    if r.avg_quality_score is not None
                    else None,
                    "success_rate": round(float(r.success_rate or 0.0), 2),
                }
                for r in rows
            ]

        if session:
            return _calc(session)
        with session_scope(self._session_factory) as s:
            return _calc(s)

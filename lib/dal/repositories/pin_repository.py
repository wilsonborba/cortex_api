from __future__ import annotations

from typing import List, Optional
from sqlalchemy import select
from sqlalchemy.orm import Session

from lib.dal.local.database import SessionLocal, session_scope
from lib.dal.models import RoutingPin


class RoutingPinRepository:
    def __init__(self, session_factory=SessionLocal) -> None:
        self._session_factory = session_factory

    def set_pin(
        self,
        tier: int,
        task_type: Optional[str] = None,
        strategy_id: Optional[str] = None,
        model_id: Optional[str] = None,
        pinned_by: Optional[str] = None,
        session: Optional[Session] = None,
    ) -> RoutingPin:
        def _set(s: Session) -> RoutingPin:
            stmt = select(RoutingPin).where(
                RoutingPin.tier == tier,
                RoutingPin.task_type == task_type,
            )
            existing = s.scalar(stmt)
            if existing:
                existing.strategy_id = strategy_id
                existing.model_id = model_id
                existing.pinned_by = pinned_by
                existing.is_active = True
                return existing

            pin = RoutingPin(
                tier=tier,
                task_type=task_type,
                strategy_id=strategy_id,
                model_id=model_id,
                pinned_by=pinned_by,
                is_active=True,
            )
            s.add(pin)
            return pin

        if session:
            return _set(session)
        with session_scope(self._session_factory) as s:
            return _set(s)

    def get_pin(
        self,
        tier: int,
        task_type: Optional[str] = None,
        session: Optional[Session] = None,
    ) -> Optional[RoutingPin]:
        def _get(s: Session) -> Optional[RoutingPin]:
            # First check for exact (tier, task_type) pin
            if task_type:
                stmt = select(RoutingPin).where(
                    RoutingPin.tier == tier,
                    RoutingPin.task_type == task_type,
                    RoutingPin.is_active == True,
                )
                pin = s.scalar(stmt)
                if pin:
                    return pin

            # Fallback to tier-level pin (task_type is None)
            stmt = select(RoutingPin).where(
                RoutingPin.tier == tier,
                RoutingPin.task_type == None,
                RoutingPin.is_active == True,
            )
            return s.scalar(stmt)

        if session:
            return _get(session)
        with session_scope(self._session_factory) as s:
            return _get(s)

    def list_pins(
        self,
        is_active: Optional[bool] = True,
        session: Optional[Session] = None,
    ) -> List[RoutingPin]:
        def _list(s: Session) -> List[RoutingPin]:
            stmt = select(RoutingPin)
            if is_active is not None:
                stmt = stmt.where(RoutingPin.is_active == is_active)
            return list(s.scalars(stmt).all())

        if session:
            return _list(session)
        with session_scope(self._session_factory) as s:
            return _list(s)

    def remove_pin(
        self,
        tier: int,
        task_type: Optional[str] = None,
        session: Optional[Session] = None,
    ) -> bool:
        def _remove(s: Session) -> bool:
            stmt = select(RoutingPin).where(
                RoutingPin.tier == tier,
                RoutingPin.task_type == task_type,
            )
            pin = s.scalar(stmt)
            if pin:
                s.delete(pin)
                return True
            return False

        if session:
            return _remove(session)
        with session_scope(self._session_factory) as s:
            return _remove(s)

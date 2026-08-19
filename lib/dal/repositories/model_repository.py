from __future__ import annotations

from datetime import datetime
from typing import List, Optional
from sqlalchemy import select
from sqlalchemy.orm import Session

from lib.dal.local.database import SessionLocal, session_scope
from lib.dal.models import AccessStatus, ModelCatalogEntry


class ModelRepository:
    def __init__(self, session_factory=SessionLocal) -> None:
        self._session_factory = session_factory

    def get_by_id(self, model_id: str, session: Optional[Session] = None) -> Optional[ModelCatalogEntry]:
        if session:
            return session.get(ModelCatalogEntry, model_id)
        with session_scope(self._session_factory) as s:
            return s.get(ModelCatalogEntry, model_id)

    def list_models(
        self,
        provider: Optional[str] = None,
        tier: Optional[int] = None,
        access_status: Optional[str] = None,
        is_enabled: Optional[bool] = None,
        session: Optional[Session] = None,
    ) -> List[ModelCatalogEntry]:
        def _query(s: Session) -> List[ModelCatalogEntry]:
            stmt = select(ModelCatalogEntry)
            if provider:
                stmt = stmt.where(ModelCatalogEntry.provider == provider)
            if access_status:
                stmt = stmt.where(ModelCatalogEntry.access_status == access_status)
            if is_enabled is not None:
                stmt = stmt.where(ModelCatalogEntry.is_enabled == is_enabled)
            
            results = list(s.scalars(stmt).all())
            if tier is not None:
                results = [m for m in results if tier in (m.tier_eligibility or [])]
            return results

        if session:
            return _query(session)
        with session_scope(self._session_factory) as s:
            return _query(s)

    def upsert(self, entry: ModelCatalogEntry, session: Optional[Session] = None) -> ModelCatalogEntry:
        def _upsert(s: Session) -> ModelCatalogEntry:
            existing = s.get(ModelCatalogEntry, entry.id)
            if existing:
                existing.provider = entry.provider
                existing.display_name = entry.display_name
                existing.access_status = entry.access_status
                existing.status_reason = entry.status_reason
                existing.parameter_size = entry.parameter_size
                existing.context_window = entry.context_window
                existing.is_local = entry.is_local
                existing.tier_eligibility = entry.tier_eligibility
                existing.capabilities = entry.capabilities
                existing.cost_per_million_tokens = entry.cost_per_million_tokens
                existing.is_enabled = entry.is_enabled
                existing.cooldown_until = entry.cooldown_until
                return existing
            s.add(entry)
            return entry

        if session:
            return _upsert(session)
        with session_scope(self._session_factory) as s:
            return _upsert(s)

    def update_status(
        self,
        model_id: str,
        status: str | AccessStatus,
        reason: Optional[str] = None,
        session: Optional[Session] = None,
    ) -> Optional[ModelCatalogEntry]:
        status_val = status.value if isinstance(status, AccessStatus) else status

        def _update(s: Session) -> Optional[ModelCatalogEntry]:
            m = s.get(ModelCatalogEntry, model_id)
            if m:
                m.access_status = status_val
                if reason is not None:
                    m.status_reason = reason
            return m

        if session:
            return _update(session)
        with session_scope(self._session_factory) as s:
            return _update(s)

    def set_cooldown(
        self,
        model_id: str,
        until: datetime,
        reason: Optional[str] = None,
        session: Optional[Session] = None,
    ) -> Optional[ModelCatalogEntry]:
        def _update(s: Session) -> Optional[ModelCatalogEntry]:
            m = s.get(ModelCatalogEntry, model_id)
            if m:
                m.access_status = AccessStatus.COOLING_DOWN.value
                m.cooldown_until = until
                if reason is not None:
                    m.status_reason = reason
            return m

        if session:
            return _update(session)
        with session_scope(self._session_factory) as s:
            return _update(s)

    def clear_cooldown(
        self, model_id: str, session: Optional[Session] = None
    ) -> Optional[ModelCatalogEntry]:
        def _update(s: Session) -> Optional[ModelCatalogEntry]:
            m = s.get(ModelCatalogEntry, model_id)
            if m:
                m.cooldown_until = None
            return m

        if session:
            return _update(session)
        with session_scope(self._session_factory) as s:
            return _update(s)

    # -- context-format preference (issue #15) ---------------------------------

    def set_context_format_pin(
        self,
        model_id: str,
        format_name: str,
        expires_at: Optional[datetime] = None,
        session: Optional[Session] = None,
    ) -> Optional[ModelCatalogEntry]:
        def _update(s: Session) -> Optional[ModelCatalogEntry]:
            m = s.get(ModelCatalogEntry, model_id)
            if m:
                m.context_format_pin = format_name
                m.context_format_pin_expires_at = expires_at
            return m

        if session:
            return _update(session)
        with session_scope(self._session_factory) as s:
            return _update(s)

    def clear_context_format_pin(
        self, model_id: str, session: Optional[Session] = None
    ) -> Optional[ModelCatalogEntry]:
        def _update(s: Session) -> Optional[ModelCatalogEntry]:
            m = s.get(ModelCatalogEntry, model_id)
            if m:
                m.context_format_pin = None
                m.context_format_pin_expires_at = None
            return m

        if session:
            return _update(session)
        with session_scope(self._session_factory) as s:
            return _update(s)

    def set_context_format_computed(
        self, model_id: str, format_name: str, session: Optional[Session] = None
    ) -> Optional[ModelCatalogEntry]:
        def _update(s: Session) -> Optional[ModelCatalogEntry]:
            m = s.get(ModelCatalogEntry, model_id)
            if m:
                m.context_format_computed = format_name
            return m

        if session:
            return _update(session)
        with session_scope(self._session_factory) as s:
            return _update(s)

    def update_config(
        self,
        model_id: str,
        tier_eligibility: Optional[List[int]] = None,
        is_enabled: Optional[bool] = None,
        session: Optional[Session] = None,
    ) -> Optional[ModelCatalogEntry]:
        def _update(s: Session) -> Optional[ModelCatalogEntry]:
            m = s.get(ModelCatalogEntry, model_id)
            if m:
                if tier_eligibility is not None:
                    m.tier_eligibility = tier_eligibility
                if is_enabled is not None:
                    m.is_enabled = is_enabled
            return m

        if session:
            return _update(session)
        with session_scope(self._session_factory) as s:
            return _update(s)

from __future__ import annotations

from typing import List, Optional
from sqlalchemy import select
from sqlalchemy.orm import Session

from lib.dal.local.database import SessionLocal, session_scope
from lib.dal.models import TierPolicy


class TierPolicyRepository:
    def __init__(self, session_factory=SessionLocal) -> None:
        self._session_factory = session_factory

    def get_policy(self, tier: int, session: Optional[Session] = None) -> Optional[TierPolicy]:
        if session:
            return session.get(TierPolicy, tier)
        with session_scope(self._session_factory) as s:
            return s.get(TierPolicy, tier)

    def list_policies(self, session: Optional[Session] = None) -> List[TierPolicy]:
        def _list(s: Session) -> List[TierPolicy]:
            stmt = select(TierPolicy).order_by(TierPolicy.tier)
            return list(s.scalars(stmt).all())

        if session:
            return _list(session)
        with session_scope(self._session_factory) as s:
            return _list(s)

    def upsert_policy(self, policy: TierPolicy, session: Optional[Session] = None) -> TierPolicy:
        def _upsert(s: Session) -> TierPolicy:
            existing = s.get(TierPolicy, policy.tier)
            if existing:
                existing.name = policy.name
                existing.max_latency_seconds = policy.max_latency_seconds
                existing.allow_multi_model = policy.allow_multi_model
                existing.allow_external = policy.allow_external
                existing.retrieval_mode = policy.retrieval_mode
                existing.require_verification = policy.require_verification
                existing.max_model_calls = policy.max_model_calls
                existing.allowed_models = policy.allowed_models
                return existing
            s.add(policy)
            return policy

        if session:
            return _upsert(session)
        with session_scope(self._session_factory) as s:
            return _upsert(s)

    def update_policy(
        self,
        tier: int,
        name: Optional[str] = None,
        max_latency_seconds: Optional[int] = None,
        allow_multi_model: Optional[bool] = None,
        allow_external: Optional[bool] = None,
        retrieval_mode: Optional[str] = None,
        require_verification: Optional[bool] = None,
        max_model_calls: Optional[int] = None,
        allowed_models: Optional[List[str]] = None,
        session: Optional[Session] = None,
    ) -> Optional[TierPolicy]:
        def _update(s: Session) -> Optional[TierPolicy]:
            p = s.get(TierPolicy, tier)
            if p:
                if name is not None:
                    p.name = name
                if max_latency_seconds is not None:
                    p.max_latency_seconds = max_latency_seconds
                if allow_multi_model is not None:
                    p.allow_multi_model = allow_multi_model
                if allow_external is not None:
                    p.allow_external = allow_external
                if retrieval_mode is not None:
                    p.retrieval_mode = retrieval_mode
                if require_verification is not None:
                    p.require_verification = require_verification
                if max_model_calls is not None:
                    p.max_model_calls = max_model_calls
                if allowed_models is not None:
                    p.allowed_models = allowed_models
            return p

        if session:
            return _update(session)
        with session_scope(self._session_factory) as s:
            return _update(s)

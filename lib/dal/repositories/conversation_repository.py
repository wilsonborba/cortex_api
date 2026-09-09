from __future__ import annotations

from datetime import datetime, timezone
from typing import List, Optional
from sqlalchemy import select
from sqlalchemy.orm import Session

from lib.dal.local.database import SessionLocal, session_scope
from lib.dal.models import Conversation


class ConversationIdConflict(Exception):
    """Raised when `conversation_id` (client-suppliable on create, and
    always taken as-is on rename/pin/delete) already belongs to a *different*
    tenant. `id` is the table's global primary key, so it can never be
    silently reused across tenants -- that would be exactly the IDOR this
    repository's tenant-scoped lookups are meant to prevent. Callers should
    turn this into a 409, not a fresh row and not a 500."""


class ConversationRepository:
    def __init__(self, session_factory=SessionLocal) -> None:
        self._session_factory = session_factory

    def _find(self, s: Session, conversation_id: str, tenant_id: str) -> Optional[Conversation]:
        """Every existence check in this repository must go through this,
        never a bare `s.get(Conversation, conversation_id)`: a plain
        primary-key lookup ignores `tenant_id` entirely, and conversation
        ids are timestamp+counter based (guessable), so that was a real
        IDOR letting any tenant rename/pin/delete another tenant's
        conversation row just by guessing/observing its id."""
        stmt = select(Conversation).where(
            Conversation.id == conversation_id, Conversation.tenant_id == tenant_id
        )
        return s.scalar(stmt)

    def _reject_if_id_taken_by_another_tenant(
        self, s: Session, conversation_id: str, tenant_id: str
    ) -> None:
        """Call right before inserting a new row under [conversation_id]:
        a bare, deliberately *not* tenant-filtered PK lookup, used only to
        detect a cross-tenant id collision before it hits the database as
        a raw IntegrityError."""
        if s.get(Conversation, conversation_id) is not None:
            raise ConversationIdConflict(
                f"conversation id {conversation_id!r} belongs to a different tenant"
            )

    def create(
        self,
        conversation_id: str,
        tenant_id: str = "default",
        title: str = "New Conversation",
        session: Optional[Session] = None,
    ) -> Conversation:
        def _create(s: Session) -> Conversation:
            existing = self._find(s, conversation_id, tenant_id)
            if existing:
                return existing
            self._reject_if_id_taken_by_another_tenant(s, conversation_id, tenant_id)
            convo = Conversation(id=conversation_id, tenant_id=tenant_id, title=title)
            s.add(convo)
            s.flush()
            return convo

        if session:
            return _create(session)
        with session_scope(self._session_factory) as s:
            convo = _create(s)
            s.refresh(convo)
            return convo

    def get(
        self, conversation_id: str, tenant_id: str = "default", session: Optional[Session] = None
    ) -> Optional[Conversation]:
        if session:
            return self._find(session, conversation_id, tenant_id)
        with session_scope(self._session_factory) as s:
            return self._find(s, conversation_id, tenant_id)

    def list_active(
        self, tenant_id: str = "default", session: Optional[Session] = None
    ) -> List[Conversation]:
        def _list(s: Session) -> List[Conversation]:
            stmt = select(Conversation).where(
                Conversation.tenant_id == tenant_id, Conversation.deleted_at.is_(None)
            )
            return list(s.scalars(stmt).all())

        if session:
            return _list(session)
        with session_scope(self._session_factory) as s:
            return _list(s)

    def rename(
        self,
        conversation_id: str,
        title: str,
        tenant_id: str = "default",
        session: Optional[Session] = None,
    ) -> Conversation:
        def _rename(s: Session) -> Conversation:
            convo = self._find(s, conversation_id, tenant_id)
            if convo is None:
                self._reject_if_id_taken_by_another_tenant(s, conversation_id, tenant_id)
                convo = Conversation(id=conversation_id, tenant_id=tenant_id, title=title)
                s.add(convo)
            else:
                convo.title = title
                convo.deleted_at = None
            s.flush()
            return convo

        if session:
            return _rename(session)
        with session_scope(self._session_factory) as s:
            convo = _rename(s)
            s.refresh(convo)
            return convo

    def set_pinned(
        self,
        conversation_id: str,
        pinned: bool,
        tenant_id: str = "default",
        session: Optional[Session] = None,
    ) -> Conversation:
        def _set_pinned(s: Session) -> Conversation:
            convo = self._find(s, conversation_id, tenant_id)
            if convo is None:
                self._reject_if_id_taken_by_another_tenant(s, conversation_id, tenant_id)
                convo = Conversation(id=conversation_id, tenant_id=tenant_id, is_pinned=pinned)
                s.add(convo)
            else:
                convo.is_pinned = pinned
                convo.deleted_at = None
            s.flush()
            return convo

        if session:
            return _set_pinned(session)
        with session_scope(self._session_factory) as s:
            convo = _set_pinned(s)
            s.refresh(convo)
            return convo

    def soft_delete(
        self, conversation_id: str, tenant_id: str = "default", session: Optional[Session] = None
    ) -> None:
        """No-op when [conversation_id] isn't (or isn't yet) one of this
        tenant's conversations: unlike rename/pin, there is no reason to
        fabricate a soft-deleted row for an id that was never this
        tenant's to begin with (also avoids ever attempting an insert that
        could collide with another tenant's real row sharing the id)."""
        def _delete(s: Session) -> None:
            convo = self._find(s, conversation_id, tenant_id)
            if convo is None:
                return
            convo.deleted_at = datetime.now(timezone.utc)
            s.flush()

        if session:
            _delete(session)
            return
        with session_scope(self._session_factory) as s:
            _delete(s)

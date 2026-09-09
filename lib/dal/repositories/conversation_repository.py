from __future__ import annotations

from datetime import datetime, timezone
from typing import List, Optional
from sqlalchemy import select
from sqlalchemy.orm import Session

from lib.dal.local.database import SessionLocal, session_scope
from lib.dal.models import Conversation


class ConversationRepository:
    def __init__(self, session_factory=SessionLocal) -> None:
        self._session_factory = session_factory

    def create(
        self,
        conversation_id: str,
        tenant_id: str = "default",
        title: str = "New Conversation",
        session: Optional[Session] = None,
    ) -> Conversation:
        def _create(s: Session) -> Conversation:
            existing = s.get(Conversation, conversation_id)
            if existing:
                return existing
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
        def _get(s: Session) -> Optional[Conversation]:
            stmt = select(Conversation).where(
                Conversation.id == conversation_id, Conversation.tenant_id == tenant_id
            )
            return s.scalar(stmt)

        if session:
            return _get(session)
        with session_scope(self._session_factory) as s:
            return _get(s)

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
            convo = s.get(Conversation, conversation_id)
            if convo is None:
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
            convo = s.get(Conversation, conversation_id)
            if convo is None:
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
        def _delete(s: Session) -> None:
            convo = s.get(Conversation, conversation_id)
            now = datetime.now(timezone.utc)
            if convo is None:
                convo = Conversation(id=conversation_id, tenant_id=tenant_id, deleted_at=now)
                s.add(convo)
            else:
                convo.deleted_at = now
            s.flush()

        if session:
            _delete(session)
            return
        with session_scope(self._session_factory) as s:
            _delete(s)

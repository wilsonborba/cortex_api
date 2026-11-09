from __future__ import annotations

from typing import List, Optional
from pydantic import BaseModel, Field


class ConversationAttachmentOut(BaseModel):
    filename: str


class ConversationTurnOut(BaseModel):
    id: str
    role: str
    content: str
    created_at: str
    attachments: List[ConversationAttachmentOut] = []


class ConversationSummaryOut(BaseModel):
    id: str
    title: str
    created_at: str
    updated_at: str
    message_count: int = 0
    preview: str = ""
    is_pinned: bool = False


class ConversationDetailOut(BaseModel):
    id: str
    title: str
    created_at: str
    updated_at: str
    messages: List[ConversationTurnOut] = []
    is_pinned: bool = False


class ConversationCreateIn(BaseModel):
    id: Optional[str] = Field(default=None, description="Client-supplied conversation id; generated if omitted")
    tenant_id: str = "default"
    title: str = "New Conversation"


class ConversationPatchIn(BaseModel):
    title: Optional[str] = None
    is_pinned: Optional[bool] = None

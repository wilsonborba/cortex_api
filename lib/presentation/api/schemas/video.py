from __future__ import annotations

from typing import List, Optional

from pydantic import BaseModel


class VideoJobCreated(BaseModel):
    attachment_id: str
    status: str  # "processing"


class VideoJobStatus(BaseModel):
    attachment_id: str
    status: str  # "processing" | "done" | "error"
    summary: Optional[str] = None
    transcript: Optional[str] = None
    errors: List[str] = []

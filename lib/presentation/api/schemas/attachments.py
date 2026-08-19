from __future__ import annotations

from pydantic import BaseModel


class Attachment(BaseModel):
    filename: str
    mime_type: str  # "audio/..." | "image/..." -- other types are rejected at ingestion
    data_base64: str

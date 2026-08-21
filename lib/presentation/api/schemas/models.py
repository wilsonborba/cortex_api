from __future__ import annotations

from datetime import datetime
from typing import Any, Dict, List, Optional

from pydantic import BaseModel, Field

from lib.dal.models import ModelCatalogEntry


class ModelOut(BaseModel):
    id: str
    provider: str
    display_name: str
    access_status: str
    status_reason: Optional[str] = None
    parameter_size: Optional[str] = None
    context_window: int
    is_local: bool
    source_kind: str
    tier_eligibility: List[int]
    capabilities: Dict[str, Any]
    cost_per_million_tokens: float
    is_enabled: bool
    context_format_computed: Optional[str] = None
    context_format_pin: Optional[str] = None
    context_format_pin_expires_at: Optional[datetime] = None

    @classmethod
    def from_entry(cls, entry: ModelCatalogEntry) -> "ModelOut":
        return cls(
            id=entry.id, provider=entry.provider, display_name=entry.display_name,
            access_status=entry.access_status, status_reason=entry.status_reason,
            parameter_size=entry.parameter_size, context_window=entry.context_window,
            is_local=entry.is_local, source_kind=entry.source_kind, tier_eligibility=list(entry.tier_eligibility or []),
            capabilities=dict(entry.capabilities or {}), cost_per_million_tokens=entry.cost_per_million_tokens,
            is_enabled=entry.is_enabled,
            context_format_computed=entry.context_format_computed,
            context_format_pin=entry.context_format_pin,
            context_format_pin_expires_at=entry.context_format_pin_expires_at,
        )


class ModelConfigRequest(BaseModel):
    tier_eligibility: Optional[List[int]] = None
    is_enabled: Optional[bool] = None
    context_format_pin: Optional[str] = Field(
        default=None,
        description="'toon' or 'json' to set/replace the pin, 'none' to clear it, omit to leave untouched",
    )
    context_format_pin_ttl_seconds: Optional[int] = Field(
        default=None, description="Pin expiry, relative to now. Omit for a pin that lasts until explicitly cleared."
    )

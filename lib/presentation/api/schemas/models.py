from __future__ import annotations

from typing import Any, Dict, List, Optional

from pydantic import BaseModel

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
    tier_eligibility: List[int]
    capabilities: Dict[str, Any]
    cost_per_million_tokens: float
    is_enabled: bool

    @classmethod
    def from_entry(cls, entry: ModelCatalogEntry) -> "ModelOut":
        return cls(
            id=entry.id, provider=entry.provider, display_name=entry.display_name,
            access_status=entry.access_status, status_reason=entry.status_reason,
            parameter_size=entry.parameter_size, context_window=entry.context_window,
            is_local=entry.is_local, tier_eligibility=list(entry.tier_eligibility or []),
            capabilities=dict(entry.capabilities or {}), cost_per_million_tokens=entry.cost_per_million_tokens,
            is_enabled=entry.is_enabled,
        )


class ModelConfigRequest(BaseModel):
    tier_eligibility: Optional[List[int]] = None
    is_enabled: Optional[bool] = None

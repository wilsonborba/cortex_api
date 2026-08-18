from __future__ import annotations

from typing import Optional

from pydantic import BaseModel

from lib.dal.models import RoutingPin


class PinOut(BaseModel):
    tier: int
    task: Optional[str] = None
    strategy_id: Optional[str] = None
    model_id: Optional[str] = None
    pinned_by: Optional[str] = None
    is_active: bool

    @classmethod
    def from_pin(cls, pin: RoutingPin) -> "PinOut":
        return cls(
            tier=pin.tier, task=pin.task_type, strategy_id=pin.strategy_id, model_id=pin.model_id,
            pinned_by=pin.pinned_by, is_active=pin.is_active,
        )


class PinCreateRequest(BaseModel):
    tier: int
    task: Optional[str] = None
    strategy_id: Optional[str] = None
    model_id: Optional[str] = None
    pinned_by: Optional[str] = None

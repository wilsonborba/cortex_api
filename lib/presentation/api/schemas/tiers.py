from __future__ import annotations

from typing import List, Optional

from pydantic import BaseModel

from lib.dal.models import TierPolicy


class TierOut(BaseModel):
    tier: int
    name: str
    max_latency_seconds: int
    allow_multi_model: bool
    allow_external: bool
    retrieval_mode: str
    require_verification: bool
    max_model_calls: int
    allowed_models: Optional[List[str]] = None

    @classmethod
    def from_policy(cls, policy: TierPolicy) -> "TierOut":
        return cls(
            tier=policy.tier, name=policy.name, max_latency_seconds=policy.max_latency_seconds,
            allow_multi_model=policy.allow_multi_model, allow_external=policy.allow_external,
            retrieval_mode=policy.retrieval_mode, require_verification=policy.require_verification,
            max_model_calls=policy.max_model_calls,
            allowed_models=list(policy.allowed_models) if policy.allowed_models is not None else None,
        )


class TierConfigRequest(BaseModel):
    max_latency_seconds: Optional[int] = None
    allow_multi_model: Optional[bool] = None
    allow_external: Optional[bool] = None
    retrieval_mode: Optional[str] = None
    require_verification: Optional[bool] = None
    max_model_calls: Optional[int] = None
    allowed_models: Optional[List[str]] = None

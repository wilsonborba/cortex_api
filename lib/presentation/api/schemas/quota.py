from __future__ import annotations

from typing import Optional

from pydantic import BaseModel

from lib.engine.quota import ProviderQuota


class QuotaOut(BaseModel):
    provider: str
    window_hours: int
    window_limit_tokens: Optional[int] = None
    consumed_tokens: int
    remaining_tokens: Optional[int] = None
    quota_factor: float
    requests_count: int

    @classmethod
    def from_provider_quota(cls, quota: ProviderQuota) -> "QuotaOut":
        return cls(
            provider=quota.provider, window_hours=quota.window_hours,
            window_limit_tokens=quota.window_limit_tokens, consumed_tokens=quota.consumed_tokens,
            remaining_tokens=quota.remaining_tokens, quota_factor=quota.quota_factor,
            requests_count=quota.requests_count,
        )

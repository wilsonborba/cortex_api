from __future__ import annotations

from typing import List

from fastapi import APIRouter, Depends

from lib.engine.quota import QuotaTracker
from lib.presentation.api.deps import get_quota_tracker
from lib.presentation.api.schemas.quota import QuotaOut

router = APIRouter(tags=["quota"])


@router.get("/quota", response_model=List[QuotaOut])
def get_quota_summary(tracker: QuotaTracker = Depends(get_quota_tracker)) -> List[QuotaOut]:
    return [QuotaOut.from_provider_quota(q) for q in tracker.get_summary()]


@router.get("/quota/{provider}", response_model=QuotaOut)
def get_quota_for_provider(provider: str, tracker: QuotaTracker = Depends(get_quota_tracker)) -> QuotaOut:
    return QuotaOut.from_provider_quota(tracker.get_quota(provider))

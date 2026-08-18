from __future__ import annotations

from typing import List

from fastapi import APIRouter, Depends, HTTPException

from lib.engine.tiers import TierService
from lib.presentation.api.deps import get_tier_service
from lib.presentation.api.schemas.tiers import TierConfigRequest, TierOut

router = APIRouter(tags=["tiers"])


@router.get("/tiers", response_model=List[TierOut])
def list_tiers(service: TierService = Depends(get_tier_service)) -> List[TierOut]:
    return [TierOut.from_policy(p) for p in service.list_envelopes()]


@router.patch("/tiers/{tier}", response_model=TierOut)
def configure_tier(
    tier: int, payload: TierConfigRequest, service: TierService = Depends(get_tier_service)
) -> TierOut:
    updated = service.configure(
        tier,
        max_latency_seconds=payload.max_latency_seconds,
        allow_multi_model=payload.allow_multi_model,
        allow_external=payload.allow_external,
        retrieval_mode=payload.retrieval_mode,
        require_verification=payload.require_verification,
        max_model_calls=payload.max_model_calls,
    )
    if payload.allowed_models is not None:
        updated = service.set_models(tier, payload.allowed_models)
    if updated is None:
        raise HTTPException(status_code=404, detail=f"tier {tier} not found")
    return TierOut.from_policy(updated)

from __future__ import annotations

from typing import List, Optional

from fastapi import APIRouter, Depends, HTTPException

from lib.engine.registry_service import ModelRegistryService
from lib.presentation.api.deps import get_registry
from lib.presentation.api.schemas.models import ModelConfigRequest, ModelOut

router = APIRouter(tags=["models"])


@router.get("/models", response_model=List[ModelOut])
def list_models(
    provider: Optional[str] = None,
    tier: Optional[int] = None,
    status: Optional[str] = None,
    registry: ModelRegistryService = Depends(get_registry),
) -> List[ModelOut]:
    models = registry.list_models(provider=provider, tier=tier, access_status=status)
    return [ModelOut.from_entry(m) for m in models]


@router.post("/models/sync", response_model=List[ModelOut])
def sync_models(registry: ModelRegistryService = Depends(get_registry)) -> List[ModelOut]:
    return [ModelOut.from_entry(m) for m in registry.sync()]


@router.get("/models/{model_id:path}", response_model=ModelOut)
def get_model(model_id: str, registry: ModelRegistryService = Depends(get_registry)) -> ModelOut:
    model = registry.get_model(model_id)
    if model is None:
        raise HTTPException(status_code=404, detail=f"model {model_id!r} not found")
    return ModelOut.from_entry(model)


@router.patch("/models/{model_id:path}", response_model=ModelOut)
def configure_model(
    model_id: str, payload: ModelConfigRequest, registry: ModelRegistryService = Depends(get_registry)
) -> ModelOut:
    updated = registry.update_config(
        model_id, tier_eligibility=payload.tier_eligibility, is_enabled=payload.is_enabled
    )
    if updated is None:
        raise HTTPException(status_code=404, detail=f"model {model_id!r} not found")
    return ModelOut.from_entry(updated)

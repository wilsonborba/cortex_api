from __future__ import annotations

from typing import List, Optional

from fastapi import APIRouter, Depends, HTTPException, Response

from lib.dal.repositories.pin_repository import RoutingPinRepository
from lib.presentation.api.deps import get_pin_repo
from lib.presentation.api.schemas.pins import PinCreateRequest, PinOut

router = APIRouter(prefix="/routing/pins", tags=["pins"])


@router.get("", response_model=List[PinOut])
def list_pins(pin_repo: RoutingPinRepository = Depends(get_pin_repo)) -> List[PinOut]:
    return [PinOut.from_pin(p) for p in pin_repo.list_pins()]


@router.post("", response_model=PinOut, status_code=201)
def create_pin(payload: PinCreateRequest, pin_repo: RoutingPinRepository = Depends(get_pin_repo)) -> PinOut:
    pin = pin_repo.set_pin(
        tier=payload.tier, task_type=payload.task, strategy_id=payload.strategy_id,
        model_id=payload.model_id, pinned_by=payload.pinned_by,
    )
    return PinOut.from_pin(pin)


@router.delete("", status_code=204, response_class=Response)
def delete_pin(
    tier: int, task: Optional[str] = None, pin_repo: RoutingPinRepository = Depends(get_pin_repo)
) -> Response:
    removed = pin_repo.remove_pin(tier, task_type=task)
    if not removed:
        raise HTTPException(status_code=404, detail=f"no pin for tier={tier} task={task!r}")
    return Response(status_code=204)

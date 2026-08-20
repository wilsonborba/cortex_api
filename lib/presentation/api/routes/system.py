from __future__ import annotations

from typing import Any, Dict

from fastapi import APIRouter, Request

from lib.core.capabilities import detect_runtime_capabilities
from lib.core.settings import get_settings

router = APIRouter(prefix="/system", tags=["system"])


@router.get("/capabilities")
def get_capabilities(request: Request) -> Dict[str, Any]:
    """Returns the installed Cortex profile, detected hardware capabilities,
    and availability of optional extensions and integrations."""
    settings = getattr(request.app.state, "settings", None) or get_settings()
    return detect_runtime_capabilities(settings).to_dict()

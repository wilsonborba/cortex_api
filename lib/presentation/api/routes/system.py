from __future__ import annotations

from datetime import datetime, timezone
from typing import Any, Dict

from fastapi import APIRouter, Request

from lib.core.capabilities import detect_runtime_capabilities
from lib.core.settings import get_settings

router = APIRouter(prefix="/system", tags=["system"])

# Captured once, at module import time (i.e. process start): the only
# reliable "when was this actually deployed" signal available without a
# separate build step, since this backend doesn't compile-time-embed a
# version string the way the Flutter frontend does (dart-define
# BUILD_TIMESTAMP). Restarting the service (`systemctl restart cortex-api`)
# updates this, so it's the fast way to confirm a restart actually took.
_PROCESS_STARTED_AT = datetime.now(timezone.utc).isoformat()


@router.get("/status")
def get_status() -> Dict[str, str]:
    """When this process last started, to confirm a deploy/restart took."""
    return {"started_at": _PROCESS_STARTED_AT}


@router.get("/capabilities")
def get_capabilities(request: Request) -> Dict[str, Any]:
    """Returns the installed Cortex profile, detected hardware capabilities,
    and availability of optional extensions and integrations."""
    settings = getattr(request.app.state, "settings", None) or get_settings()
    data = detect_runtime_capabilities(settings).to_dict()
    data["process_started_at"] = _PROCESS_STARTED_AT
    return data

from __future__ import annotations

from pathlib import Path
from typing import AsyncIterator

from fastapi import APIRouter, WebSocket, WebSocketDisconnect

from lib.core.logs import _tail
from lib.core.settings import get_settings

router = APIRouter(tags=["logs"])


@router.websocket("/logs/stream")
async def stream_logs(websocket: WebSocket) -> None:
    """Streams the log file live, line by line, as it is written.
    Clients connect and receive new log frames via WebSocket text messages.
    """
    await websocket.accept()
    settings = get_settings()
    try:
        async for line in _tail(settings.log_file):
            await websocket.send_text(line)
    except WebSocketDisconnect:
        pass

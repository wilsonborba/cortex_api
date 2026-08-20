from __future__ import annotations

import asyncio
from pathlib import Path

import pytest
from fastapi import FastAPI
from fastapi.testclient import TestClient

from lib.core.logs import _tail, configure_logging, get_logger, LogTarget
from lib.core.settings import Settings
from lib.presentation.api.routes.logs_stream import router


@pytest.mark.asyncio
async def test_async_tail_generator(tmp_path: Path):
    log_file = tmp_path / "tail_test.log"
    log_file.write_text("initial line 1\ninitial line 2\n", encoding="utf-8")

    tail_gen = _tail(log_file, poll_interval=0.05)
    
    # Give async generator a turn to seek to end
    async def write_lines():
        await asyncio.sleep(0.1)
        with log_file.open("a", encoding="utf-8") as f:
            f.write("new line 1\nnew line 2\n")
            f.flush()

    writer_task = asyncio.create_task(write_lines())

    collected = []
    async for line in tail_gen:
        collected.append(line)
        if len(collected) == 2:
            break

    await writer_task
    assert collected == ["new line 1", "new line 2"]


def test_websocket_log_stream(tmp_path: Path, monkeypatch):
    log_file = tmp_path / "stream_app.log"
    log_file.write_text("old entry\n", encoding="utf-8")

    mock_settings = Settings(log_file=log_file)
    monkeypatch.setattr("lib.presentation.api.routes.logs_stream.get_settings", lambda: mock_settings)

    app = FastAPI()
    app.include_router(router)

    client = TestClient(app)
    with client.websocket_connect("/logs/stream") as ws:
        with log_file.open("a", encoding="utf-8") as f:
            f.write("live streaming line\n")
            f.flush()
        received = ws.receive_text()
        assert received == "live streaming line"

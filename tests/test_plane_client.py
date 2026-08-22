from __future__ import annotations

import pytest
import httpx

from lib.engine.retrieval.plane_client import PlaneClient, PlaneTask


@pytest.mark.asyncio
async def test_plane_client_list_tasks():
    def handler(request: httpx.Request) -> httpx.Response:
        return httpx.Response(200, json=[{"id": "issue-1", "name": "Implement feature X"}])

    transport = httpx.MockTransport(handler)
    client = PlaneClient("http://localhost:8011", client_factory=lambda: httpx.AsyncClient(transport=transport))

    tasks = await client.list_tasks("default", "proj-1")
    assert len(tasks) == 1
    assert tasks[0].id == "issue-1"
    assert tasks[0].name == "Implement feature X"

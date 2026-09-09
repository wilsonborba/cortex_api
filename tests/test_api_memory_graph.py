from __future__ import annotations

import pytest
from fastapi.testclient import TestClient

from lib.presentation.api.app import create_app
from lib.presentation.api.deps import get_hippocampus_client
from lib.engine.retrieval.hippocampus import HippocampusClient


class FakeAsyncClient:
    """Fakes hippocampus's real routes for two tenants (`tenant-a`,
    `tenant-b`), each with its own memory, so cross-tenant leakage would show
    up as the wrong tenant's node appearing in the other's graph/context."""

    def __init__(self):
        self.memories_by_workspace = {
            "tenant-a": [
                {"id": "mem-a1", "title": "Cortex architecture notes", "content": "Cortex routes by tier."},
            ],
            "tenant-b": [
                {"id": "mem-b1", "title": "attachment:report.pdf", "content": "Quarterly report text."},
            ],
        }
        self.graphs_by_memory = {
            "mem-a1": {
                "nodes": [
                    {"id": "mem-a1", "node_type": "memory", "label": "Cortex architecture notes"},
                    {"id": "tag:architecture", "node_type": "tag", "label": "architecture"},
                ],
                "edges": [
                    {"source_id": "mem-a1", "target_id": "tag:architecture", "edge_type": "tagged_with"},
                ],
            },
            "mem-b1": {
                "nodes": [
                    {"id": "mem-b1", "node_type": "memory", "label": "attachment:report.pdf"},
                ],
                "edges": [],
            },
        }

    async def __aenter__(self):
        return self

    async def __aexit__(self, *args):
        pass

    async def get(self, url, params=None, headers=None):
        class FakeResponse:
            def __init__(self, d, s=200):
                self.d = d
                self.status_code = s

            def raise_for_status(self):
                if self.status_code >= 400:
                    import httpx
                    raise httpx.HTTPStatusError("error", request=None, response=None)

            def json(self):
                return self.d

        params = params or {}
        if url.endswith("/api/v1/memories"):
            ws = params.get("workspace_id")
            return FakeResponse({"data": self.memories_by_workspace.get(ws, [])})

        if url.endswith("/graph"):
            memory_id = url.split("/api/v1/memories/")[1].split("/graph")[0]
            ws = params.get("workspace_id")
            owner_ws = "tenant-a" if memory_id == "mem-a1" else "tenant-b" if memory_id == "mem-b1" else None
            if owner_ws != ws:
                return FakeResponse({"error": "not found"}, 404)
            return FakeResponse({"data": self.graphs_by_memory.get(memory_id, {"nodes": [], "edges": []})})

        # GET /api/v1/memories/{id}
        memory_id = url.rsplit("/", 1)[-1]
        ws = params.get("workspace_id")
        owner_ws = "tenant-a" if memory_id == "mem-a1" else "tenant-b" if memory_id == "mem-b1" else None
        if ws is not None and owner_ws != ws:
            return FakeResponse({"error": "not found"}, 404)
        for items in self.memories_by_workspace.values():
            for m in items:
                if m["id"] == memory_id:
                    return FakeResponse({"data": m})
        return FakeResponse({"error": "not found"}, 404)

    async def post(self, url, json=None, headers=None):
        class FakeResponse:
            def __init__(self, d, s=200):
                self.d = d
                self.status_code = s

            def raise_for_status(self):
                pass

            def json(self):
                return self.d

        return FakeResponse({"data": []})


@pytest.fixture
def test_app():
    app = create_app()
    client = HippocampusClient(base_url="http://fake", client_factory=lambda: FakeAsyncClient())
    app.dependency_overrides[get_hippocampus_client] = lambda: client
    return app


def test_workspace_graph_is_scoped_to_the_requesting_tenant(test_app):
    client = TestClient(test_app, headers={"x-uuid": "tenant-a"})
    resp = client.get("/memory-graph")
    assert resp.status_code == 200
    data = resp.json()
    node_ids = {n["id"] for n in data["nodes"]}
    assert "mem-a1" in node_ids
    assert "mem-b1" not in node_ids


def test_attachment_memory_is_relabeled_as_attachment_node(test_app):
    client = TestClient(test_app, headers={"x-uuid": "tenant-b"})
    resp = client.get("/memory-graph")
    assert resp.status_code == 200
    nodes = resp.json()["nodes"]
    attachment_nodes = [n for n in nodes if n["id"] == "mem-b1"]
    assert len(attachment_nodes) == 1
    assert attachment_nodes[0]["node_type"] == "attachment"
    assert attachment_nodes[0]["label"] == "report.pdf"


def test_node_context_for_a_memory_id(test_app):
    client = TestClient(test_app, headers={"x-uuid": "tenant-a"})
    resp = client.get("/memory-graph/nodes/mem-a1/context")
    assert resp.status_code == 200
    body = resp.json()
    assert body["content"] == "Cortex routes by tier."


def test_node_context_refuses_another_tenants_memory_id(test_app):
    """Regression guard: tenant-a must not be able to read tenant-b's
    memory content just by guessing/observing its id."""
    client = TestClient(test_app, headers={"x-uuid": "tenant-a"})
    resp = client.get("/memory-graph/nodes/mem-b1/context")
    assert resp.status_code == 404

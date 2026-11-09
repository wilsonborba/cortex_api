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
            "tenant-c": [
                {"id": "mem-c1", "title": "Turn in convo-999", "content": "User: what is sqlite\n\nAssistant: it's an embedded database."},
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
            "mem-c1": {
                "nodes": [
                    {
                        "id": "mem-c1",
                        "node_type": "memory",
                        "label": "Turn in convo-999",
                        "metadata": {"content_preview": "User: what is sqlite\n\nAssistant: it's an embedded database."},
                    },
                    {"id": "tag:conversation:convo-999", "node_type": "tag", "label": "conversation:convo-999"},
                    {"id": "tag:tenant:tenant-c", "node_type": "tag", "label": "tenant:tenant-c"},
                    {"id": "tag:type:conversation-turn", "node_type": "tag", "label": "type:conversation-turn"},
                    {"id": "tag:general:general", "node_type": "tag", "label": "general:general"},
                ],
                "edges": [
                    {"source_id": "mem-c1", "target_id": "tag:conversation:convo-999", "edge_type": "tagged_with"},
                    {"source_id": "mem-c1", "target_id": "tag:tenant:tenant-c", "edge_type": "tagged_with"},
                    {"source_id": "mem-c1", "target_id": "tag:type:conversation-turn", "edge_type": "tagged_with"},
                    {"source_id": "mem-c1", "target_id": "tag:general:general", "edge_type": "tagged_with"},
                ],
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

        owners = {"mem-a1": "tenant-a", "mem-b1": "tenant-b", "mem-c1": "tenant-c"}

        if url.endswith("/graph"):
            memory_id = url.split("/api/v1/memories/")[1].split("/graph")[0]
            ws = params.get("workspace_id")
            if owners.get(memory_id) != ws:
                return FakeResponse({"error": "not found"}, 404)
            return FakeResponse({"data": self.graphs_by_memory.get(memory_id, {"nodes": [], "edges": []})})

        # GET /api/v1/memories/{id}
        memory_id = url.rsplit("/", 1)[-1]
        ws = params.get("workspace_id")
        if ws is not None and owners.get(memory_id) != ws:
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


def test_conversation_turn_label_is_rebuilt_from_content_preview(test_app):
    """Regression test: every conversation-turn memory shares the exact same
    generic title (`f"Turn in {conversation_id}"`), which used to leak
    straight through as the node's label -- every one of these nodes looked
    identical and told the user nothing. It must be rebuilt from the user's
    own words in `content_preview` instead."""
    client = TestClient(test_app, headers={"x-uuid": "tenant-c"})
    resp = client.get("/memory-graph")
    assert resp.status_code == 200
    nodes = resp.json()["nodes"]
    turn_nodes = [n for n in nodes if n["id"] == "mem-c1"]
    assert len(turn_nodes) == 1
    assert turn_nodes[0]["label"] == "what is sqlite"


def test_bookkeeping_tenant_and_type_tags_are_filtered_out(test_app):
    """`tenant:*`/`type:conversation-turn` tags are pure plumbing this API
    writes on every turn for its own lookups, never a meaningful topic --
    they must not clutter the graph."""
    client = TestClient(test_app, headers={"x-uuid": "tenant-c"})
    resp = client.get("/memory-graph")
    assert resp.status_code == 200
    node_ids = {n["id"] for n in resp.json()["nodes"]}
    assert "tag:tenant:tenant-c" not in node_ids
    assert "tag:type:conversation-turn" not in node_ids


def test_general_general_default_topic_tag_is_filtered_out(test_app):
    """`general:general` is what `task_type`'s default value canonicalizes
    to as a tag (see hippocampus's `canonicalize_tag_filter`) whenever a
    caller never set a real `memory_topic`/`task_type` -- every such
    attachment/turn shares the exact same meaningless tag, a floating node
    with no real link to anything ("uma tag solta sem link chamada
    general:general")."""
    client = TestClient(test_app, headers={"x-uuid": "tenant-c"})
    resp = client.get("/memory-graph")
    assert resp.status_code == 200
    node_ids = {n["id"] for n in resp.json()["nodes"]}
    assert "tag:general:general" not in node_ids


def test_conversation_tag_becomes_a_cluster_node_not_filtered_out(test_app):
    """Unlike `tenant:*`/`type:*`, `conversation:*` is the only thing that
    links a conversation's turns together in this graph (no memory-to-memory
    relationships exist between them) -- dropping it entirely would leave
    every turn as a fully disconnected card. It must survive as a `cluster`
    node instead, with a readable label (no local conversation row exists
    for "convo-999" in this test, so it falls back to a generic label
    rather than a raw tag string) and its member memory ids in
    `metadata.cluster_of`."""
    client = TestClient(test_app, headers={"x-uuid": "tenant-c"})
    resp = client.get("/memory-graph")
    assert resp.status_code == 200
    nodes = resp.json()["nodes"]
    cluster_nodes = [n for n in nodes if n["id"] == "tag:conversation:convo-999"]
    assert len(cluster_nodes) == 1
    cluster = cluster_nodes[0]
    assert cluster["node_type"] == "cluster"
    assert "convo-999" in cluster["label"]
    assert cluster["metadata"]["cluster_of"] == ["mem-c1"]

    edges = resp.json()["edges"]
    assert any(
        e["source_id"] == "mem-c1" and e["target_id"] == "tag:conversation:convo-999" for e in edges
    )


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

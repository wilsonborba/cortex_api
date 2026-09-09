from __future__ import annotations

import pytest
from fastapi.testclient import TestClient

from lib.presentation.api.app import create_app
from lib.presentation.api.deps import get_hippocampus_client
from lib.engine.retrieval.hippocampus import HippocampusClient


class FakeAsyncClient:
    def __init__(self, data=None, status_code=200, tags_by_id=None):
        self.data = data or {}
        self.status_code = status_code
        # Server-side-only tag associations: real Hippocampus filters by tag
        # but never echoes tags back in the response body (see `MemoryOut`),
        # so this mirrors that by keeping tags out of the returned items
        # entirely, just used here to decide which items a `tag` query
        # param matches.
        self.tags_by_id = tags_by_id or {}

    async def __aenter__(self):
        return self

    async def __aexit__(self, *args):
        pass

    async def get(self, url, params=None, headers=None):
        class FakeResponse:
            def __init__(self, d, s):
                self.d = d
                self.status_code = s
            def raise_for_status(self):
                pass
            def json(self):
                return self.d

        tag = (params or {}).get("tag")
        all_items = self.data.get("data", [])
        if tag is None:
            filtered = all_items
        else:
            filtered = [m for m in all_items if tag in self.tags_by_id.get(m.get("id"), [])]
        return FakeResponse({"data": filtered}, self.status_code)

    async def post(self, url, json=None, headers=None):
        class FakeResponse:
            def __init__(self, d, s):
                self.d = d
                self.status_code = s
            def raise_for_status(self):
                pass
            def json(self):
                return self.d
        return FakeResponse(self.data, self.status_code)


@pytest.fixture
def test_app():
    app = create_app()
    # Shaped exactly like Hippocampus's real `MemoryOut` response: no
    # `metadata`/`tags` fields at all (it never echoes those back on read,
    # only accepts them on write), so `conversation_id` must be recoverable
    # from `title` (`f"Turn in {conversation_id}"`, what
    # `record_conversation_turn` always writes) and the split between user
    # and assistant text must be recoverable from `content` alone.
    fake_memories = {
        "data": [
            {
                "id": "mem-1",
                "title": "Turn in convo-123",
                "content": "User: hello\n\nAssistant: hi there",
                "created_at": "2026-09-09T00:00:00Z",
                "updated_at": "2026-09-09T00:00:00Z",
            }
        ]
    }
    tags_by_id = {"mem-1": ["conversation:convo-123", "tenant:default"]}
    client = HippocampusClient(
        base_url="http://fake",
        client_factory=lambda: FakeAsyncClient(fake_memories, tags_by_id=tags_by_id),
    )
    app.dependency_overrides[get_hippocampus_client] = lambda: client
    return app


def test_list_conversations(test_app):
    client = TestClient(test_app)
    resp = client.get("/conversations")
    assert resp.status_code == 200
    data = resp.json()
    assert len(data) == 1
    assert data[0]["id"] == "convo-123"
    assert data[0]["message_count"] == 2


def test_get_conversation_detail(test_app):
    client = TestClient(test_app)
    resp = client.get("/conversations/convo-123")
    assert resp.status_code == 200
    data = resp.json()
    assert data["id"] == "convo-123"
    assert len(data["messages"]) == 2
    assert data["messages"][0]["role"] == "user"
    assert data["messages"][0]["content"] == "hello"
    assert data["messages"][1]["role"] == "assistant"
    assert data["messages"][1]["content"] == "hi there"


def test_create_conversation_is_empty_but_visible(test_app):
    client = TestClient(test_app)
    resp = client.post("/conversations", json={"id": "convo-crud-new", "title": "Fresh chat"})
    assert resp.status_code == 200
    data = resp.json()
    assert data["id"] == "convo-crud-new"
    assert data["title"] == "Fresh chat"
    assert data["message_count"] == 0

    detail = client.get("/conversations/convo-crud-new")
    assert detail.status_code == 200
    assert detail.json()["messages"] == []

    listing = client.get("/conversations")
    assert any(c["id"] == "convo-crud-new" for c in listing.json())


def test_rename_conversation(test_app):
    client = TestClient(test_app)
    client.post("/conversations", json={"id": "convo-crud-rename", "title": "Old title"})
    resp = client.patch("/conversations/convo-crud-rename", json={"title": "New title"})
    assert resp.status_code == 200
    assert resp.json()["title"] == "New title"


def test_pin_conversation(test_app):
    client = TestClient(test_app)
    client.post("/conversations", json={"id": "convo-crud-pin"})
    resp = client.patch("/conversations/convo-crud-pin", json={"is_pinned": True})
    assert resp.status_code == 200
    assert resp.json()["is_pinned"] is True

    resp = client.patch("/conversations/convo-crud-pin", json={"is_pinned": False})
    assert resp.json()["is_pinned"] is False


def test_delete_conversation_hides_it_from_listing(test_app):
    client = TestClient(test_app)
    client.post("/conversations", json={"id": "convo-crud-delete"})
    resp = client.delete("/conversations/convo-crud-delete")
    assert resp.status_code == 204

    listing = client.get("/conversations")
    assert all(c["id"] != "convo-crud-delete" for c in listing.json())


def test_another_tenant_cannot_rename_pin_or_delete_a_conversation_by_guessing_its_id(test_app):
    """Regression test for a real IDOR: rename/set_pinned/soft_delete used
    to look a conversation up by bare id (ignoring tenant_id) before
    deciding whether to update it, so a guessed/observed id from another
    tenant would get silently mutated instead of being treated as
    not-found and adopted fresh under the caller's own tenant."""
    owner = TestClient(test_app, headers={"x-uuid": "user-a"})
    intruder = TestClient(test_app, headers={"x-uuid": "user-b"})

    created = owner.post("/conversations", json={"id": "convo-shared-id", "title": "Owner's chat"})
    assert created.status_code == 200

    # The intruder renames/pins/deletes the same id: must never touch the
    # owner's row, only ever affect (or create) their own.
    intruder.patch("/conversations/convo-shared-id", json={"title": "Hijacked"})
    intruder.patch("/conversations/convo-shared-id", json={"is_pinned": True})
    intruder.delete("/conversations/convo-shared-id")

    owner_view = owner.get("/conversations/convo-shared-id")
    assert owner_view.status_code == 200
    assert owner_view.json()["title"] == "Owner's chat"
    assert owner_view.json()["is_pinned"] is False

    # The owner's conversation must still be listed (not soft-deleted by
    # the intruder's delete call).
    owner_listing = owner.get("/conversations")
    assert any(c["id"] == "convo-shared-id" for c in owner_listing.json())

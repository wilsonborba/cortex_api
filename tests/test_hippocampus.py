from __future__ import annotations

from typing import Any, Optional

import httpx
import pytest

from lib.engine.retrieval.hippocampus import HippocampusClient, MemoryChunk, format_memory_context


class _FakeResponse:
    def __init__(self, payload: Any, status_code: int = 200) -> None:
        self._payload = payload
        self.status_code = status_code

    def raise_for_status(self) -> None:
        if self.status_code >= 400:
            request = httpx.Request("GET", "http://fake")
            raise httpx.HTTPStatusError(
                "error", request=request, response=httpx.Response(self.status_code, request=request)
            )

    def json(self) -> Any:
        return self._payload


class _FakeAsyncClient:
    def __init__(self, payload: Any = None, status_code: int = 200, raise_error: Optional[Exception] = None) -> None:
        self._payload = payload
        self._status_code = status_code
        self._raise_error = raise_error

    async def __aenter__(self) -> "_FakeAsyncClient":
        return self

    async def __aexit__(self, *exc_info: Any) -> None:
        return None

    async def get(self, url: str, params: Optional[dict] = None, headers: Optional[dict] = None) -> _FakeResponse:
        if self._raise_error:
            raise self._raise_error
        return _FakeResponse(self._payload, self._status_code)

    async def post(self, url: str, json: Optional[dict] = None, headers: Optional[dict] = None) -> _FakeResponse:
        if self._raise_error:
            raise self._raise_error
        return _FakeResponse(self._payload, self._status_code)


# --- search_memory (POST /api/v1/recall) ------------------------------------------


@pytest.mark.asyncio
async def test_search_memory_parses_data_envelope():
    payload = {
        "data": [
            {
                "memory": {"id": "m1", "content": "cortex routes by tier", "memory_type": "fact"},
                "score": 0.91,
                "signals": {},
                "match_reasons": ["tag_match"],
            },
            {
                "memory": {"id": "m2", "content": "", "summary": "", "title": ""},
                "score": 0.5,
            },  # dropped: no content/summary/title to fall back to
        ]
    }
    client = HippocampusClient(
        base_url="http://localhost:8001", client_factory=lambda: _FakeAsyncClient(payload)
    )

    chunks = await client.search_memory("asodya_core", "how does routing work", limit=5)

    assert chunks == [
        MemoryChunk(
            id="m1",
            topic="asodya_core",
            content="cortex routes by tier",
            score=0.91,
            metadata={"memory_type": "fact"},
        )
    ]


@pytest.mark.asyncio
async def test_search_memory_falls_back_to_summary_then_title():
    payload = {
        "data": [
            {"memory": {"id": "m1", "content": "", "summary": "a summary", "title": "a title"}, "score": 0.1},
            {"memory": {"id": "m2", "content": "", "summary": "", "title": "just a title"}, "score": 0.2},
        ]
    }
    client = HippocampusClient(
        base_url="http://localhost:8001", client_factory=lambda: _FakeAsyncClient(payload)
    )

    chunks = await client.search_memory("asodya_core", "anything")

    assert [c.content for c in chunks] == ["a summary", "just a title"]


@pytest.mark.asyncio
async def test_search_memory_sends_query_as_tags_filter_and_auth_header():
    captured: dict[str, Any] = {}

    class _CapturingClient(_FakeAsyncClient):
        async def post(self, url: str, json: Optional[dict] = None, headers: Optional[dict] = None) -> _FakeResponse:
            captured["url"] = url
            captured["json"] = json
            captured["headers"] = headers
            return await super().post(url, json=json, headers=headers)

    client = HippocampusClient(
        base_url="http://localhost:8001",
        api_key="secret-key",
        client_factory=lambda: _CapturingClient({"data": []}),
    )

    await client.search_memory("asodya_core", "how does routing work", limit=5)

    assert captured["url"] == "http://localhost:8001/api/v1/recall"
    assert captured["json"] == {"query": "how does routing work", "tags": ["asodya_core"], "limit": 5}
    assert captured["headers"] == {"Authorization": "Bearer secret-key"}


@pytest.mark.asyncio
async def test_search_memory_no_api_key_sends_no_auth_header():
    captured: dict[str, Any] = {}

    class _CapturingClient(_FakeAsyncClient):
        async def post(self, url: str, json: Optional[dict] = None, headers: Optional[dict] = None) -> _FakeResponse:
            captured["headers"] = headers
            return await super().post(url, json=json, headers=headers)

    client = HippocampusClient(
        base_url="http://localhost:8001", client_factory=lambda: _CapturingClient({"data": []})
    )

    await client.search_memory("asodya_core", "anything")

    assert captured["headers"] == {}


@pytest.mark.asyncio
async def test_search_memory_degrades_gracefully_when_unreachable():
    client = HippocampusClient(
        base_url="http://localhost:8001",
        client_factory=lambda: _FakeAsyncClient(raise_error=httpx.ConnectError("connection refused")),
    )

    chunks = await client.search_memory("asodya_core", "anything")

    assert chunks == []


@pytest.mark.asyncio
async def test_search_memory_degrades_gracefully_on_malformed_json():
    class _BadJsonClient(_FakeAsyncClient):
        async def post(self, url: str, json: Optional[dict] = None, headers: Optional[dict] = None) -> _FakeResponse:
            response = _FakeResponse(None)
            response.json = lambda: (_ for _ in ()).throw(ValueError("bad json"))  # type: ignore[method-assign]
            return response

    client = HippocampusClient(base_url="http://localhost:8001", client_factory=_BadJsonClient)

    chunks = await client.search_memory("asodya_core", "anything")

    assert chunks == []


# --- store_event (POST /api/v1/memories) -------------------------------------------


@pytest.mark.asyncio
async def test_store_event_returns_true_on_success():
    client = HippocampusClient(
        base_url="http://localhost:8001", client_factory=lambda: _FakeAsyncClient({"data": {"id": "mem-1"}})
    )

    ok = await client.store_event("asodya_core", "last_seen", {"value": 1}, ttl_seconds=3600)

    assert ok is True


@pytest.mark.asyncio
async def test_store_event_maps_topic_key_value_to_memory_create_request():
    captured: dict[str, Any] = {}

    class _CapturingClient(_FakeAsyncClient):
        async def post(self, url: str, json: Optional[dict] = None, headers: Optional[dict] = None) -> _FakeResponse:
            captured["url"] = url
            captured["json"] = json
            return await super().post(url, json=json, headers=headers)

    client = HippocampusClient(
        base_url="http://localhost:8001", client_factory=lambda: _CapturingClient({"data": {}})
    )

    await client.store_event("asodya_core", "last_seen", {"value": 1}, ttl_seconds=3600)

    assert captured["url"] == "http://localhost:8001/api/v1/memories"
    body = captured["json"]
    assert body["title"] == "last_seen"
    assert body["tags"] == ["asodya_core"]
    assert body["metadata"] == {"key": "last_seen"}
    assert '"value": 1' in body["content"]  # non-string values are JSON-encoded
    assert "expires_at" in body  # ttl_seconds was given


@pytest.mark.asyncio
async def test_store_event_string_value_is_stored_as_is_and_no_ttl_means_no_expiry():
    captured: dict[str, Any] = {}

    class _CapturingClient(_FakeAsyncClient):
        async def post(self, url: str, json: Optional[dict] = None, headers: Optional[dict] = None) -> _FakeResponse:
            captured["json"] = json
            return await super().post(url, json=json, headers=headers)

    client = HippocampusClient(
        base_url="http://localhost:8001", client_factory=lambda: _CapturingClient({"data": {}})
    )

    await client.store_event("asodya_core", "last_seen", "a plain string value")

    assert captured["json"]["content"] == "a plain string value"
    assert "expires_at" not in captured["json"]


@pytest.mark.asyncio
async def test_store_event_returns_false_when_unreachable():
    client = HippocampusClient(
        base_url="http://localhost:8001",
        client_factory=lambda: _FakeAsyncClient(raise_error=httpx.ConnectTimeout("timed out")),
    )

    ok = await client.store_event("asodya_core", "last_seen", "value")

    assert ok is False


# --- resolve_document_context (GET /api/v1/memories/{id}[/document]) ---------------


class _MemoryWithDocumentClient(_FakeAsyncClient):
    """GET .../memories/{id} returns `memory_payload`; GET .../document
    returns `document_payload` (or 404s if None, simulating no linked doc)."""

    def __init__(self, memory_payload: Any, document_payload: Any = None) -> None:
        super().__init__()
        self._memory_payload = memory_payload
        self._document_payload = document_payload

    async def get(self, url: str, params: Optional[dict] = None, headers: Optional[dict] = None) -> _FakeResponse:
        if url.endswith("/document"):
            if self._document_payload is None:
                request = httpx.Request("GET", url)
                raise httpx.HTTPStatusError(
                    "not found", request=request, response=httpx.Response(404, request=request)
                )
            return _FakeResponse(self._document_payload)
        return _FakeResponse(self._memory_payload)


@pytest.mark.asyncio
async def test_resolve_document_context_falls_back_to_memory_content_when_no_linked_document():
    memory_payload = {"data": {"id": "mem-1", "title": "Asodya Architecture", "content": "the memory's own text", "memory_type": "note"}}
    client = HippocampusClient(
        base_url="http://localhost:8001",
        client_factory=lambda: _MemoryWithDocumentClient(memory_payload, document_payload=None),
    )

    context = await client.resolve_document_context("mem-1")

    assert context.available is True
    assert context.document_id == "mem-1"
    assert context.title == "Asodya Architecture"
    assert context.chunks == ["the memory's own text"]


@pytest.mark.asyncio
async def test_resolve_document_context_prefers_linked_document_content():
    memory_payload = {"data": {"id": "mem-1", "title": "Asodya Architecture", "content": "memory text"}}
    document_payload = {"data": {"content": "the full linked document text"}}
    client = HippocampusClient(
        base_url="http://localhost:8001",
        client_factory=lambda: _MemoryWithDocumentClient(memory_payload, document_payload=document_payload),
    )

    context = await client.resolve_document_context("mem-1")

    assert context.available is True
    assert context.chunks == ["the full linked document text"]


@pytest.mark.asyncio
async def test_resolve_document_context_degrades_gracefully_when_offline():
    client = HippocampusClient(
        base_url="http://localhost:8001",
        client_factory=lambda: _FakeAsyncClient(raise_error=httpx.ConnectError("connection refused")),
    )

    context = await client.resolve_document_context("mem-1")

    assert context.available is False
    assert context.document_id == "mem-1"
    assert context.chunks == []


# --- format_memory_context --------------------------------------------------------


def test_format_memory_context_joins_chunks_with_topic_headings():
    chunks = [
        MemoryChunk(id="1", topic="asodya_core", content="fact one"),
        MemoryChunk(id="2", topic="asodya_core", content="fact two"),
    ]

    markdown = format_memory_context(chunks)

    assert "fact one" in markdown
    assert "fact two" in markdown
    assert markdown.count("### asodya_core") == 2


def test_format_memory_context_empty_list_returns_empty_string():
    assert format_memory_context([]) == ""

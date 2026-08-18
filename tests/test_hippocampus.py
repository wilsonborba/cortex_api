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

    async def get(self, url: str, params: Optional[dict] = None) -> _FakeResponse:
        if self._raise_error:
            raise self._raise_error
        return _FakeResponse(self._payload, self._status_code)

    async def post(self, url: str, json: Optional[dict] = None) -> _FakeResponse:
        if self._raise_error:
            raise self._raise_error
        return _FakeResponse(self._payload, self._status_code)


# --- search_memory --------------------------------------------------------------


@pytest.mark.asyncio
async def test_search_memory_parses_results():
    payload = {
        "results": [
            {"id": "m1", "topic": "asodya_core", "content": "cortex routes by tier", "score": 0.91},
            {"id": "m2", "topic": "asodya_core", "content": "", "score": 0.5},  # dropped: empty content
        ]
    }
    client = HippocampusClient(
        base_url="http://localhost:8001", client_factory=lambda: _FakeAsyncClient(payload)
    )

    chunks = await client.search_memory("asodya_core", "how does routing work", limit=5)

    assert len(chunks) == 1
    assert chunks[0] == MemoryChunk(
        id="m1", topic="asodya_core", content="cortex routes by tier", score=0.91, metadata={}
    )


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
        async def get(self, url: str, params: Optional[dict] = None) -> _FakeResponse:
            response = _FakeResponse(None)
            response.json = lambda: (_ for _ in ()).throw(ValueError("bad json"))  # type: ignore[method-assign]
            return response

    client = HippocampusClient(base_url="http://localhost:8001", client_factory=_BadJsonClient)

    chunks = await client.search_memory("asodya_core", "anything")

    assert chunks == []


# --- store_event -----------------------------------------------------------------


@pytest.mark.asyncio
async def test_store_event_returns_true_on_success():
    client = HippocampusClient(
        base_url="http://localhost:8001", client_factory=lambda: _FakeAsyncClient({"ok": True})
    )

    ok = await client.store_event("asodya_core", "last_seen", {"value": 1}, ttl_seconds=3600)

    assert ok is True


@pytest.mark.asyncio
async def test_store_event_returns_false_when_unreachable():
    client = HippocampusClient(
        base_url="http://localhost:8001",
        client_factory=lambda: _FakeAsyncClient(raise_error=httpx.ConnectTimeout("timed out")),
    )

    ok = await client.store_event("asodya_core", "last_seen", "value")

    assert ok is False


# --- resolve_document_context -----------------------------------------------------


@pytest.mark.asyncio
async def test_resolve_document_context_parses_payload():
    payload = {
        "document_id": "doc-1",
        "title": "Asodya Architecture",
        "chunks": ["chunk one", "chunk two"],
        "source_url": "s3://bucket/doc-1.pdf",
    }
    client = HippocampusClient(
        base_url="http://localhost:8001", client_factory=lambda: _FakeAsyncClient(payload)
    )

    context = await client.resolve_document_context("doc-1")

    assert context.available is True
    assert context.title == "Asodya Architecture"
    assert context.chunks == ["chunk one", "chunk two"]
    assert context.source_url == "s3://bucket/doc-1.pdf"


@pytest.mark.asyncio
async def test_resolve_document_context_degrades_gracefully_when_offline():
    client = HippocampusClient(
        base_url="http://localhost:8001",
        client_factory=lambda: _FakeAsyncClient(raise_error=httpx.ConnectError("connection refused")),
    )

    context = await client.resolve_document_context("doc-1")

    assert context.available is False
    assert context.document_id == "doc-1"
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

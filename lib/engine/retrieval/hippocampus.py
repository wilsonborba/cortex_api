from __future__ import annotations

import json
from dataclasses import dataclass, field
from datetime import datetime, timedelta, timezone
from typing import Any, Callable, Dict, List, Optional

import httpx

from lib.core.logs import get_logger
from lib.core.settings import Settings, get_settings

logger = get_logger(__name__)


@dataclass(frozen=True)
class MemoryChunk:
    id: str
    topic: str
    content: str
    score: float = 0.0
    metadata: Dict[str, Any] = field(default_factory=dict)


@dataclass(frozen=True)
class DocumentContext:
    document_id: str
    title: str = ""
    chunks: List[str] = field(default_factory=list)
    source_url: Optional[str] = None
    metadata: Dict[str, Any] = field(default_factory=dict)
    available: bool = True


def format_memory_context(chunks: List[MemoryChunk]) -> str:
    """Renders retrieved memory chunks as a Markdown block ready for prompt injection."""
    if not chunks:
        return ""
    sections = [f"### {chunk.topic}\n\n{chunk.content.strip()}" for chunk in chunks]
    return "\n\n---\n\n".join(sections)


class HippocampusClient:
    """Async client for the real `hippocampus` service (github.com/wilsonborba/hippocampus).

    hippocampus's actual domain is memory/tag/entity, not topic/event: a
    memory has content, tags, entities, provenance, and is retrieved via
    `POST /api/v1/recall` (query + tag/entity filters, scored results), not
    a topic-keyed lookup. #7 built this client against a provisional,
    unverified contract before hippocampus existed as real code; this
    module (#18) was rewritten against its actual routes/schemas
    (`lib/presentation/api/routes/memories.py`, `recall.py`,
    `lib/presentation/api/schemas/memory.py`, `recall.py`), fetched
    directly from that repo's `feature/main` branch while building this.

    The public method names/signatures here (`search_memory(topic, ...)`,
    `store_event(topic, key, value, ...)`, `resolve_document_context(id)`)
    are kept exactly as #7 shipped them -- nothing in Cortex called them
    yet (checked: `Executor._gather_context` only calls `search_memory`),
    so this is a "fix what's under the hood" issue, not an interface
    change. `topic` is honestly a tag filter internally, not a real
    hippocampus concept; see each method's docstring for the mapping.

    Every method still degrades gracefully on any failure (unreachable,
    timeout, malformed response, non-2xx): a warning is logged and a
    neutral empty result is returned, same as #7's original contract.
    hippocampus's own error envelope (`{"error": {"code", "message",
    "details"}}`) isn't parsed here -- graceful degradation doesn't need to
    know *why* a call failed, only that it did.
    """

    def __init__(
        self,
        base_url: str,
        timeout: float = 10.0,
        api_key: Optional[str] = None,
        client_factory: Optional[Callable[[], httpx.AsyncClient]] = None,
    ) -> None:
        self._base_url = base_url.rstrip("/")
        self._timeout = timeout
        self._api_key = api_key
        self._client_factory = client_factory or (lambda: httpx.AsyncClient(timeout=self._timeout))

    def _headers(self) -> Dict[str, str]:
        # hippocampus's auth is optional bearer-key (disabled entirely when
        # the service has no api_keys configured, e.g. local/LAN dev) --
        # only send the header when a key is actually configured here.
        return {"Authorization": f"Bearer {self._api_key}"} if self._api_key else {}

    async def search_memory(
        self, topic: str, query: str, limit: int = 5, tenant_id: Optional[str] = None
    ) -> List[MemoryChunk]:
        tags = [topic] if topic else []
        if tenant_id:
            tags.append(f"tenant:{tenant_id}")
        try:
            async with self._client_factory() as client:
                response = await client.post(
                    f"{self._base_url}/api/v1/recall",
                    # `workspace_id` (not just the `tenant:` tag) is what
                    # Hippocampus actually enforces at the repository level
                    # (see MemoryRepository.list/get); pass both so recall
                    # is scoped by the real, enforced field, tags stay for
                    # the topic filter only.
                    json={"query": query, "tags": tags, "workspace_id": tenant_id, "limit": limit},
                    headers=self._headers(),
                )
                response.raise_for_status()
                payload = response.json()
        except (httpx.HTTPError, ValueError) as exc:
            logger.warning("hippocampus.search_memory unavailable (topic=%r): %s", topic, exc)
            return []

        results = payload.get("data", []) if isinstance(payload, dict) else []
        chunks: List[MemoryChunk] = []
        for item in results:
            memory = item.get("memory") or {}
            content = memory.get("content") or memory.get("summary") or memory.get("title") or ""
            if not content:
                continue
            chunks.append(
                MemoryChunk(
                    id=str(memory.get("id", "")),
                    topic=topic,  # queried tag, not something hippocampus returns per-result
                    content=content,
                    score=float(item.get("score", 0.0) or 0.0),
                    metadata={"memory_type": memory.get("memory_type")},
                )
            )
        return chunks

    async def store_event(
        self, topic: str, key: str, value: Any, ttl_seconds: Optional[int] = None, tenant_id: Optional[str] = None
    ) -> bool:
        content = value if isinstance(value, str) else json.dumps(value, default=str)
        tags = [topic] if topic else []
        if tenant_id:
            tags.append(f"tenant:{tenant_id}")
        metadata = {"key": key}
        if tenant_id:
            metadata["tenant_id"] = tenant_id
        body: Dict[str, Any] = {
            "content": content,
            "title": key,
            "tags": tags,
            "metadata": metadata,
            "workspace_id": tenant_id,
        }
        if ttl_seconds is not None:
            body["expires_at"] = (datetime.now(timezone.utc) + timedelta(seconds=ttl_seconds)).isoformat()

        try:
            async with self._client_factory() as client:
                response = await client.post(
                    f"{self._base_url}/api/v1/memories", json=body, headers=self._headers()
                )
                response.raise_for_status()
        except httpx.HTTPError as exc:
            logger.warning("hippocampus.store_event unavailable (topic=%r, key=%r): %s", topic, key, exc)
            return False
        return True

    async def resolve_document_context(self, document_id: str) -> DocumentContext:
        """`document_id` is actually a *memory* id -- hippocampus's real
        addressable unit -- kept as this parameter's existing name for
        Executor-side compatibility (#7). Fetches the memory itself
        (`GET /api/v1/memories/{id}`) and, best-effort, its linked source
        document (`GET /api/v1/memories/{id}/document`); the memory's own
        content is the fallback when there's no separate linked document.
        """
        try:
            async with self._client_factory() as client:
                memory_response = await client.get(
                    f"{self._base_url}/api/v1/memories/{document_id}", headers=self._headers()
                )
                memory_response.raise_for_status()
                memory = memory_response.json().get("data") or {}

                document_text: Optional[str] = None
                try:
                    doc_response = await client.get(
                        f"{self._base_url}/api/v1/memories/{document_id}/document", headers=self._headers()
                    )
                    doc_response.raise_for_status()
                    document = doc_response.json().get("data")
                    if isinstance(document, dict):
                        document_text = document.get("content") or document.get("text")
                except httpx.HTTPError:
                    pass  # no linked document, or the sub-resource errored: fall back to the memory's own content
        except (httpx.HTTPError, ValueError) as exc:
            logger.warning("hippocampus.resolve_document_context unavailable (document_id=%r): %s", document_id, exc)
            return DocumentContext(document_id=document_id, available=False)

        chunks = [document_text] if document_text else ([memory["content"]] if memory.get("content") else [])
        return DocumentContext(
            document_id=memory.get("id", document_id),
            title=memory.get("title") or "",
            chunks=chunks,
            metadata={"memory_type": memory.get("memory_type")} if memory else {},
            available=True,
        )


def build_default_hippocampus_client(settings: Optional[Settings] = None) -> HippocampusClient:
    settings = settings or get_settings()
    return HippocampusClient(
        base_url=settings.hippocampus_url,
        timeout=settings.hippocampus_timeout_seconds,
        api_key=settings.hippocampus_api_key,
    )

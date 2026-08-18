from __future__ import annotations

from dataclasses import dataclass, field
from typing import Any, Callable, List, Optional

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
    metadata: dict[str, Any] = field(default_factory=dict)


@dataclass(frozen=True)
class DocumentContext:
    document_id: str
    title: str = ""
    chunks: List[str] = field(default_factory=list)
    source_url: Optional[str] = None
    metadata: dict[str, Any] = field(default_factory=dict)
    available: bool = True


def format_memory_context(chunks: List[MemoryChunk]) -> str:
    """Renders retrieved memory chunks as a Markdown block ready for prompt injection."""
    if not chunks:
        return ""
    sections = [f"### {chunk.topic}\n\n{chunk.content.strip()}" for chunk in chunks]
    return "\n\n---\n\n".join(sections)


class HippocampusClient:
    """Async client for the external `hippocampus` memory/document microservice.

    hippocampus owns three memory layers Cortex never touches directly:
    session/topic memory (Redis), semantic memory (local Ollama embeddings),
    and document storage (S3). This client only talks to its REST API.

    Every method degrades gracefully: an unreachable service, a timeout, or a
    malformed response logs a warning and returns a neutral empty result
    instead of raising. A request must never fail just because long-term
    memory happens to be down.

    Response shapes below are this client's working assumption for
    hippocampus's REST API (`GET /memory/search`, `POST /memory/event`,
    `GET /documents/{id}`); parsing is defensive (`.get(..., default)`
    throughout) so small schema drift degrades rather than crashes, but it's
    worth re-checking against hippocampus's actual contract once that
    service exists.
    """

    def __init__(
        self,
        base_url: str,
        timeout: float = 10.0,
        client_factory: Optional[Callable[[], httpx.AsyncClient]] = None,
    ) -> None:
        self._base_url = base_url.rstrip("/")
        self._timeout = timeout
        self._client_factory = client_factory or (lambda: httpx.AsyncClient(timeout=self._timeout))

    async def search_memory(self, topic: str, query: str, limit: int = 5) -> List[MemoryChunk]:
        try:
            async with self._client_factory() as client:
                response = await client.get(
                    f"{self._base_url}/memory/search",
                    params={"topic": topic, "query": query, "limit": limit},
                )
                response.raise_for_status()
                payload = response.json()
        except (httpx.HTTPError, ValueError) as exc:
            logger.warning("hippocampus.search_memory unavailable (topic=%r): %s", topic, exc)
            return []

        raw_results = payload.get("results", []) if isinstance(payload, dict) else []
        return [
            MemoryChunk(
                id=str(r.get("id", "")),
                topic=r.get("topic", topic),
                content=r.get("content", ""),
                score=float(r.get("score", 0.0) or 0.0),
                metadata=r.get("metadata") or {},
            )
            for r in raw_results
            if r.get("content")
        ]

    async def store_event(
        self, topic: str, key: str, value: Any, ttl_seconds: Optional[int] = None
    ) -> bool:
        try:
            async with self._client_factory() as client:
                response = await client.post(
                    f"{self._base_url}/memory/event",
                    json={"topic": topic, "key": key, "value": value, "ttl_seconds": ttl_seconds},
                )
                response.raise_for_status()
        except httpx.HTTPError as exc:
            logger.warning("hippocampus.store_event unavailable (topic=%r, key=%r): %s", topic, key, exc)
            return False
        return True

    async def resolve_document_context(self, document_id: str) -> DocumentContext:
        try:
            async with self._client_factory() as client:
                response = await client.get(f"{self._base_url}/documents/{document_id}")
                response.raise_for_status()
                payload = response.json()
        except (httpx.HTTPError, ValueError) as exc:
            logger.warning("hippocampus.resolve_document_context unavailable (document_id=%r): %s", document_id, exc)
            return DocumentContext(document_id=document_id, available=False)

        if not isinstance(payload, dict):
            return DocumentContext(document_id=document_id, available=False)

        return DocumentContext(
            document_id=payload.get("document_id", document_id),
            title=payload.get("title", ""),
            chunks=list(payload.get("chunks", [])),
            source_url=payload.get("source_url"),
            metadata=payload.get("metadata") or {},
            available=True,
        )


def build_default_hippocampus_client(settings: Optional[Settings] = None) -> HippocampusClient:
    settings = settings or get_settings()
    return HippocampusClient(
        base_url=settings.hippocampus_url, timeout=settings.hippocampus_timeout_seconds
    )

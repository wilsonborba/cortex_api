from __future__ import annotations

from typing import Any, Callable, Optional

import httpx
from duckduckgo_search import DDGS
from duckduckgo_search.exceptions import DuckDuckGoSearchException

from lib.engine.retrieval.base import RetrievalError, SearchResult


class DuckDuckGoSearchProvider:
    """Free, zero-API-key search via the `duckduckgo-search` library."""

    provider = "duckduckgo"

    def __init__(
        self,
        region: str = "wt-wt",
        safesearch: str = "moderate",
        timeout: float = 10.0,
        client_factory: Optional[Callable[[], DDGS]] = None,
    ) -> None:
        self._region = region
        self._safesearch = safesearch
        self._timeout = timeout
        self._client_factory = client_factory or (lambda: DDGS(timeout=int(self._timeout)))

    def search(self, query: str, max_results: int = 5) -> list[SearchResult]:
        try:
            with self._client_factory() as ddgs:
                raw = ddgs.text(
                    query,
                    region=self._region,
                    safesearch=self._safesearch,
                    max_results=max_results,
                )
        except DuckDuckGoSearchException as exc:
            raise RetrievalError(self.provider, f"DuckDuckGo search failed: {exc}") from exc

        return [
            SearchResult(title=r.get("title", ""), url=r["href"], snippet=r.get("body", ""))
            for r in (raw or [])
            if r.get("href")
        ]


class SearXNGSearchProvider:
    """Meta-search via a self-hosted SearXNG instance's JSON API."""

    provider = "searxng"

    def __init__(
        self,
        base_url: str,
        timeout: float = 10.0,
        client_factory: Optional[Callable[[], httpx.Client]] = None,
    ) -> None:
        self._base_url = base_url.rstrip("/")
        self._timeout = timeout
        self._client_factory = client_factory or (lambda: httpx.Client(timeout=self._timeout))

    def search(self, query: str, max_results: int = 5) -> list[SearchResult]:
        try:
            with self._client_factory() as client:
                response = client.get(
                    f"{self._base_url}/search", params={"q": query, "format": "json"}
                )
                response.raise_for_status()
                payload = response.json()
        except (httpx.HTTPError, ValueError) as exc:
            raise RetrievalError(self.provider, f"SearXNG unreachable at {self._base_url}: {exc}") from exc

        results: list[dict[str, Any]] = payload.get("results", []) if isinstance(payload, dict) else []
        return [
            SearchResult(title=r.get("title", ""), url=r["url"], snippet=r.get("content", ""))
            for r in results[:max_results]
            if r.get("url")
        ]

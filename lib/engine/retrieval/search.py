from __future__ import annotations

import urllib.parse
from typing import Any, Callable, Optional

import httpx
import lxml.html
from duckduckgo_search import DDGS
from duckduckgo_search.exceptions import DuckDuckGoSearchException

from lib.engine.retrieval.base import RetrievalError, SearchResult


class DuckDuckGoHtmlProvider:
    """Search DuckDuckGo by scraping its plain HTML results endpoint with
    ordinary browser-like headers, instead of going through the
    duckduckgo-search library's browser-impersonation client (`primp`).

    That client's fingerprint can get flagged under sustained automated use,
    at which point DDG stops returning results (a silent redirect, not an
    error) for every request from it - a plain httpx GET, with no
    impersonation to detect, is a distinct-enough fingerprint that it often
    still gets through when the impersonated client is throttled. Listed
    ahead of `DuckDuckGoSearchProvider` in the default chain since it's the
    more likely one to still work; `WebRetrievalService._search` already
    falls through to the next provider on an empty result, so both are kept
    rather than one replacing the other.
    """

    provider = "duckduckgo_html"
    _ENDPOINT = "https://html.duckduckgo.com/html/"
    _HEADERS = {
        "User-Agent": (
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 "
            "(KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36"
        ),
        "Accept": "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
        "Accept-Language": "en-US,en;q=0.9",
    }

    def __init__(
        self,
        timeout: float = 10.0,
        client_factory: Optional[Callable[[], httpx.Client]] = None,
    ) -> None:
        self._timeout = timeout
        self._client_factory = client_factory or (
            lambda: httpx.Client(timeout=self._timeout, headers=self._HEADERS, follow_redirects=True)
        )

    def search(self, query: str, max_results: int = 5) -> list[SearchResult]:
        try:
            with self._client_factory() as client:
                response = client.post(self._ENDPOINT, data={"q": query})
                response.raise_for_status()
        except httpx.HTTPError as exc:
            raise RetrievalError(self.provider, f"DuckDuckGo HTML endpoint unreachable: {exc}") from exc

        return self._parse(response.text)[:max_results]

    @staticmethod
    def _parse(html: str) -> list[SearchResult]:
        try:
            tree = lxml.html.fromstring(html)
        except Exception:
            return []

        results: list[SearchResult] = []
        for node in tree.cssselect("div.result"):
            links = node.cssselect("a.result__a")
            if not links:
                continue
            title = links[0].text_content().strip()
            url = DuckDuckGoHtmlProvider._resolve_url(links[0].get("href", ""))
            if not url:
                continue
            snippet_nodes = node.cssselect(".result__snippet")
            snippet = snippet_nodes[0].text_content().strip() if snippet_nodes else ""
            results.append(SearchResult(title=title, url=url, snippet=snippet))
        return results

    @staticmethod
    def _resolve_url(href: str) -> str:
        """DDG's HTML result links are redirect wrappers
        (`//duckduckgo.com/l/?uddg=<url-encoded-real-url>&...`), not the
        real destination - unwrap it so citations point at the actual page.
        """
        if not href:
            return ""
        if "uddg=" not in href:
            return href
        parsed = urllib.parse.urlparse(href if href.startswith("http") else f"https:{href}")
        real = urllib.parse.parse_qs(parsed.query).get("uddg", [""])[0]
        return urllib.parse.unquote(real) if real else href


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

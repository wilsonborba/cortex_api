from __future__ import annotations

from dataclasses import dataclass, field
import re
from typing import Any, Dict, Iterable, List, Optional

from lib.core.settings import Settings, get_settings
from lib.engine.retrieval.base import ContentScraper, RetrievalError, ScrapedPage, SearchProvider, SearchResult
from lib.engine.retrieval.scraper import Crawl4AIScraper, TrafilaturaScraper
from lib.engine.retrieval.search import DuckDuckGoSearchProvider, SearXNGSearchProvider


@dataclass(frozen=True)
class WebContextResult:
    """A ready-to-inject Markdown context block plus the sources it came from.

    `items` (issue #16) is the same data as `markdown`, kept structured
    instead of pre-rendered: `[{"title", "url", "content"}, ...]`, one per
    result, so the Executor can encode it as TOON/JSON per the destination
    model's preference instead of always Markdown. `markdown`/`sources`
    still work exactly as before -- this is purely additive.
    """

    query: str
    markdown: str
    sources: List[str] = field(default_factory=list)
    items: List[Dict[str, Any]] = field(default_factory=list)


class WebRetrievalService:
    """Search + scrape + format: the whole `--web` pipeline in one call.

    Search providers are tried in order until one returns results (SearXNG,
    if configured, then DuckDuckGo). For each result, scrapers are tried in
    order (Trafilatura first: it's fast; Crawl4AI as the JS-heavy fallback).
    A result that can't be scraped by anything still makes it into the
    context block as its search snippet, rather than being dropped.
    """

    def __init__(
        self,
        search_providers: Iterable[SearchProvider],
        scrapers: Iterable[ContentScraper],
        max_results: int = 5,
    ) -> None:
        self._search_providers = list(search_providers)
        self._scrapers = list(scrapers)
        self._max_results = max_results

    def gather_context(self, query: str, max_results: Optional[int] = None) -> WebContextResult:
        limit = max_results or self._max_results
        results = self._search(query, limit)
        if not results:
            return WebContextResult(query=query, markdown="", sources=[])

        sections: List[str] = []
        sources: List[str] = []
        items: List[Dict[str, Any]] = []
        collected: List[tuple[SearchResult, str, str]] = []
        for result in results:
            page = self._scrape(result.url)
            heading = result.title or (page.title if page else "") or result.url
            body = (page.markdown if page and page.success and page.markdown.strip() else result.snippet).strip()
            collected.append((result, heading, body))

        for result, heading, body in self._filter_context_items(query, collected):
            sections.append(f"### {heading}\nSource: {result.url}\n\n{body}")
            sources.append(result.url)
            items.append({"title": heading, "url": result.url, "content": body})

        if not sections:
            return WebContextResult(query=query, markdown="", sources=[], items=[])

        markdown = "\n\n---\n\n".join(sections)
        return WebContextResult(query=query, markdown=markdown, sources=sources, items=items)

    def _search(self, query: str, limit: int) -> List[SearchResult]:
        for provider in self._search_providers:
            try:
                results = provider.search(query, max_results=limit)
            except RetrievalError:
                continue
            if results:
                return results
        return []

    def _scrape(self, url: str) -> Optional[ScrapedPage]:
        for scraper in self._scrapers:
            page = scraper.scrape(url)
            if page.success:
                return page
        return None

    def _filter_context_items(
        self,
        query: str,
        items: List[tuple[SearchResult, str, str]],
    ) -> List[tuple[SearchResult, str, str]]:
        query_terms = _tokenize(query)
        ranked: List[tuple[float, tuple[SearchResult, str, str]]] = []
        seen_urls: set[str] = set()
        seen_bodies: set[str] = set()

        for item in items:
            result, heading, body = item
            if result.url in seen_urls:
                continue
            signature = _content_signature(body)
            if signature in seen_bodies:
                continue
            score = _overlap_score(query_terms, _tokenize(f"{heading} {body}"))
            if score <= 0.0:
                continue
            ranked.append((score, item))
            seen_urls.add(result.url)
            seen_bodies.add(signature)

        ranked.sort(key=lambda pair: pair[0], reverse=True)
        return [item for _, item in ranked[: self._max_results]]


def build_default_web_retrieval_service(settings: Optional[Settings] = None) -> WebRetrievalService:
    """Wires the real search providers/scrapers. SearXNG only joins the chain
    when a base URL is actually configured (it's self-hosted, opt-in)."""
    settings = settings or get_settings()

    search_providers: List[SearchProvider] = []
    if settings.searxng_base_url:
        search_providers.append(
            SearXNGSearchProvider(
                base_url=settings.searxng_base_url, timeout=settings.web_retrieval_timeout_seconds
            )
        )
    search_providers.append(DuckDuckGoSearchProvider(timeout=settings.web_retrieval_timeout_seconds))

    return WebRetrievalService(
        search_providers=search_providers,
        scrapers=[TrafilaturaScraper(), Crawl4AIScraper()],
        max_results=settings.web_search_max_results,
    )


def _tokenize(text: str) -> set[str]:
    return {token for token in re.findall(r"[a-z0-9]+", (text or "").lower()) if len(token) >= 3}


def _overlap_score(query_terms: set[str], item_terms: set[str]) -> float:
    if not query_terms or not item_terms:
        return 0.0
    overlap = query_terms & item_terms
    return len(overlap) / len(query_terms)


def _content_signature(text: str) -> str:
    terms = sorted(_tokenize(text))
    return " ".join(terms[:24])

from __future__ import annotations

import sys
from typing import Any, List, Optional

import httpx
import pytest
from duckduckgo_search.exceptions import DuckDuckGoSearchException

from lib.engine.retrieval.base import RetrievalError, ScrapedPage, SearchResult
from lib.engine.retrieval.scraper import Crawl4AIScraper, TrafilaturaScraper
from lib.engine.retrieval.search import DuckDuckGoSearchProvider, SearXNGSearchProvider
from lib.engine.retrieval.service import WebRetrievalService


# --- DuckDuckGoSearchProvider --------------------------------------------------


class _FakeDDGS:
    def __init__(self, results: Optional[List[dict]] = None, error: Optional[Exception] = None) -> None:
        self._results = results or []
        self._error = error

    def __enter__(self) -> "_FakeDDGS":
        return self

    def __exit__(self, *exc_info: Any) -> None:
        return None

    def text(self, keywords: str, **kwargs: Any) -> List[dict]:
        if self._error:
            raise self._error
        return self._results


def test_duckduckgo_provider_parses_results():
    raw = [
        {"title": "Python", "href": "https://python.org", "body": "The Python language"},
        {"title": "No href", "body": "should be dropped"},
    ]
    provider = DuckDuckGoSearchProvider(client_factory=lambda: _FakeDDGS(results=raw))

    results = provider.search("python", max_results=5)

    assert len(results) == 1
    assert results[0] == SearchResult(title="Python", url="https://python.org", snippet="The Python language")


def test_duckduckgo_provider_raises_retrieval_error_on_failure():
    provider = DuckDuckGoSearchProvider(
        client_factory=lambda: _FakeDDGS(error=DuckDuckGoSearchException("202 Ratelimit"))
    )

    with pytest.raises(RetrievalError) as exc_info:
        provider.search("python")
    assert exc_info.value.source == "duckduckgo"


# --- SearXNGSearchProvider ------------------------------------------------------


class _FakeSearXNGResponse:
    def __init__(self, payload: Any, status_code: int = 200) -> None:
        self._payload = payload
        self.status_code = status_code

    def raise_for_status(self) -> None:
        if self.status_code >= 400:
            request = httpx.Request("GET", "http://fake")
            raise httpx.HTTPStatusError("error", request=request, response=httpx.Response(self.status_code, request=request))

    def json(self) -> Any:
        return self._payload


class _FakeSearXNGClient:
    def __init__(self, payload: Any) -> None:
        self._payload = payload

    def __enter__(self) -> "_FakeSearXNGClient":
        return self

    def __exit__(self, *exc_info: Any) -> None:
        return None

    def get(self, url: str, params: dict) -> _FakeSearXNGResponse:
        return _FakeSearXNGResponse(self._payload)


def test_searxng_provider_parses_results():
    payload = {
        "results": [
            {"title": "Python", "url": "https://python.org", "content": "The Python language"},
            {"title": "No url", "content": "should be dropped"},
        ]
    }
    provider = SearXNGSearchProvider(
        base_url="http://localhost:8080", client_factory=lambda: _FakeSearXNGClient(payload)
    )

    results = provider.search("python", max_results=5)

    assert results == [SearchResult(title="Python", url="https://python.org", snippet="The Python language")]


def test_searxng_provider_raises_on_unreachable():
    def _broken_factory() -> Any:
        raise httpx.ConnectError("connection refused")

    provider = SearXNGSearchProvider(base_url="http://localhost:8080", client_factory=_broken_factory)

    with pytest.raises(RetrievalError) as exc_info:
        provider.search("python")
    assert exc_info.value.source == "searxng"


# --- TrafilaturaScraper ----------------------------------------------------------


def test_trafilatura_scraper_returns_markdown():
    scraper = TrafilaturaScraper(
        fetch_url=lambda url: "<html><body><p>hello world</p></body></html>",
        extract=lambda content, **kwargs: "hello world",
    )

    page = scraper.scrape("https://example.com")

    assert page.success is True
    assert page.markdown == "hello world"


def test_trafilatura_scraper_handles_download_failure():
    scraper = TrafilaturaScraper(fetch_url=lambda url: None, extract=lambda content, **kwargs: None)

    page = scraper.scrape("https://example.com")

    assert page.success is False
    assert "download" in page.error.lower()


def test_trafilatura_scraper_handles_empty_extraction():
    scraper = TrafilaturaScraper(fetch_url=lambda url: "<html></html>", extract=lambda content, **kwargs: "")

    page = scraper.scrape("https://example.com")

    assert page.success is False
    assert page.error == "No extractable content"


# --- Crawl4AIScraper -----------------------------------------------------------


class _FakeCrawlResult:
    def __init__(self, success: bool, markdown: str = "", error_message: str = "", metadata: Optional[dict] = None) -> None:
        self.success = success
        self.markdown = markdown
        self.error_message = error_message
        self.metadata = metadata or {}


class _FakeAsyncCrawler:
    def __init__(self, result: _FakeCrawlResult) -> None:
        self._result = result

    async def __aenter__(self) -> "_FakeAsyncCrawler":
        return self

    async def __aexit__(self, *exc_info: Any) -> None:
        return None

    async def arun(self, url: str) -> _FakeCrawlResult:
        return self._result


def test_crawl4ai_scraper_returns_markdown_from_injected_crawler():
    result = _FakeCrawlResult(success=True, markdown="# Title\n\ncontent", metadata={"title": "Title"})
    scraper = Crawl4AIScraper(crawler_factory=lambda: _FakeAsyncCrawler(result))

    page = scraper.scrape("https://example.com")

    assert page.success is True
    assert page.markdown == "# Title\n\ncontent"
    assert page.title == "Title"


def test_crawl4ai_scraper_handles_failed_crawl():
    result = _FakeCrawlResult(success=False, error_message="blocked by robots.txt")
    scraper = Crawl4AIScraper(crawler_factory=lambda: _FakeAsyncCrawler(result))

    page = scraper.scrape("https://example.com")

    assert page.success is False
    assert page.error == "blocked by robots.txt"


def test_crawl4ai_scraper_reports_missing_dependency(monkeypatch: pytest.MonkeyPatch):
    monkeypatch.setitem(sys.modules, "crawl4ai", None)  # forces `from crawl4ai import ...` to raise ImportError
    scraper = Crawl4AIScraper()

    page = scraper.scrape("https://example.com")

    assert page.success is False
    assert "not installed" in page.error.lower()


# --- WebRetrievalService ---------------------------------------------------------


class _StaticSearchProvider:
    provider = "static"

    def __init__(self, results: Optional[List[SearchResult]] = None, error: Optional[Exception] = None) -> None:
        self._results = results or []
        self._error = error

    def search(self, query: str, max_results: int = 5) -> List[SearchResult]:
        if self._error:
            raise self._error
        return self._results


class _StaticScraper:
    scraper = "static"

    def __init__(self, page: Optional[ScrapedPage] = None) -> None:
        self._page = page or ScrapedPage(url="", title="", markdown="", success=False, error="no page configured")

    def scrape(self, url: str) -> ScrapedPage:
        return ScrapedPage(
            url=url, title=self._page.title, markdown=self._page.markdown,
            success=self._page.success, error=self._page.error,
        )


def test_gather_context_formats_scraped_markdown_with_source():
    provider = _StaticSearchProvider(results=[SearchResult(title="Python", url="https://python.org", snippet="snippet")])
    scraper = _StaticScraper(page=ScrapedPage(url="", title="", markdown="full page content", success=True))
    service = WebRetrievalService(search_providers=[provider], scrapers=[scraper])

    context = service.gather_context("python")

    assert "full page content" in context.markdown
    assert "https://python.org" in context.markdown
    assert context.sources == ["https://python.org"]


def test_gather_context_falls_back_to_snippet_when_scrape_fails():
    provider = _StaticSearchProvider(results=[SearchResult(title="Python", url="https://python.org", snippet="a useful snippet")])
    scraper = _StaticScraper(page=ScrapedPage(url="", title="", markdown="", success=False, error="blocked"))
    service = WebRetrievalService(search_providers=[provider], scrapers=[scraper])

    context = service.gather_context("python")

    assert "a useful snippet" in context.markdown
    assert context.sources == ["https://python.org"]


def test_gather_context_tries_next_search_provider_on_failure():
    failing = _StaticSearchProvider(error=RetrievalError("static", "down"))
    working = _StaticSearchProvider(results=[SearchResult(title="Python", url="https://python.org", snippet="s")])
    scraper = _StaticScraper(page=ScrapedPage(url="", title="", markdown="content", success=True))
    service = WebRetrievalService(search_providers=[failing, working], scrapers=[scraper])

    context = service.gather_context("python")

    assert context.sources == ["https://python.org"]


def test_gather_context_returns_empty_when_no_provider_has_results():
    service = WebRetrievalService(search_providers=[_StaticSearchProvider(results=[])], scrapers=[])

    context = service.gather_context("python")

    assert context.markdown == ""
    assert context.sources == []


def test_gather_context_discards_irrelevant_results():
    provider = _StaticSearchProvider(
        results=[
            SearchResult(title="Python sqlite", url="https://good.example", snippet="python sqlite usage"),
            SearchResult(title="Football scores", url="https://bad.example", snippet="latest football match"),
        ]
    )
    scraper = _StaticScraper(page=ScrapedPage(url="", title="", markdown="python sqlite tutorial", success=True))
    service = WebRetrievalService(search_providers=[provider], scrapers=[scraper])

    context = service.gather_context("python sqlite")

    assert context.sources == ["https://good.example"]
    assert "bad.example" not in context.markdown


def test_gather_context_deduplicates_near_identical_pages():
    provider = _StaticSearchProvider(
        results=[
            SearchResult(title="Python", url="https://a.example", snippet="python sqlite basics"),
            SearchResult(title="Python duplicate", url="https://b.example", snippet="python sqlite basics"),
        ]
    )
    scraper = _StaticScraper(page=ScrapedPage(url="", title="", markdown="python sqlite basics", success=True))
    service = WebRetrievalService(search_providers=[provider], scrapers=[scraper])

    context = service.gather_context("python sqlite")

    assert len(context.sources) == 1

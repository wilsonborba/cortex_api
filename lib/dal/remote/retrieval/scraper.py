from __future__ import annotations

import asyncio
from typing import Any, Callable, Optional

import trafilatura

from lib.engine.retrieval.base import ScrapedPage


class TrafilaturaScraper:
    """Fast (<50ms typical) static-HTML extractor: strips nav/ads/boilerplate."""

    scraper = "trafilatura"

    def __init__(
        self,
        fetch_url: Optional[Callable[[str], Optional[str]]] = None,
        extract: Optional[Callable[..., Optional[str]]] = None,
    ) -> None:
        self._fetch_url = fetch_url or trafilatura.fetch_url
        self._extract = extract or trafilatura.extract

    def scrape(self, url: str) -> ScrapedPage:
        try:
            downloaded = self._fetch_url(url)
        except Exception as exc:  # trafilatura's downloader can raise a variety of errors
            return ScrapedPage(url=url, title="", markdown="", success=False, error=str(exc))

        if not downloaded:
            return ScrapedPage(url=url, title="", markdown="", success=False, error="Could not download page")

        try:
            markdown = self._extract(
                downloaded,
                url=url,
                output_format="markdown",
                include_tables=True,
                include_links=False,
            )
        except Exception as exc:
            return ScrapedPage(url=url, title="", markdown="", success=False, error=str(exc))

        if not markdown or not markdown.strip():
            return ScrapedPage(url=url, title="", markdown="", success=False, error="No extractable content")

        return ScrapedPage(url=url, title="", markdown=markdown.strip(), success=True)


class Crawl4AIScraper:
    """Fallback for JS-heavy sites/rich tables Trafilatura can't render statically.

    `crawl4ai` is an optional dependency (`cortex[crawler]`) and drives a real
    headless browser, so this is meant as the second link in a scraper chain,
    not the default.
    """

    scraper = "crawl4ai"

    def __init__(self, crawler_factory: Optional[Callable[[], Any]] = None) -> None:
        self._crawler_factory = crawler_factory

    def scrape(self, url: str) -> ScrapedPage:
        try:
            return asyncio.run(self._scrape_async(url))
        except RuntimeError as exc:
            # e.g. invoked from inside an already-running event loop
            return ScrapedPage(url=url, title="", markdown="", success=False, error=str(exc))

    async def _scrape_async(self, url: str) -> ScrapedPage:
        crawler_factory = self._crawler_factory
        if crawler_factory is None:
            try:
                from crawl4ai import AsyncWebCrawler
            except ImportError as exc:
                return ScrapedPage(
                    url=url, title="", markdown="", success=False, error=f"crawl4ai not installed: {exc}"
                )
            crawler_factory = AsyncWebCrawler

        try:
            async with crawler_factory() as crawler:
                result = await crawler.arun(url)
        except Exception as exc:
            return ScrapedPage(url=url, title="", markdown="", success=False, error=str(exc))

        if not getattr(result, "success", False):
            error = getattr(result, "error_message", None) or "crawl failed"
            return ScrapedPage(url=url, title="", markdown="", success=False, error=error)

        markdown = str(result.markdown) if result.markdown else ""
        metadata = result.metadata if isinstance(result.metadata, dict) else {}
        title = metadata.get("title", "") or ""
        if not markdown.strip():
            return ScrapedPage(url=url, title=title, markdown="", success=False, error="No extractable content")
        return ScrapedPage(url=url, title=title, markdown=markdown.strip(), success=True)

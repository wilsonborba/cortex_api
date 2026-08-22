from __future__ import annotations

from dataclasses import dataclass
from typing import Optional, Protocol


@dataclass(frozen=True)
class SearchResult:
    title: str
    url: str
    snippet: str


class SearchProvider(Protocol):
    provider: str

    def search(self, query: str, max_results: int = 5) -> list[SearchResult]:
        """Returns up to `max_results` results, ranked best-first.

        Raises `RetrievalError` if the provider itself couldn't be reached.
        """
        ...


@dataclass(frozen=True)
class ScrapedPage:
    url: str
    title: str
    markdown: str
    success: bool
    error: Optional[str] = None


class ContentScraper(Protocol):
    scraper: str

    def scrape(self, url: str) -> ScrapedPage:
        """Fetches `url` and returns its content as clean Markdown.

        Never raises: a failure is reported as `ScrapedPage(success=False, ...)`
        so a formatter can fall through to the next scraper in its chain.
        """
        ...


class RetrievalError(RuntimeError):
    """Raised when a search provider can't be reached at all."""

    def __init__(self, source: str, message: str) -> None:
        super().__init__(message)
        self.source = source

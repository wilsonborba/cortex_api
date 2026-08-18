from __future__ import annotations

from lib.engine.retrieval.base import ContentScraper, RetrievalError, ScrapedPage, SearchProvider, SearchResult
from lib.engine.retrieval.scraper import Crawl4AIScraper, TrafilaturaScraper
from lib.engine.retrieval.search import DuckDuckGoSearchProvider, SearXNGSearchProvider
from lib.engine.retrieval.service import WebContextResult, WebRetrievalService, build_default_web_retrieval_service

__all__ = [
    "ContentScraper",
    "Crawl4AIScraper",
    "DuckDuckGoSearchProvider",
    "RetrievalError",
    "ScrapedPage",
    "SearchProvider",
    "SearchResult",
    "SearXNGSearchProvider",
    "TrafilaturaScraper",
    "WebContextResult",
    "WebRetrievalService",
    "build_default_web_retrieval_service",
]

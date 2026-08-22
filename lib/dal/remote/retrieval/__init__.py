from __future__ import annotations

from lib.engine.retrieval.base import ContentScraper, RetrievalError, ScrapedPage, SearchProvider, SearchResult
from lib.engine.retrieval.hippocampus import (
    DocumentContext,
    HippocampusClient,
    MemoryChunk,
    build_default_hippocampus_client,
    format_memory_context,
)
from lib.engine.retrieval.scraper import Crawl4AIScraper, TrafilaturaScraper
from lib.engine.retrieval.search import DuckDuckGoSearchProvider, SearXNGSearchProvider
from lib.engine.retrieval.service import WebContextResult, WebRetrievalService, build_default_web_retrieval_service

__all__ = [
    "ContentScraper",
    "Crawl4AIScraper",
    "DocumentContext",
    "DuckDuckGoSearchProvider",
    "HippocampusClient",
    "MemoryChunk",
    "RetrievalError",
    "ScrapedPage",
    "SearchProvider",
    "SearchResult",
    "SearXNGSearchProvider",
    "TrafilaturaScraper",
    "WebContextResult",
    "WebRetrievalService",
    "build_default_hippocampus_client",
    "build_default_web_retrieval_service",
    "format_memory_context",
]

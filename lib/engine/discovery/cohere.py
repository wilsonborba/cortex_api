from __future__ import annotations

from typing import Callable, Optional

import httpx

from lib.engine.discovery.openai_compatible import OpenAICompatibleDiscovery

DEFAULT_BASE_URL = "https://api.cohere.com/v1"


class CohereDiscovery(OpenAICompatibleDiscovery):
    """Cohere's free-tier OpenAI-compatible API."""

    def __init__(
        self,
        api_key: Optional[str],
        base_url: str = DEFAULT_BASE_URL,
        timeout: float = 10.0,
        client_factory: Optional[Callable[[], httpx.Client]] = None,
    ) -> None:
        super().__init__(
            provider="cohere", base_url=base_url, api_key=api_key, timeout=timeout, client_factory=client_factory
        )

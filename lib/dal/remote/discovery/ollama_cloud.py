from __future__ import annotations

from typing import Callable, Optional

import httpx

from lib.engine.discovery.openai_compatible import OpenAICompatibleDiscovery

DEFAULT_BASE_URL = "https://ollama.com/v1"


class OllamaCloudDiscovery(OpenAICompatibleDiscovery):
    """Ollama Cloud's free-tier OpenAI-compatible API.

    Distinct from `OllamaDiscovery` (local daemon, no auth, `/api/tags`):
    this is the hosted service, authenticated with an API key like the other
    free-tier providers.
    """

    def __init__(
        self,
        api_key: Optional[str],
        base_url: str = DEFAULT_BASE_URL,
        timeout: float = 10.0,
        client_factory: Optional[Callable[[], httpx.Client]] = None,
    ) -> None:
        super().__init__(
            provider="ollama_cloud", base_url=base_url, api_key=api_key, timeout=timeout, client_factory=client_factory
        )

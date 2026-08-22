from __future__ import annotations

from typing import Callable, Optional

import httpx

from lib.engine.discovery.openai_compatible import OpenAICompatibleDiscovery

# Recovered from the multilingual research pass, collected after the batch
# test -- verify the key works before relying on this driver (see
# docs/free-tier-adapters-plan.md).
DEFAULT_BASE_URL = "https://api.sambanova.ai/v1"


class SambaNovaDiscovery(OpenAICompatibleDiscovery):
    """SambaNova's free-tier OpenAI-compatible API."""

    def __init__(
        self,
        api_key: Optional[str],
        base_url: str = DEFAULT_BASE_URL,
        timeout: float = 10.0,
        client_factory: Optional[Callable[[], httpx.Client]] = None,
    ) -> None:
        super().__init__(
            provider="sambanova", base_url=base_url, api_key=api_key, timeout=timeout, client_factory=client_factory
        )

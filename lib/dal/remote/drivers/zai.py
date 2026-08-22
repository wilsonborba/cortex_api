from __future__ import annotations

from typing import Callable, Optional

import httpx

from lib.engine.drivers.openai_compatible import OpenAICompatibleDriver

DEFAULT_BASE_URL = "https://api.z.ai/api/paas/v4"


class ZaiDriver(OpenAICompatibleDriver):
    """Z.AI / Zhipu's free-tier OpenAI-compatible API."""

    def __init__(
        self,
        api_key: Optional[str],
        base_url: str = DEFAULT_BASE_URL,
        timeout: float = 60.0,
        client_factory: Optional[Callable[[], httpx.Client]] = None,
    ) -> None:
        super().__init__(
            provider="zai", base_url=base_url, api_key=api_key, timeout=timeout, client_factory=client_factory
        )

from __future__ import annotations

from typing import Callable, Optional

import httpx

from lib.engine.drivers.openai_compatible import OpenAICompatibleDriver

# `.com`, not `.cn` -- confirmed via real test in 2026-08-19 (see
# docs/free-tier-adapters-plan.md).
DEFAULT_BASE_URL = "https://api.siliconflow.com/v1"


class SiliconFlowDriver(OpenAICompatibleDriver):
    """SiliconFlow's free-tier OpenAI-compatible API."""

    def __init__(
        self,
        api_key: Optional[str],
        base_url: str = DEFAULT_BASE_URL,
        timeout: float = 60.0,
        client_factory: Optional[Callable[[], httpx.Client]] = None,
    ) -> None:
        super().__init__(
            provider="siliconflow", base_url=base_url, api_key=api_key, timeout=timeout, client_factory=client_factory
        )

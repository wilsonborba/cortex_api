from __future__ import annotations

from typing import Callable, Optional

import httpx

from lib.engine.discovery.openai_compatible import OpenAICompatibleDiscovery

DEFAULT_BASE_URL = "https://generativelanguage.googleapis.com/v1beta/openai"


class GoogleAIStudioDiscovery(OpenAICompatibleDiscovery):
    """Google AI Studio (Gemini) via its own OpenAI-compatible endpoint.

    Fits the shared base class as-is; kept as its own subclass (rather than a
    bare instantiation in `registry_service.py`) since the key
    (`CORTEX_GOOGLE_AI_STUDIO_API_KEY`) is deliberately separate from the
    pre-existing, unrelated `GOOGLE_API_KEY` used elsewhere in the repo.
    """

    def __init__(
        self,
        api_key: Optional[str],
        base_url: str = DEFAULT_BASE_URL,
        timeout: float = 10.0,
        client_factory: Optional[Callable[[], httpx.Client]] = None,
    ) -> None:
        super().__init__(
            provider="google_ai_studio",
            base_url=base_url,
            api_key=api_key,
            timeout=timeout,
            client_factory=client_factory,
        )

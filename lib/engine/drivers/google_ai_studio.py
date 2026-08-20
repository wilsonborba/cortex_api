from __future__ import annotations

from typing import Callable, Optional

import httpx

from lib.engine.drivers.openai_compatible import OpenAICompatibleDriver

DEFAULT_BASE_URL = "https://generativelanguage.googleapis.com/v1beta/openai"


class GoogleAIStudioDriver(OpenAICompatibleDriver):
    """Google AI Studio (Gemini) via its own OpenAI-compatible endpoint.

    See `discovery/google_ai_studio.py` for why this key/subclass is kept
    separate from the pre-existing `GOOGLE_API_KEY`.
    """

    def __init__(
        self,
        api_key: Optional[str],
        base_url: str = DEFAULT_BASE_URL,
        timeout: float = 60.0,
        client_factory: Optional[Callable[[], httpx.Client]] = None,
    ) -> None:
        super().__init__(
            provider="google_ai_studio",
            base_url=base_url,
            api_key=api_key,
            timeout=timeout,
            client_factory=client_factory,
        )

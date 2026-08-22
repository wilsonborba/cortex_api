from __future__ import annotations

from typing import Callable, Optional

import httpx

from lib.engine.drivers.openai_compatible import OpenAICompatibleDriver

# Inference is OpenAI-compatible via the router host, even though auth-check
# (discovery/huggingface.py) isn't -- so the driver reuses the shared base.
DEFAULT_BASE_URL = "https://router.huggingface.co/v1"


class HuggingFaceDriver(OpenAICompatibleDriver):
    """Hugging Face's OpenAI-compatible inference router."""

    def __init__(
        self,
        api_key: Optional[str],
        base_url: str = DEFAULT_BASE_URL,
        timeout: float = 60.0,
        client_factory: Optional[Callable[[], httpx.Client]] = None,
    ) -> None:
        super().__init__(
            provider="huggingface", base_url=base_url, api_key=api_key, timeout=timeout, client_factory=client_factory
        )

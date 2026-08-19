from __future__ import annotations

from typing import Callable, Optional

import httpx

from lib.engine.drivers.base import DriverResult, failed
from lib.engine.drivers.openai_compatible import OpenAICompatibleDriver

BASE_URL_TEMPLATE = "https://api.cloudflare.com/client/v4/accounts/{account_id}/ai/v1"


class CloudflareDriver(OpenAICompatibleDriver):
    """Cloudflare Workers AI's OpenAI-compatible API.

    See `discovery/cloudflare.py`: the account id is embedded in the base
    URL, not just the bearer token.
    """

    def __init__(
        self,
        api_key: Optional[str],
        account_id: Optional[str],
        timeout: float = 60.0,
        client_factory: Optional[Callable[[], httpx.Client]] = None,
    ) -> None:
        self._account_id = account_id
        base_url = BASE_URL_TEMPLATE.format(account_id=account_id or "")
        super().__init__(
            provider="cloudflare", base_url=base_url, api_key=api_key, timeout=timeout, client_factory=client_factory
        )

    def run(self, model: str, prompt: str) -> DriverResult:
        if not self._account_id:
            return failed("unreachable", "cloudflare: no account id configured")
        return super().run(model, prompt)

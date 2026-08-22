from __future__ import annotations

from typing import Callable, Optional

import httpx

from lib.engine.discovery.base import ProviderDiscoveryError
from lib.engine.discovery.openai_compatible import OpenAICompatibleDiscovery

BASE_URL_TEMPLATE = "https://api.cloudflare.com/client/v4/accounts/{account_id}/ai/v1"


class CloudflareDiscovery(OpenAICompatibleDiscovery):
    """Cloudflare Workers AI's OpenAI-compatible API.

    The only "normal" free-tier provider whose base URL isn't fixed: the
    account id is embedded in the path (`CORTEX_CLOUDFLARE_ACCOUNT_ID`,
    alongside `CORTEX_CLOUDFLARE_API_KEY`), so both must be present.
    """

    def __init__(
        self,
        api_key: Optional[str],
        account_id: Optional[str],
        timeout: float = 10.0,
        client_factory: Optional[Callable[[], httpx.Client]] = None,
    ) -> None:
        self._account_id = account_id
        base_url = BASE_URL_TEMPLATE.format(account_id=account_id or "")
        super().__init__(
            provider="cloudflare", base_url=base_url, api_key=api_key, timeout=timeout, client_factory=client_factory
        )

    def discover(self):
        if not self._account_id:
            raise ProviderDiscoveryError(self.provider, "cloudflare: no account id configured")
        return super().discover()

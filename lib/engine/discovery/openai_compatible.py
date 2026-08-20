from __future__ import annotations

from typing import Any, Callable, Optional

import httpx

from lib.dal.models import AccessStatus
from lib.engine.discovery.base import DiscoveredModel, ProviderDiscoveryError

# Free-tier cloud models aren't local, but they cost nothing within quota, so
# they start eligible across the same broad tier range Antigravity's cloud
# models do (see discovery/antigravity.py).
DEFAULT_TIER_ELIGIBILITY = [0, 1, 2, 3, 4, 5]
DEFAULT_CONTEXT_WINDOW = 8192


class OpenAICompatibleDiscovery:
    """Discovers models from any provider that speaks the OpenAI-compatible
    `GET /models` shape (`{"data": [{"id": ...}, ...]}`), authenticated via a
    bearer token.

    Base class for the free-tier provider adapters (`docs/free-tier-adapters-plan.md`):
    a "normal" provider is just a subclass declaring `base_url` + reading its
    own API key env var. A provider needing different auth or response shape
    (Cloudflare, Hugging Face) subclasses `ProviderDiscovery` directly instead
    of reusing this as-is.
    """

    def __init__(
        self,
        provider: str,
        base_url: str,
        api_key: Optional[str],
        models_path: str = "/models",
        timeout: float = 10.0,
        client_factory: Optional[Callable[[], httpx.Client]] = None,
    ) -> None:
        self.provider = provider
        self._base_url = base_url.rstrip("/")
        self._api_key = api_key
        self._models_path = models_path
        self._timeout = timeout
        self._client_factory = client_factory or (lambda: httpx.Client(timeout=self._timeout))

    def discover(self) -> list[DiscoveredModel]:
        if not self._api_key:
            raise ProviderDiscoveryError(self.provider, f"{self.provider}: no API key configured")
        try:
            with self._client_factory() as client:
                response = client.get(f"{self._base_url}{self._models_path}", headers=self._headers())
                response.raise_for_status()
                payload = response.json()
        except (httpx.HTTPError, ValueError) as exc:
            raise ProviderDiscoveryError(
                self.provider, f"{self.provider} unreachable at {self._base_url}: {exc}"
            ) from exc

        entries = payload.get("data", []) if isinstance(payload, dict) else []
        return [self._parse_entry(raw) for raw in entries if isinstance(raw, dict) and raw.get("id")]

    def _headers(self) -> dict[str, str]:
        return {"Authorization": f"Bearer {self._api_key}"}

    def _parse_entry(self, raw: dict[str, Any]) -> DiscoveredModel:
        model_id = raw["id"]
        return DiscoveredModel(
            id=f"{self.provider}/{model_id}",
            provider=self.provider,
            display_name=model_id,
            access_status=AccessStatus.AVAILABLE.value,
            status_reason="Free-tier key OK",
            context_window=DEFAULT_CONTEXT_WINDOW,
            is_local=False,
            tier_eligibility=list(DEFAULT_TIER_ELIGIBILITY),
            capabilities={},
            cost_per_million_tokens=0.0,
        )

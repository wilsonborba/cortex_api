from __future__ import annotations

from typing import Callable, Optional

import httpx

from lib.dal.models import AccessStatus
from lib.engine.discovery.base import DiscoveredModel, ProviderDiscoveryError
from lib.engine.discovery.openai_compatible import DEFAULT_CONTEXT_WINDOW, DEFAULT_TIER_ELIGIBILITY
from lib.engine.discovery.heuristics import infer_capabilities, infer_tier_eligibility

WHOAMI_URL = "https://huggingface.co/api/whoami-v2"
MODELS_URL = "https://router.huggingface.co/v1/models"


class HuggingFaceDiscovery:
    """Hugging Face: auth-check and inference both diverge from the shared
    OpenAI-compatible base, so this doesn't subclass it.

    Auth is verified against `whoami-v2` (there's no `/models` list at that
    host); the actual catalog comes from the separate inference router,
    which *is* OpenAI-compatible once authenticated.
    """

    provider = "huggingface"

    def __init__(
        self,
        api_key: Optional[str],
        whoami_url: str = WHOAMI_URL,
        models_url: str = MODELS_URL,
        timeout: float = 10.0,
        client_factory: Optional[Callable[[], httpx.Client]] = None,
    ) -> None:
        self._api_key = api_key
        self._whoami_url = whoami_url
        self._models_url = models_url
        self._timeout = timeout
        self._client_factory = client_factory or (lambda: httpx.Client(timeout=timeout))

    def discover(self) -> list[DiscoveredModel]:
        if not self._api_key:
            raise ProviderDiscoveryError(self.provider, "huggingface: no API key configured")

        headers = {"Authorization": f"Bearer {self._api_key}"}
        try:
            with self._client_factory() as client:
                client.get(self._whoami_url, headers=headers).raise_for_status()
                response = client.get(self._models_url, headers=headers)
                response.raise_for_status()
                payload = response.json()
        except (httpx.HTTPError, ValueError) as exc:
            raise ProviderDiscoveryError(self.provider, f"huggingface unreachable: {exc}") from exc

        entries = payload.get("data", []) if isinstance(payload, dict) else []
        return [
            DiscoveredModel(
                id=f"{self.provider}/{raw['id']}",
                provider=self.provider,
                display_name=raw["id"],
                access_status=AccessStatus.AVAILABLE.value,
                status_reason="Free-tier key OK",
                context_window=DEFAULT_CONTEXT_WINDOW,
                is_local=False,
                tier_eligibility=infer_tier_eligibility(raw["id"]) or list(DEFAULT_TIER_ELIGIBILITY),
                capabilities=infer_capabilities(raw["id"]),
                cost_per_million_tokens=0.0,
            )
            for raw in entries
            if isinstance(raw, dict) and raw.get("id")
        ]

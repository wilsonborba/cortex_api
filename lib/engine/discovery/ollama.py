from __future__ import annotations

from typing import Any, Callable, Optional

import httpx

from lib.dal.models import AccessStatus
from lib.engine.discovery.base import DiscoveredModel, ProviderDiscoveryError

# Local models start eligible for the low-effort tiers; promote them via
# `cortex models config` once telemetry shows they hold up for more.
DEFAULT_TIER_ELIGIBILITY = [0, 1, 2]
DEFAULT_CONTEXT_WINDOW = 8192


class OllamaDiscovery:
    """Discovers locally-served models from an Ollama daemon over its HTTP API."""

    provider = "ollama"

    def __init__(
        self,
        base_url: str,
        timeout: float = 10.0,
        client_factory: Optional[Callable[[], httpx.Client]] = None,
    ) -> None:
        self._base_url = base_url.rstrip("/")
        self._timeout = timeout
        self._client_factory = client_factory or (lambda: httpx.Client(timeout=self._timeout))

    def discover(self) -> list[DiscoveredModel]:
        try:
            with self._client_factory() as client:
                response = client.get(f"{self._base_url}/api/tags")
                response.raise_for_status()
                payload = response.json()
                entries = payload.get("models", []) if isinstance(payload, dict) else []
                return [self._parse_entry(raw, client) for raw in entries]
        except (httpx.HTTPError, ValueError) as exc:
            raise ProviderDiscoveryError(
                self.provider, f"Ollama unreachable at {self._base_url}: {exc}"
            ) from exc

    def _parse_entry(self, raw: dict[str, Any], client: httpx.Client) -> DiscoveredModel:
        name = raw.get("name") or raw.get("model") or "unknown"
        details = raw.get("details") or {}
        return DiscoveredModel(
            id=f"{self.provider}/{name}",
            provider=self.provider,
            display_name=name,
            access_status=AccessStatus.AVAILABLE.value,
            status_reason="Local (0 cost)",
            parameter_size=details.get("parameter_size"),
            context_window=self._probe_context_window(client, name),
            is_local=True,
            tier_eligibility=list(DEFAULT_TIER_ELIGIBILITY),
            capabilities={},
            cost_per_million_tokens=0.0,
        )

    def _probe_context_window(self, client: httpx.Client, name: str) -> int:
        """Best-effort: `/api/tags` doesn't carry context length, `/api/show` might.

        Any failure here (older Ollama, unexpected shape) falls back to a
        conservative default rather than failing the whole discovery pass.
        """
        try:
            response = client.post(f"{self._base_url}/api/show", json={"name": name})
            response.raise_for_status()
            model_info = response.json().get("model_info", {}) or {}
            for key, value in model_info.items():
                if key.endswith("context_length") and isinstance(value, int):
                    return value
        except (httpx.HTTPError, ValueError):
            pass
        return DEFAULT_CONTEXT_WINDOW

from __future__ import annotations

import subprocess
from typing import Callable, Optional

from lib.dal.models import AccessStatus
from lib.engine.discovery.base import DiscoveredModel, ProviderDiscoveryError

# Gemini/partner models via Antigravity aren't local; start them in the
# mid tiers and let telemetry (or manual curation) move them from there.
DEFAULT_TIER_ELIGIBILITY = [2, 3, 4, 5]

_SIGN_IN_MARKERS = ("sign in", "not logged in", "unauthorized", "unauthenticated")
_NOISE_PREFIXES = ("fetching", "available models", "model", "usage", "---")


class AntigravityDiscovery:
    """Discovers models via the Antigravity CLI (`agy models`), parsed non-interactively.

    `agy models` has no machine-readable output mode, so parsing here is
    intentionally lenient: one model identifier per non-empty, non-banner line.
    """

    provider = "agy"

    def __init__(
        self,
        command: str = "agy",
        timeout: float = 20.0,
        runner: Optional[Callable[..., subprocess.CompletedProcess]] = None,
    ) -> None:
        self._command = command
        self._timeout = timeout
        self._runner = runner or subprocess.run

    def discover(self) -> list[DiscoveredModel]:
        try:
            result = self._runner(
                [self._command, "models"],
                capture_output=True,
                text=True,
                timeout=self._timeout,
            )
        except (OSError, subprocess.TimeoutExpired) as exc:
            raise ProviderDiscoveryError(
                self.provider, f"'{self._command} models' failed to run: {exc}"
            ) from exc

        combined_output = f"{result.stdout}\n{result.stderr}"
        if result.returncode != 0 or self._looks_unauthenticated(combined_output):
            raise ProviderDiscoveryError(
                self.provider,
                "Antigravity CLI is not signed in; run `agy` interactively to authenticate.",
            )
        return self._parse_models(result.stdout)

    @staticmethod
    def _looks_unauthenticated(output: str) -> bool:
        lowered = output.lower()
        return any(marker in lowered for marker in _SIGN_IN_MARKERS)

    def _parse_models(self, stdout: str) -> list[DiscoveredModel]:
        models: list[DiscoveredModel] = []
        for raw_line in stdout.splitlines():
            line = raw_line.strip()
            if not line or line.lower().startswith(_NOISE_PREFIXES):
                continue
            name = line.split()[0].strip("-*• ")
            if not name:
                continue
            models.append(
                DiscoveredModel(
                    id=f"{self.provider}/{name}",
                    provider=self.provider,
                    display_name=name,
                    access_status=AccessStatus.AVAILABLE.value,
                    status_reason="OAuth OK",
                    is_local=False,
                    tier_eligibility=list(DEFAULT_TIER_ELIGIBILITY),
                    capabilities={},
                    cost_per_million_tokens=0.0,
                )
            )
        return models

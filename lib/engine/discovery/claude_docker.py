from __future__ import annotations

import json
from pathlib import Path
from typing import Tuple

from lib.dal.models import AccessStatus
from lib.engine.discovery.base import DiscoveredModel

# Claude has no public "list my models" call, so this is a curated seed of the
# current Claude family. Extend/reorder via `cortex models config`, not here.
_CATALOG: Tuple[dict, ...] = (
    {"id": "claude-sonnet-5", "display_name": "Claude Sonnet 5", "tier_eligibility": [2, 3, 4]},
    {"id": "claude-opus-5", "display_name": "Claude Opus 5", "tier_eligibility": [4, 5]},
    {"id": "claude-fable-5", "display_name": "Claude Fable 5", "tier_eligibility": [3, 4]},
    {
        "id": "claude-haiku-4-5-20251001",
        "display_name": "Claude Haiku 4.5",
        "tier_eligibility": [0, 1, 2],
    },
)


class ClaudeDockerDiscovery:
    """Pre-mapped Claude models, gated by a local `claude-docker` credentials probe.

    This never fails outright: the catalog stays visible even when signed out,
    it's just reported as `OFFLINE` so it stays out of the Router's candidate set.
    """

    provider = "claude"

    def __init__(self, credentials_path: Path) -> None:
        self._credentials_path = credentials_path

    def discover(self) -> list[DiscoveredModel]:
        authenticated, reason = self._probe()
        status = AccessStatus.AVAILABLE.value if authenticated else AccessStatus.OFFLINE.value
        return [
            DiscoveredModel(
                id=f"{self.provider}/{entry['id']}",
                provider=self.provider,
                display_name=entry["display_name"],
                access_status=status,
                status_reason=reason,
                is_local=False,
                tier_eligibility=list(entry["tier_eligibility"]),
                capabilities={},
                cost_per_million_tokens=0.0,
            )
            for entry in _CATALOG
        ]

    def _probe(self) -> Tuple[bool, str]:
        if not self._credentials_path.exists():
            return False, f"No credentials found at {self._credentials_path}"
        try:
            json.loads(self._credentials_path.read_text(encoding="utf-8"))
        except (OSError, ValueError) as exc:
            return False, f"Unreadable credentials file: {exc}"
        return True, "claude-docker credentials present"

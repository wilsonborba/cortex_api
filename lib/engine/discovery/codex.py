from __future__ import annotations

import json
from pathlib import Path
from typing import Optional, Tuple

from lib.dal.models import AccessStatus
from lib.engine.discovery.base import DiscoveredModel

# Codex CLI has no "list my models" command either, so this is a curated seed.
# Extend/reorder via `cortex models config`, not here.
_CATALOG: Tuple[dict, ...] = ({"id": "o3", "display_name": "OpenAI o3", "tier_eligibility": [4, 5]},)


class CodexDiscovery:
    """Pre-mapped Codex/OpenAI models, gated by a local `~/.codex/auth.json` probe.

    `auth_mode` in that file tells apart a raw API key (billed access to whatever
    is mapped below) from a ChatGPT-plan login (access depends on entitlements we
    can't verify locally, so it's surfaced as `REQUIRES_SUBSCRIPTION` rather than
    guessed at).
    """

    provider = "codex"

    def __init__(self, auth_path: Path) -> None:
        self._auth_path = auth_path

    def discover(self) -> list[DiscoveredModel]:
        authenticated, reason, auth_mode = self._probe()
        status, status_reason = self._resolve_status(authenticated, reason, auth_mode)
        return [
            DiscoveredModel(
                id=f"{self.provider}/{entry['id']}",
                provider=self.provider,
                display_name=entry["display_name"],
                access_status=status,
                status_reason=status_reason,
                is_local=False,
                tier_eligibility=list(entry["tier_eligibility"]),
                capabilities={},
                cost_per_million_tokens=0.0,
            )
            for entry in _CATALOG
        ]

    @staticmethod
    def _resolve_status(
        authenticated: bool, reason: str, auth_mode: Optional[str]
    ) -> Tuple[str, str]:
        if not authenticated:
            return AccessStatus.OFFLINE.value, reason
        if auth_mode == "chatgpt":
            return (
                AccessStatus.REQUIRES_SUBSCRIPTION.value,
                "Logged in via ChatGPT; access depends on plan entitlements",
            )
        return AccessStatus.AVAILABLE.value, reason

    def _probe(self) -> Tuple[bool, str, Optional[str]]:
        if not self._auth_path.exists():
            return False, f"No credentials found at {self._auth_path}", None
        try:
            data = json.loads(self._auth_path.read_text(encoding="utf-8"))
        except (OSError, ValueError) as exc:
            return False, f"Unreadable credentials file: {exc}", None
        return True, "codex credentials present", data.get("auth_mode")

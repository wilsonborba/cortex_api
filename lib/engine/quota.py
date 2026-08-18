from __future__ import annotations

from dataclasses import dataclass
from datetime import datetime, timedelta, timezone
from typing import List, Optional

from lib.core.settings import Settings, get_settings
from lib.core.time_utils import ensure_utc
from lib.dal.models import AccessStatus
from lib.dal.repositories.model_repository import ModelRepository
from lib.dal.repositories.quota_repository import QuotaRepository
from lib.engine.drivers.base import DriverResult

# Local providers run for free with no external window to budget against;
# their quota factor is always the maximum.
UNBOUNDED_PROVIDERS = {"ollama"}


@dataclass(frozen=True)
class ProviderQuota:
    """A provider's token budget within the current sliding window."""

    provider: str
    window_hours: int
    window_limit_tokens: Optional[int]  # None => unbounded (local providers)
    consumed_tokens: int
    remaining_tokens: Optional[int]
    quota_factor: float  # Q in [0.0, 1.0]; 1.0 == full budget available
    requests_count: int = 0


class QuotaTracker:
    """Sliding-window token budget plus 429/cooldown handling.

    `record_execution` is the single entry point the Executor (issue #9)
    should call after every driver run: it logs usage for the window
    calculation and, on a rate-limit signal, flips the model straight to
    `COOLING_DOWN` in the Model Registry so the Router stops offering it.
    """

    def __init__(
        self,
        quota_repo: Optional[QuotaRepository] = None,
        model_repo: Optional[ModelRepository] = None,
        settings: Optional[Settings] = None,
    ) -> None:
        self._quota_repo = quota_repo or QuotaRepository()
        self._model_repo = model_repo or ModelRepository()
        self._settings = settings or get_settings()

    # -- recording --------------------------------------------------------

    def record_execution(self, provider: str, model: str, result: DriverResult) -> None:
        if result.input_tokens or result.output_tokens:
            self._quota_repo.record_usage(
                provider=provider,
                model=model,
                input_tokens=result.input_tokens,
                output_tokens=result.output_tokens,
            )
        if not result.success and result.error_type == "rate_limit":
            self.enter_cooldown(model, reason=result.error_message)

    # -- reads --------------------------------------------------------------

    def get_quota(self, provider: str) -> ProviderQuota:
        hours = self._settings.sliding_window_hours
        consumed = self._quota_repo.get_consumed_tokens_in_window(provider, hours=hours)
        return self._to_provider_quota(provider, hours, consumed, requests_count=0)

    def get_summary(self) -> List[ProviderQuota]:
        hours = self._settings.sliding_window_hours
        rows = {row["provider"]: row for row in self._quota_repo.get_usage_summary(hours=hours)}
        providers = set(rows) | {m.provider for m in self._model_repo.list_models()}
        return [
            self._to_provider_quota(
                provider,
                hours,
                rows.get(provider, {}).get("total_tokens", 0),
                requests_count=rows.get(provider, {}).get("requests_count", 0),
            )
            for provider in sorted(providers)
        ]

    def _to_provider_quota(
        self, provider: str, hours: int, consumed: int, requests_count: int
    ) -> ProviderQuota:
        limit = self._window_limit(provider)
        if limit is None:
            return ProviderQuota(provider, hours, None, consumed, None, 1.0, requests_count)
        remaining = max(0, limit - consumed)
        factor = (remaining / limit) if limit > 0 else 0.0
        return ProviderQuota(provider, hours, limit, consumed, remaining, factor, requests_count)

    def _window_limit(self, provider: str) -> Optional[int]:
        if provider in UNBOUNDED_PROVIDERS:
            return None
        return self._settings.quota_window_tokens_by_provider.get(
            provider, self._settings.default_quota_window_tokens
        )

    # -- cooling down ---------------------------------------------------------

    def enter_cooldown(
        self, model_id: str, reason: Optional[str] = None, minutes: Optional[int] = None
    ) -> None:
        until = datetime.now(timezone.utc) + timedelta(
            minutes=minutes if minutes is not None else self._settings.cooldown_minutes
        )
        self._model_repo.set_cooldown(model_id, until=until, reason=reason or "Rate limited (429)")

    def refresh_cooldowns(self) -> List[str]:
        """Moves any model whose cooldown window has elapsed back to `OFFLINE`.

        `OFFLINE` (not `AVAILABLE`) on purpose: only a real Model Registry
        sync should be the one to confirm the provider is actually back.
        """
        now = datetime.now(timezone.utc)
        cleared: List[str] = []
        for entry in self._model_repo.list_models(access_status=AccessStatus.COOLING_DOWN.value):
            if entry.cooldown_until is not None and ensure_utc(entry.cooldown_until) <= now:
                self._model_repo.update_status(
                    entry.id, AccessStatus.OFFLINE, reason="Cooldown elapsed; pending re-sync"
                )
                self._model_repo.clear_cooldown(entry.id)
                cleared.append(entry.id)
        return cleared

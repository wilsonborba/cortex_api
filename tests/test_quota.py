from __future__ import annotations

from datetime import datetime, timedelta, timezone

import pytest
from sqlalchemy.orm import Session

from lib.core.settings import Settings
from lib.core.time_utils import ensure_utc
from lib.dal.models import AccessStatus, ModelCatalogEntry
from lib.dal.repositories.model_repository import ModelRepository
from lib.dal.repositories.quota_repository import QuotaRepository
from lib.engine.drivers.base import DriverResult
from lib.engine.quota import QuotaTracker


@pytest.fixture
def quota_repo_(db_session: Session) -> QuotaRepository:
    return QuotaRepository(session_factory=lambda: db_session)


@pytest.fixture
def model_repo_(db_session: Session) -> ModelRepository:
    return ModelRepository(session_factory=lambda: db_session)


def _settings(**overrides) -> Settings:
    base = dict(
        default_quota_window_tokens=10_000,
        quota_window_tokens_by_provider={},
        cooldown_minutes=15,
        sliding_window_hours=5,
    )
    base.update(overrides)
    return Settings(**base)


def test_quota_factor_decreases_as_tokens_are_consumed(
    quota_repo_: QuotaRepository, model_repo_: ModelRepository
):
    tracker = QuotaTracker(
        quota_repo=quota_repo_, model_repo=model_repo_, settings=_settings()
    )
    result = DriverResult(
        success=True, response_text="hi", input_tokens=3000, output_tokens=1000, latency_ms=10
    )

    tracker.record_execution("codex", "codex/o3", result)

    quota = tracker.get_quota("codex")
    assert quota.consumed_tokens == 4000
    assert quota.window_limit_tokens == 10_000
    assert quota.remaining_tokens == 6000
    assert quota.quota_factor == pytest.approx(0.6)


def test_quota_factor_ignores_usage_outside_the_window(
    quota_repo_: QuotaRepository, model_repo_: ModelRepository
):
    tracker = QuotaTracker(quota_repo=quota_repo_, model_repo=model_repo_, settings=_settings())
    stale = datetime.now(timezone.utc) - timedelta(hours=6)
    quota_repo_.record_usage(
        provider="claude-quota-stale", model="claude-sonnet-5", input_tokens=9000, output_tokens=900, timestamp=stale
    )

    quota = tracker.get_quota("claude-quota-stale")

    assert quota.consumed_tokens == 0
    assert quota.quota_factor == 1.0


def test_local_provider_is_always_full_quota(quota_repo_: QuotaRepository, model_repo_: ModelRepository):
    tracker = QuotaTracker(quota_repo=quota_repo_, model_repo=model_repo_, settings=_settings())
    result = DriverResult(
        success=True, response_text="hi", input_tokens=999_999, output_tokens=999_999, latency_ms=10
    )

    tracker.record_execution("ollama", "ollama/dolphin3:8b", result)

    quota = tracker.get_quota("ollama")
    assert quota.window_limit_tokens is None
    assert quota.remaining_tokens is None
    assert quota.quota_factor == 1.0


def test_rate_limited_execution_enters_cooldown(
    quota_repo_: QuotaRepository, model_repo_: ModelRepository
):
    model_repo_.upsert(
        ModelCatalogEntry(
            id="codex/quota-cooldown-o3",
            provider="codex",
            display_name="OpenAI o3",
            access_status=AccessStatus.AVAILABLE.value,
            context_window=8192,
            is_local=False,
            tier_eligibility=[],
            capabilities={},
            cost_per_million_tokens=0.0,
            is_enabled=True,
        )
    )
    tracker = QuotaTracker(quota_repo=quota_repo_, model_repo=model_repo_, settings=_settings())
    result = DriverResult(
        success=False,
        response_text="",
        input_tokens=0,
        output_tokens=0,
        latency_ms=0,
        error_type="rate_limit",
        error_message="429 rate limited",
    )

    tracker.record_execution("codex", "codex/quota-cooldown-o3", result)

    stored = model_repo_.get_by_id("codex/quota-cooldown-o3")
    assert stored.access_status == AccessStatus.COOLING_DOWN.value
    assert stored.cooldown_until is not None
    assert ensure_utc(stored.cooldown_until) > datetime.now(timezone.utc)


def test_refresh_cooldowns_moves_expired_models_to_offline(
    quota_repo_: QuotaRepository, model_repo_: ModelRepository
):
    model_repo_.upsert(
        ModelCatalogEntry(
            id="codex/quota-expired-o3",
            provider="codex",
            display_name="OpenAI o3",
            access_status=AccessStatus.COOLING_DOWN.value,
            status_reason="Rate limited (429)",
            context_window=8192,
            is_local=False,
            tier_eligibility=[],
            capabilities={},
            cost_per_million_tokens=0.0,
            is_enabled=True,
            cooldown_until=datetime.now(timezone.utc) - timedelta(minutes=1),
        )
    )
    tracker = QuotaTracker(quota_repo=quota_repo_, model_repo=model_repo_, settings=_settings())

    cleared = tracker.refresh_cooldowns()

    assert "codex/quota-expired-o3" in cleared
    stored = model_repo_.get_by_id("codex/quota-expired-o3")
    assert stored.access_status == AccessStatus.OFFLINE.value
    assert stored.cooldown_until is None


def test_refresh_cooldowns_leaves_active_cooldowns_alone(
    quota_repo_: QuotaRepository, model_repo_: ModelRepository
):
    future = datetime.now(timezone.utc) + timedelta(minutes=10)
    model_repo_.upsert(
        ModelCatalogEntry(
            id="codex/quota-active-o3",
            provider="codex",
            display_name="OpenAI o3",
            access_status=AccessStatus.COOLING_DOWN.value,
            context_window=8192,
            is_local=False,
            tier_eligibility=[],
            capabilities={},
            cost_per_million_tokens=0.0,
            is_enabled=True,
            cooldown_until=future,
        )
    )
    tracker = QuotaTracker(quota_repo=quota_repo_, model_repo=model_repo_, settings=_settings())

    cleared = tracker.refresh_cooldowns()

    assert "codex/quota-active-o3" not in cleared
    stored = model_repo_.get_by_id("codex/quota-active-o3")
    assert stored.access_status == AccessStatus.COOLING_DOWN.value
    assert ensure_utc(stored.cooldown_until) == future

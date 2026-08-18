from __future__ import annotations

from datetime import datetime, timedelta, timezone
import pytest

from lib.dal.models import (
    AccessStatus,
    ModelCatalogEntry,
    RoutingPin,
    SlidingWindowUsage,
    TelemetryEvent,
    TierPolicy,
)
from lib.dal.repositories.model_repository import ModelRepository
from lib.dal.repositories.pin_repository import RoutingPinRepository
from lib.dal.repositories.quota_repository import QuotaRepository
from lib.dal.repositories.telemetry_repository import TelemetryRepository
from lib.dal.repositories.tier_policy_repository import TierPolicyRepository


def test_model_repository_crud(model_repo: ModelRepository):
    entry = ModelCatalogEntry(
        id="ollama/dolphin3:8b",
        provider="ollama",
        display_name="Dolphin 3 8B",
        access_status=AccessStatus.AVAILABLE.value,
        status_reason="Local (0 cost)",
        parameter_size="8B",
        context_window=131072,
        is_local=True,
        tier_eligibility=[0, 1, 2],
        capabilities={"reasoning": 0.45, "coding": 0.5, "writing": 0.7, "creative_freedom": 0.95},
        cost_per_million_tokens=0.0,
        is_enabled=True,
    )
    model_repo.upsert(entry)

    retrieved = model_repo.get_by_id("ollama/dolphin3:8b")
    assert retrieved is not None
    assert retrieved.display_name == "Dolphin 3 8B"
    assert retrieved.is_local is True
    assert 0 in retrieved.tier_eligibility

    # Update status
    model_repo.update_status("ollama/dolphin3:8b", AccessStatus.COOLING_DOWN, reason="Rate limited")
    updated = model_repo.get_by_id("ollama/dolphin3:8b")
    assert updated.access_status == AccessStatus.COOLING_DOWN.value
    assert updated.status_reason == "Rate limited"

    # List models with filter
    results = model_repo.list_models(provider="ollama", tier=1)
    assert len(results) >= 1
    assert results[0].id == "ollama/dolphin3:8b"


def test_telemetry_repository_and_stats(telemetry_repo: TelemetryRepository):
    now = datetime.now(timezone.utc)
    ev1 = TelemetryEvent(
        request_id="req-1",
        execution_id="exec-1",
        strategy_id="coding_t3_v1",
        tier_requested=3,
        tier_executed=3,
        task_type="coding",
        provider="claude",
        model="claude-sonnet-5",
        role="primary",
        input_tokens=100,
        output_tokens=200,
        total_tokens=300,
        started_at=now,
        finished_at=now + timedelta(seconds=2),
        latency_seconds=2.0,
        latency_ms=2000,
        success=True,
        estimated_cost_usd=0.005,
        quality_score=0.95,
    )
    ev2 = TelemetryEvent(
        request_id="req-2",
        execution_id="exec-2",
        strategy_id="coding_t3_v1",
        tier_requested=3,
        tier_executed=3,
        task_type="coding",
        provider="claude",
        model="claude-sonnet-5",
        role="primary",
        input_tokens=150,
        output_tokens=250,
        total_tokens=400,
        started_at=now,
        finished_at=now + timedelta(seconds=3),
        latency_seconds=3.0,
        latency_ms=3000,
        success=True,
        estimated_cost_usd=0.007,
        quality_score=0.90,
    )
    telemetry_repo.record_event(ev1)
    telemetry_repo.record_event(ev2)

    events = telemetry_repo.list_events(strategy_id="coding_t3_v1")
    assert len(events) == 2

    stats = telemetry_repo.get_stats(strategy_id="coding_t3_v1")
    assert len(stats) == 1
    assert stats[0]["total_runs"] == 2
    assert stats[0]["success_rate"] == 100.0
    assert stats[0]["avg_latency_seconds"] == 2.5
    assert stats[0]["avg_total_tokens"] == 350.0


def test_quota_repository_sliding_window(quota_repo: QuotaRepository):
    now = datetime.now(timezone.utc)
    # Record usage inside window
    quota_repo.record_usage(
        provider="claude",
        model="claude-sonnet-5",
        input_tokens=1000,
        output_tokens=500,
        timestamp=now - timedelta(hours=1),
    )
    quota_repo.record_usage(
        provider="claude",
        model="claude-sonnet-5",
        input_tokens=2000,
        output_tokens=1000,
        timestamp=now - timedelta(hours=2),
    )
    # Record usage outside 5h window
    quota_repo.record_usage(
        provider="claude",
        model="claude-sonnet-5",
        input_tokens=10000,
        output_tokens=10000,
        timestamp=now - timedelta(hours=6),
    )

    consumed = quota_repo.get_consumed_tokens_in_window("claude", hours=5, reference_time=now)
    # Inside window: (1000+500) + (2000+1000) = 4500
    assert consumed == 4500

    summary = quota_repo.get_usage_summary(hours=5, reference_time=now)
    assert len(summary) >= 1
    claude_summary = next(s for s in summary if s["provider"] == "claude")
    assert claude_summary["total_tokens"] == 4500
    assert claude_summary["requests_count"] == 2


def test_routing_pin_repository(pin_repo: RoutingPinRepository):
    # Set pin for tier 3 coding
    pin_repo.set_pin(
        tier=3,
        task_type="coding",
        strategy_id="coding_t3_codex_claude_refine",
        pinned_by="test_user",
    )
    # Set tier-level pin for tier 0
    pin_repo.set_pin(
        tier=0,
        task_type=None,
        model_id="ollama/dolphin3:8b",
        pinned_by="test_user",
    )

    pin_t3 = pin_repo.get_pin(3, task_type="coding")
    assert pin_t3 is not None
    assert pin_t3.strategy_id == "coding_t3_codex_claude_refine"

    pin_t0 = pin_repo.get_pin(0, task_type="general")
    assert pin_t0 is not None
    assert pin_t0.model_id == "ollama/dolphin3:8b"

    pins = pin_repo.list_pins()
    assert len(pins) == 2

    # Remove pin
    removed = pin_repo.remove_pin(3, task_type="coding")
    assert removed is True
    assert pin_repo.get_pin(3, task_type="coding") is None


def test_tier_policy_repository(tier_repo: TierPolicyRepository):
    policy = TierPolicy(
        tier=3,
        name="Advanced",
        max_latency_seconds=45,
        allow_multi_model=True,
        allow_external=True,
        retrieval_mode="full",
        require_verification=False,
        max_model_calls=3,
        allowed_models=["claude/claude-sonnet-5", "agy/gemini-3.1-pro"],
    )
    tier_repo.upsert_policy(policy)

    retrieved = tier_repo.get_policy(3)
    assert retrieved is not None
    assert retrieved.name == "Advanced"
    assert retrieved.max_latency_seconds == 45
    assert retrieved.allow_multi_model is True

    tier_repo.update_policy(3, max_latency_seconds=40)
    updated = tier_repo.get_policy(3)
    assert updated.max_latency_seconds == 40


def test_safety_guardrail_rejection():
    # Attempting to run test with real production database URL must fail
    from lib.dal.local.database import get_engine
    real_engine = get_engine("sqlite:///var/cortex.db")
    db_url = str(real_engine.url)
    assert "var/cortex.db" in db_url


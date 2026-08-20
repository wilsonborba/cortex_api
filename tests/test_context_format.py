from __future__ import annotations

from datetime import datetime, timedelta, timezone

import pytest

from lib.dal.models import AccessStatus, ModelCatalogEntry, TelemetryEvent
from lib.dal.repositories.model_repository import ModelRepository
from lib.dal.repositories.telemetry_repository import TelemetryRepository
from lib.engine.context_format import evaluate_and_update_context_format, get_preferred_context_format
from lib.engine.format import JSON, TOON


@pytest.fixture
def model_repo_() -> ModelRepository:
    return ModelRepository()


@pytest.fixture
def telemetry_repo_() -> TelemetryRepository:
    return TelemetryRepository()


def _model(**overrides) -> ModelCatalogEntry:
    defaults = dict(
        provider="ctxfmt", display_name="model", access_status=AccessStatus.AVAILABLE.value,
        context_window=8192, is_local=False, tier_eligibility=[], capabilities={},
        cost_per_million_tokens=0.0, is_enabled=True,
    )
    defaults.update(overrides)
    return ModelCatalogEntry(**defaults)


def _event(provider: str, model: str, context_format: str, context_type: str, success: bool, latency_seconds: float) -> TelemetryEvent:
    now = datetime.now(timezone.utc)
    return TelemetryEvent(
        request_id=f"req-{model}-{context_format}-{success}-{latency_seconds}",
        execution_id=f"exec-{model}-{context_format}-{success}-{latency_seconds}",
        strategy_id="s", tier_requested=1, tier_executed=1, task_type="general",
        provider=provider, model=model, role="primary", input_tokens=10, output_tokens=5,
        total_tokens=15, started_at=now, finished_at=now, latency_seconds=latency_seconds,
        latency_ms=int(latency_seconds * 1000), success=success,
        context_format=context_format, context_type=context_type,
    )


# --- ModelRepository: pin + computed persistence --------------------------------


def test_set_and_clear_context_format_pin(model_repo_: ModelRepository):
    model_repo_.upsert(_model(id="ctxfmt/pin-model"))

    pinned = model_repo_.set_context_format_pin("ctxfmt/pin-model", JSON)
    assert pinned.context_format_pin == JSON
    assert pinned.context_format_pin_expires_at is None

    cleared = model_repo_.clear_context_format_pin("ctxfmt/pin-model")
    assert cleared.context_format_pin is None
    assert cleared.context_format_pin_expires_at is None


def test_set_context_format_computed(model_repo_: ModelRepository):
    model_repo_.upsert(_model(id="ctxfmt/computed-model"))

    updated = model_repo_.set_context_format_computed("ctxfmt/computed-model", TOON)

    assert updated.context_format_computed == TOON


# --- get_preferred_context_format: layers 1+2 -------------------------------------


def test_default_with_no_data_is_toon():
    model = _model(id="ctxfmt/no-data")
    assert get_preferred_context_format(model) == TOON


def test_computed_value_is_used_when_set():
    model = _model(id="ctxfmt/computed", context_format_computed=JSON)
    assert get_preferred_context_format(model) == JSON


def test_active_pin_overrides_computed_value():
    model = _model(id="ctxfmt/pinned", context_format_computed=JSON, context_format_pin=TOON)
    assert get_preferred_context_format(model) == TOON


def test_pin_without_expiry_never_expires():
    model = _model(id="ctxfmt/pin-forever", context_format_pin=JSON, context_format_pin_expires_at=None)
    assert get_preferred_context_format(model) == JSON


def test_expired_pin_falls_back_to_computed():
    past = datetime.now(timezone.utc) - timedelta(minutes=1)
    model = _model(
        id="ctxfmt/pin-expired", context_format_computed=JSON, context_format_pin=TOON,
        context_format_pin_expires_at=past,
    )
    assert get_preferred_context_format(model) == JSON  # pin expired, computed wins


def test_non_expired_pin_is_still_honored():
    future = datetime.now(timezone.utc) + timedelta(minutes=10)
    model = _model(
        id="ctxfmt/pin-active", context_format_computed=JSON, context_format_pin=TOON,
        context_format_pin_expires_at=future,
    )
    assert get_preferred_context_format(model) == TOON


# --- TelemetryRepository.get_context_format_stats ---------------------------------


def test_get_context_format_stats_groups_by_type_and_format(telemetry_repo_: TelemetryRepository):
    telemetry_repo_.record_event(_event("ctxfmt", "ctxfmt/stats-model", TOON, "web", True, 1.0))
    telemetry_repo_.record_event(_event("ctxfmt", "ctxfmt/stats-model", TOON, "web", True, 3.0))
    telemetry_repo_.record_event(_event("ctxfmt", "ctxfmt/stats-model", JSON, "web", False, 2.0))
    telemetry_repo_.record_event(_event("ctxfmt", "ctxfmt/stats-model", None, None, True, 0.5))  # no context: excluded

    rows = telemetry_repo_.get_context_format_stats("ctxfmt", "ctxfmt/stats-model")
    by_format = {r["context_format"]: r for r in rows}

    assert set(by_format) == {TOON, JSON}
    assert by_format[TOON]["total_runs"] == 2
    assert by_format[TOON]["avg_latency_seconds"] == pytest.approx(2.0)
    assert by_format[TOON]["success_rate"] == pytest.approx(100.0)
    assert by_format[JSON]["success_rate"] == pytest.approx(0.0)


# --- evaluate_and_update_context_format --------------------------------------------


def test_evaluate_returns_none_with_no_data(model_repo_: ModelRepository, telemetry_repo_: TelemetryRepository):
    model_repo_.upsert(_model(id="ctxfmt/eval-empty"))

    result = evaluate_and_update_context_format(
        "ctxfmt", "ctxfmt/eval-empty", telemetry_repo=telemetry_repo_, model_repo=model_repo_, min_sample_size=5
    )

    assert result is None
    assert model_repo_.get_by_id("ctxfmt/eval-empty").context_format_computed is None


def test_evaluate_returns_none_when_only_one_format_has_enough_samples(
    model_repo_: ModelRepository, telemetry_repo_: TelemetryRepository
):
    model_repo_.upsert(_model(id="ctxfmt/eval-onesided"))
    for i in range(5):
        telemetry_repo_.record_event(_event("ctxfmt", "ctxfmt/eval-onesided", TOON, "web", True, 1.0 + i))

    result = evaluate_and_update_context_format(
        "ctxfmt", "ctxfmt/eval-onesided", telemetry_repo=telemetry_repo_, model_repo=model_repo_, min_sample_size=5
    )

    assert result is None  # only TOON has data; nothing to compare against


def test_evaluate_picks_the_format_with_higher_success_rate(
    model_repo_: ModelRepository, telemetry_repo_: TelemetryRepository
):
    model_repo_.upsert(_model(id="ctxfmt/eval-success"))
    for i in range(5):
        telemetry_repo_.record_event(_event("ctxfmt", "ctxfmt/eval-success", TOON, "web", True, 1.0))
    for i in range(5):
        telemetry_repo_.record_event(_event("ctxfmt", "ctxfmt/eval-success", JSON, "web", i > 2, 1.0))  # 2/5 success

    result = evaluate_and_update_context_format(
        "ctxfmt", "ctxfmt/eval-success", telemetry_repo=telemetry_repo_, model_repo=model_repo_, min_sample_size=5
    )

    assert result == TOON
    assert model_repo_.get_by_id("ctxfmt/eval-success").context_format_computed == TOON


def test_evaluate_breaks_a_success_rate_tie_with_latency(
    model_repo_: ModelRepository, telemetry_repo_: TelemetryRepository
):
    model_repo_.upsert(_model(id="ctxfmt/eval-tiebreak"))
    for i in range(5):
        telemetry_repo_.record_event(_event("ctxfmt", "ctxfmt/eval-tiebreak", TOON, "memory", True, 1.0))  # fast
    for i in range(5):
        telemetry_repo_.record_event(_event("ctxfmt", "ctxfmt/eval-tiebreak", JSON, "memory", True, 5.0))  # slow

    result = evaluate_and_update_context_format(
        "ctxfmt", "ctxfmt/eval-tiebreak", telemetry_repo=telemetry_repo_, model_repo=model_repo_, min_sample_size=5
    )

    assert result == TOON  # same 100% success rate, TOON wins on latency


def test_evaluate_does_not_overwrite_existing_computed_value_when_inconclusive(
    model_repo_: ModelRepository, telemetry_repo_: TelemetryRepository
):
    model_repo_.upsert(_model(id="ctxfmt/eval-keep", context_format_computed=JSON))
    # Only 2 TOON samples recorded: below the min_sample_size=5 threshold.
    telemetry_repo_.record_event(_event("ctxfmt", "ctxfmt/eval-keep", TOON, "web", True, 1.0))
    telemetry_repo_.record_event(_event("ctxfmt", "ctxfmt/eval-keep", TOON, "web", True, 1.0))

    result = evaluate_and_update_context_format(
        "ctxfmt", "ctxfmt/eval-keep", telemetry_repo=telemetry_repo_, model_repo=model_repo_, min_sample_size=5
    )

    assert result is None
    assert model_repo_.get_by_id("ctxfmt/eval-keep").context_format_computed == JSON  # untouched

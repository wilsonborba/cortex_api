from __future__ import annotations

from datetime import datetime, timezone

import pytest
from sqlalchemy.orm import Session

from lib.core.settings import Settings
from lib.dal.models import ModelCatalogEntry, TelemetryEvent
from lib.dal.repositories.model_repository import ModelRepository
from lib.dal.repositories.quota_repository import QuotaRepository
from lib.dal.repositories.telemetry_repository import TelemetryRepository
from lib.engine.quota import QuotaTracker
from lib.engine.scoring import ModelScorer, NEUTRAL_LATENCY_NORM, ScoringWeights


@pytest.fixture
def telemetry_repo_(db_session: Session) -> TelemetryRepository:
    return TelemetryRepository(session_factory=lambda: db_session)


@pytest.fixture
def quota_repo_(db_session: Session) -> QuotaRepository:
    return QuotaRepository(session_factory=lambda: db_session)


@pytest.fixture
def model_repo_(db_session: Session) -> ModelRepository:
    return ModelRepository(session_factory=lambda: db_session)


@pytest.fixture
def quota_tracker_(quota_repo_: QuotaRepository, model_repo_: ModelRepository) -> QuotaTracker:
    return QuotaTracker(
        quota_repo=quota_repo_,
        model_repo=model_repo_,
        settings=Settings(default_quota_window_tokens=10_000, quota_window_tokens_by_provider={}),
    )


def _model(**overrides) -> ModelCatalogEntry:
    defaults = dict(
        id="claude/scoring-sonnet",
        provider="claude",
        display_name="Claude Sonnet 5",
        access_status="AVAILABLE",
        context_window=200_000,
        is_local=False,
        tier_eligibility=[3, 4],
        capabilities={"coding": 0.9, "writing": 0.6},
        cost_per_million_tokens=5.0,
        is_enabled=True,
    )
    defaults.update(overrides)
    return ModelCatalogEntry(**defaults)


def test_score_uses_task_specific_capability(telemetry_repo_: TelemetryRepository, quota_tracker_: QuotaTracker):
    scorer = ModelScorer(quota_tracker=quota_tracker_, telemetry_repo=telemetry_repo_)

    coding_score = scorer.score(_model(), task_type="coding", max_latency_seconds=45)
    writing_score = scorer.score(_model(), task_type="writing", max_latency_seconds=45)

    assert coding_score.capability == 0.9
    assert writing_score.capability == 0.6
    assert coding_score.score > writing_score.score


def test_score_falls_back_to_average_capability_for_unknown_task(
    telemetry_repo_: TelemetryRepository, quota_tracker_: QuotaTracker
):
    scorer = ModelScorer(quota_tracker=quota_tracker_, telemetry_repo=telemetry_repo_)

    result = scorer.score(_model(), task_type="unknown_task", max_latency_seconds=45)

    assert result.capability == pytest.approx((0.9 + 0.6) / 2)


def test_score_uses_neutral_latency_when_no_telemetry_exists(
    telemetry_repo_: TelemetryRepository, quota_tracker_: QuotaTracker
):
    scorer = ModelScorer(quota_tracker=quota_tracker_, telemetry_repo=telemetry_repo_)

    result = scorer.score(_model(id="claude/scoring-no-history"), task_type="coding", max_latency_seconds=45)

    assert result.latency_norm == NEUTRAL_LATENCY_NORM


def test_score_normalizes_latency_against_tier_budget(
    telemetry_repo_: TelemetryRepository, quota_tracker_: QuotaTracker
):
    model = _model(id="claude/scoring-fast")
    now = datetime.now(timezone.utc)
    telemetry_repo_.record_event(
        TelemetryEvent(
            request_id="r1", execution_id="e1", strategy_id="s", tier_requested=3, tier_executed=3,
            task_type="coding", provider=model.provider, model=model.id, input_tokens=10, output_tokens=10,
            total_tokens=20, started_at=now, finished_at=now, latency_seconds=9.0, latency_ms=9000, success=True,
        )
    )
    scorer = ModelScorer(quota_tracker=quota_tracker_, telemetry_repo=telemetry_repo_)

    result = scorer.score(model, task_type="coding", max_latency_seconds=45)

    assert result.latency_norm == pytest.approx(9.0 / 45)


def test_score_penalizes_expensive_models(telemetry_repo_: TelemetryRepository, quota_tracker_: QuotaTracker):
    scorer = ModelScorer(
        quota_tracker=quota_tracker_, telemetry_repo=telemetry_repo_,
        weights=ScoringWeights(capability=0.0, quota=0.0, latency=0.0, cost=1.0), cost_ceiling=10.0,
    )

    cheap = scorer.score(_model(id="claude/scoring-cheap", cost_per_million_tokens=1.0), "coding", 45)
    pricey = scorer.score(_model(id="claude/scoring-pricey", cost_per_million_tokens=9.0), "coding", 45)

    assert cheap.score > pricey.score


def test_score_rewards_quota_availability(
    telemetry_repo_: TelemetryRepository, quota_repo_: QuotaRepository, model_repo_: ModelRepository
):
    quota_repo_.record_usage(provider="codex", model="codex/scoring-o3", input_tokens=9000, output_tokens=500)
    tracker = QuotaTracker(
        quota_repo=quota_repo_,
        model_repo=model_repo_,
        settings=Settings(
            default_quota_window_tokens=10_000, quota_window_tokens_by_provider={"codex": 10_000}
        ),
    )
    scorer = ModelScorer(
        quota_tracker=tracker, telemetry_repo=telemetry_repo_,
        weights=ScoringWeights(capability=0.0, quota=1.0, latency=0.0, cost=0.0),
    )

    starved = scorer.score(_model(id="codex/scoring-o3", provider="codex"), "coding", 45)
    fresh = scorer.score(_model(id="claude/scoring-fresh", provider="claude"), "coding", 45)

    assert fresh.score > starved.score

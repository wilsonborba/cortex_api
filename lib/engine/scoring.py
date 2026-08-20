from __future__ import annotations

from dataclasses import dataclass
from typing import Optional

from lib.core.settings import Settings, get_settings
from lib.dal.models import ModelCatalogEntry
from lib.dal.repositories.telemetry_repository import TelemetryRepository
from lib.engine.quota import QuotaTracker

# Neither "good" nor "bad": until telemetry exists for a model, its latency
# term contributes neither a bonus nor a penalty.
NEUTRAL_LATENCY_NORM = 0.5
DEFAULT_CAPABILITY = 0.5


@dataclass(frozen=True)
class ScoringWeights:
    capability: float = 0.5
    quota: float = 0.25
    latency: float = 0.15
    cost: float = 0.10


@dataclass(frozen=True)
class ModelScore:
    model_id: str
    provider: str
    score: float
    capability: float
    quota_factor: float
    latency_norm: float
    cost_norm: float


class ModelScorer:
    """Score(M) = w_cap*Cap(M,task) + w_q*Q(M) - w_lat*LatencyNorm(M) - w_cost*CostNorm(M)"""

    def __init__(
        self,
        quota_tracker: QuotaTracker,
        telemetry_repo: Optional[TelemetryRepository] = None,
        weights: Optional[ScoringWeights] = None,
        cost_ceiling: float = 20.0,
    ) -> None:
        self._quota_tracker = quota_tracker
        self._telemetry_repo = telemetry_repo or TelemetryRepository()
        self._weights = weights or ScoringWeights()
        self._cost_ceiling = cost_ceiling

    def score(self, model: ModelCatalogEntry, task_type: str, max_latency_seconds: int) -> ModelScore:
        capability = self._capability(model, task_type)
        quota_factor = self._quota_tracker.get_quota(model.provider).quota_factor
        latency_norm = self._latency_norm(model, max_latency_seconds)
        cost_norm = self._cost_norm(model)
        w = self._weights
        raw_score = (
            w.capability * capability
            + w.quota * quota_factor
            - w.latency * latency_norm
            - w.cost * cost_norm
        )
        return ModelScore(
            model_id=model.id,
            provider=model.provider,
            score=raw_score,
            capability=capability,
            quota_factor=quota_factor,
            latency_norm=latency_norm,
            cost_norm=cost_norm,
        )

    @staticmethod
    def _capability(model: ModelCatalogEntry, task_type: str) -> float:
        capabilities = model.capabilities or {}
        if task_type in capabilities:
            return float(capabilities[task_type])
        if capabilities:
            return float(sum(capabilities.values()) / len(capabilities))
        return DEFAULT_CAPABILITY

    def _latency_norm(self, model: ModelCatalogEntry, max_latency_seconds: int) -> float:
        # Keyed by the full catalog id (e.g. "claude/claude-sonnet-5"), matching
        # how the Quota Tracker and Executor are expected to record it.
        stats = self._telemetry_repo.get_model_latency_stats(model.provider, model.id)
        if not stats or max_latency_seconds <= 0:
            return NEUTRAL_LATENCY_NORM
        return min(1.0, max(0.0, stats["avg_latency_seconds"] / max_latency_seconds))

    def _cost_norm(self, model: ModelCatalogEntry) -> float:
        if self._cost_ceiling <= 0:
            return 0.0
        return min(1.0, max(0.0, model.cost_per_million_tokens / self._cost_ceiling))


def build_default_scorer(
    settings: Optional[Settings] = None, quota_tracker: Optional[QuotaTracker] = None
) -> ModelScorer:
    settings = settings or get_settings()
    weights = ScoringWeights(
        capability=settings.routing_weight_capability,
        quota=settings.routing_weight_quota,
        latency=settings.routing_weight_latency,
        cost=settings.routing_weight_cost,
    )
    return ModelScorer(
        quota_tracker=quota_tracker or QuotaTracker(settings=settings),
        weights=weights,
        cost_ceiling=settings.routing_cost_ceiling_usd_per_million,
    )

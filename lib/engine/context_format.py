from __future__ import annotations

from datetime import datetime, timezone
from typing import Dict, Optional

from lib.core.time_utils import ensure_utc
from lib.dal.models import ModelCatalogEntry
from lib.dal.repositories.model_repository import ModelRepository
from lib.dal.repositories.telemetry_repository import TelemetryRepository
from lib.engine.format import JSON, TOON

# Below this many recorded runs for a format, there isn't enough signal to
# trust it over the current default -- leave the computed preference alone
# rather than flip-flopping on thin data.
MIN_SAMPLE_SIZE = 5


def get_preferred_context_format(model: ModelCatalogEntry) -> str:
    """Layers 1 (computed) and 2 (pin) of the 3-layer config -- layer 3
    (per-request force) is resolved by the caller in #16, this function
    doesn't know about it.

    Precedence: an active (non-expired) pin wins; otherwise the algorithm's
    computed preference; otherwise TOON, the hard default when there's no
    data yet at all ("deve tentar usar o máximo o TOON").
    """
    if model.context_format_pin and not _pin_expired(model):
        return model.context_format_pin
    return model.context_format_computed or TOON


def _pin_expired(model: ModelCatalogEntry) -> bool:
    if model.context_format_pin_expires_at is None:
        return False  # no expiry set: the pin lasts until explicitly cleared
    return ensure_utc(model.context_format_pin_expires_at) <= datetime.now(timezone.utc)


def evaluate_and_update_context_format(
    provider: str,
    model_id: str,
    telemetry_repo: Optional[TelemetryRepository] = None,
    model_repo: Optional[ModelRepository] = None,
    min_sample_size: int = MIN_SAMPLE_SIZE,
) -> Optional[str]:
    """Looks at accumulated telemetry for one model and updates its
    `context_format_computed` (layer 1) if the data clearly favors one
    format over the other. Returns the format it set, or None if there
    wasn't enough data on *both* sides to make a real comparison (the
    existing computed value, if any, is left untouched -- no flip-flopping
    on thin data).

    Comparison order: success rate first (a format that fails more often is
    worse regardless of how fast/cheap it is when it works), latency as the
    tiebreaker. Intentionally simple -- per docs/specs.md's own philosophy,
    "rules + metrics" is the right amount of sophistication for now; a
    contextual-bandit-style continuous re-exploration is explicitly future
    work, not this issue's scope.
    """
    telemetry_repo = telemetry_repo or TelemetryRepository()
    model_repo = model_repo or ModelRepository()

    rows = telemetry_repo.get_context_format_stats(provider, model_id)
    totals: Dict[str, Dict[str, float]] = {}
    for row in rows:
        fmt = row["context_format"]
        bucket = totals.setdefault(fmt, {"total_runs": 0.0, "success_weighted": 0.0, "latency_weighted": 0.0})
        bucket["total_runs"] += row["total_runs"]
        bucket["success_weighted"] += row["success_rate"] * row["total_runs"]
        bucket["latency_weighted"] += row["avg_latency_seconds"] * row["total_runs"]

    candidates: Dict[str, Dict[str, float]] = {}
    for fmt, bucket in totals.items():
        if bucket["total_runs"] < min_sample_size:
            continue
        candidates[fmt] = {
            "success_rate": bucket["success_weighted"] / bucket["total_runs"],
            "avg_latency_seconds": bucket["latency_weighted"] / bucket["total_runs"],
        }

    if TOON not in candidates or JSON not in candidates:
        return None  # not enough data on both sides yet to compare for real

    toon_stats, json_stats = candidates[TOON], candidates[JSON]
    if toon_stats["success_rate"] > json_stats["success_rate"]:
        winner = TOON
    elif json_stats["success_rate"] > toon_stats["success_rate"]:
        winner = JSON
    else:
        winner = TOON if toon_stats["avg_latency_seconds"] <= json_stats["avg_latency_seconds"] else JSON

    model_repo.set_context_format_computed(model_id, winner)
    return winner

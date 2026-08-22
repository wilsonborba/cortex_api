from __future__ import annotations

from dataclasses import dataclass
from typing import Dict, List, Optional

from lib.dal.models import TierPolicy
from lib.dal.repositories.tier_policy_repository import TierPolicyRepository
from lib.engine.curated_tier_catalog import model_ids_for_tier

MIN_TIER = 0
MAX_TIER = 5


@dataclass(frozen=True)
class TierPreset:
    tier: int
    name: str
    max_latency_seconds: int
    allow_multi_model: bool
    allow_external: bool
    retrieval_mode: str
    require_verification: bool
    max_model_calls: int


# Factory presets (config layer 1): the envelope matrix from
# docs/tier-and-execution-envelope.md. T0 is local-only and single-shot;
# T5 is the only tier that requires a dedicated critic by default.
FACTORY_PRESETS: Dict[int, TierPreset] = {
    0: TierPreset(0, "Basic", 300, False, False, "none", False, 1),
    1: TierPreset(1, "Light", 300, False, True, "simple", False, 1),
    2: TierPreset(2, "Standard", 300, True, True, "vector_rerank", False, 2),
    3: TierPreset(3, "Advanced", 300, True, True, "full", False, 2),
    4: TierPreset(4, "High", 300, True, True, "deep", False, 3),
    5: TierPreset(5, "Ultra", 300, True, True, "deep_web", True, 4),
}


def preset_to_policy(preset: TierPreset) -> TierPolicy:
    return TierPolicy(
        tier=preset.tier,
        name=preset.name,
        max_latency_seconds=preset.max_latency_seconds,
        allow_multi_model=preset.allow_multi_model,
        allow_external=preset.allow_external,
        retrieval_mode=preset.retrieval_mode,
        require_verification=preset.require_verification,
        max_model_calls=preset.max_model_calls,
        allowed_models=model_ids_for_tier(preset.tier),
    )


class TierService:
    """Config layers 1 (factory presets) and 2 (persistent SQLite rules).

    Layer 3 (request-level `--force-*` overrides) lives in the Router: it
    never touches persisted tier policy, it just bypasses it for one call.
    """

    def __init__(self, repo: Optional[TierPolicyRepository] = None) -> None:
        self._repo = repo or TierPolicyRepository()

    def ensure_seeded(self) -> None:
        """Idempotent: inserts missing tiers and updates the normal budget to 300s."""
        existing = {p.tier: p for p in self._repo.list_policies()}
        for tier, preset in FACTORY_PRESETS.items():
            if tier not in existing:
                self._repo.upsert_policy(preset_to_policy(preset))
            else:
                self._repo.update_policy(tier, max_latency_seconds=300)

    def get_envelope(self, tier: int) -> TierPolicy:
        policy = self._repo.get_policy(tier)
        if policy is not None:
            return policy
        preset = FACTORY_PRESETS.get(tier)
        if preset is None:
            raise ValueError(f"Unknown tier: {tier} (expected {MIN_TIER}-{MAX_TIER})")
        return preset_to_policy(preset)  # transient fallback; call ensure_seeded() to persist it

    def list_envelopes(self) -> List[TierPolicy]:
        self.ensure_seeded()
        return self._repo.list_policies()

    def set_models(self, tier: int, models: List[str]) -> Optional[TierPolicy]:
        return self._repo.update_policy(tier, allowed_models=models)

    def add_model(self, tier: int, model_id: str) -> Optional[TierPolicy]:
        policy = self._repo.get_policy(tier)
        current = list(policy.allowed_models) if policy and policy.allowed_models else []
        if model_id not in current:
            current.append(model_id)
        return self._repo.update_policy(tier, allowed_models=current)

    def remove_model(self, tier: int, model_id: str) -> Optional[TierPolicy]:
        policy = self._repo.get_policy(tier)
        if not policy or not policy.allowed_models:
            return policy
        return self._repo.update_policy(
            tier, allowed_models=[m for m in policy.allowed_models if m != model_id]
        )

    def configure(
        self,
        tier: int,
        max_latency_seconds: Optional[int] = None,
        allow_multi_model: Optional[bool] = None,
        allow_external: Optional[bool] = None,
        retrieval_mode: Optional[str] = None,
        require_verification: Optional[bool] = None,
        max_model_calls: Optional[int] = None,
    ) -> Optional[TierPolicy]:
        return self._repo.update_policy(
            tier,
            max_latency_seconds=max_latency_seconds,
            allow_multi_model=allow_multi_model,
            allow_external=allow_external,
            retrieval_mode=retrieval_mode,
            require_verification=require_verification,
            max_model_calls=max_model_calls,
        )

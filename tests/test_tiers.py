from __future__ import annotations

import pytest
from sqlalchemy.orm import Session

from lib.dal.repositories.tier_policy_repository import TierPolicyRepository
from lib.engine.tiers import FACTORY_PRESETS, TierService


@pytest.fixture
def tier_repo_(db_session: Session) -> TierPolicyRepository:
    return TierPolicyRepository(session_factory=lambda: db_session)


def test_ensure_seeded_inserts_all_factory_presets(tier_repo_: TierPolicyRepository):
    service = TierService(repo=tier_repo_)

    service.ensure_seeded()

    tiers = {p.tier for p in tier_repo_.list_policies()}
    assert tiers >= set(FACTORY_PRESETS.keys())


def test_ensure_seeded_enforces_the_global_latency_budget(tier_repo_: TierPolicyRepository):
    service = TierService(repo=tier_repo_)
    service.ensure_seeded()
    service.configure(0, max_latency_seconds=999)

    service.ensure_seeded()

    assert tier_repo_.get_policy(0).max_latency_seconds == 300


def test_get_envelope_falls_back_to_preset_when_unseeded(tier_repo_: TierPolicyRepository):
    service = TierService(repo=tier_repo_)  # ensure_seeded() never called

    envelope = service.get_envelope(0)

    assert envelope.name == "Basic"
    assert envelope.allow_external is False  # T0 is local-only per the envelope matrix


def test_get_envelope_rejects_unknown_tier(tier_repo_: TierPolicyRepository):
    service = TierService(repo=tier_repo_)

    with pytest.raises(ValueError):
        service.get_envelope(9)


def test_set_models_replaces_allowed_models(tier_repo_: TierPolicyRepository):
    service = TierService(repo=tier_repo_)
    service.ensure_seeded()

    service.set_models(5, ["ollama/dolphin3:8b"])

    assert tier_repo_.get_policy(5).allowed_models == ["ollama/dolphin3:8b"]


def test_add_model_appends_without_duplicating(tier_repo_: TierPolicyRepository):
    service = TierService(repo=tier_repo_)
    service.ensure_seeded()
    service.set_models(5, ["ollama/dolphin3:8b"])

    service.add_model(5, "codex/o3")
    service.add_model(5, "codex/o3")  # duplicate: must not append twice

    assert tier_repo_.get_policy(5).allowed_models == ["ollama/dolphin3:8b", "codex/o3"]


def test_remove_model_drops_only_that_model(tier_repo_: TierPolicyRepository):
    service = TierService(repo=tier_repo_)
    service.ensure_seeded()
    service.set_models(5, ["ollama/dolphin3:8b", "codex/o3"])

    service.remove_model(5, "codex/o3")

    assert tier_repo_.get_policy(5).allowed_models == ["ollama/dolphin3:8b"]


def test_configure_updates_only_provided_fields(tier_repo_: TierPolicyRepository):
    service = TierService(repo=tier_repo_)
    service.ensure_seeded()
    original = tier_repo_.get_policy(3)

    service.configure(3, max_latency_seconds=40)

    updated = tier_repo_.get_policy(3)
    assert updated.max_latency_seconds == 40
    assert updated.allow_external == original.allow_external  # untouched

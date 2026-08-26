from __future__ import annotations

from typing import Iterable, Optional

import pytest
from sqlalchemy.orm import Session

from lib.core.settings import Settings
from lib.dal.models import AccessStatus, ModelCatalogEntry
from lib.dal.repositories.model_repository import ModelRepository
from lib.dal.repositories.pin_repository import RoutingPinRepository
from lib.dal.repositories.quota_repository import QuotaRepository
from lib.dal.repositories.telemetry_repository import TelemetryRepository
from lib.dal.repositories.tier_policy_repository import TierPolicyRepository
from lib.engine.quota import QuotaTracker
from lib.engine.registry_service import ModelRegistryService
from lib.engine.attachments import Attachment
from lib.engine.router import NoEligibleModelError, Router, RoutingRequest
from lib.engine.scoring import ModelScorer, ScoringWeights
from lib.engine.tiers import TierService
from lib.engine.curated_tier_catalog import model_ids_for_tier


@pytest.fixture
def model_repo_(db_session: Session) -> ModelRepository:
    return ModelRepository(session_factory=lambda: db_session)


@pytest.fixture
def tier_repo_(db_session: Session) -> TierPolicyRepository:
    return TierPolicyRepository(session_factory=lambda: db_session)


@pytest.fixture
def pin_repo_(db_session: Session) -> RoutingPinRepository:
    return RoutingPinRepository(session_factory=lambda: db_session)


@pytest.fixture
def tier_service_(tier_repo_: TierPolicyRepository) -> TierService:
    service = TierService(repo=tier_repo_)
    service.ensure_seeded()
    return service


@pytest.fixture
def router_(
    model_repo_: ModelRepository,
    tier_service_: TierService,
    pin_repo_: RoutingPinRepository,
    db_session: Session,
) -> Router:
    settings = Settings(
        quota_critical_threshold=0.15, default_quota_window_tokens=10_000, quota_window_tokens_by_provider={}
    )
    quota_tracker = QuotaTracker(
        quota_repo=QuotaRepository(session_factory=lambda: db_session),
        model_repo=model_repo_,
        settings=settings,
    )
    scorer = ModelScorer(
        quota_tracker=quota_tracker,
        telemetry_repo=TelemetryRepository(session_factory=lambda: db_session),
        weights=ScoringWeights(capability=0.5, quota=0.25, latency=0.15, cost=0.10),
    )
    return Router(
        registry=ModelRegistryService(discoveries=[], repository=model_repo_),
        quota_tracker=quota_tracker,
        scorer=scorer,
        tier_service=tier_service_,
        pin_repo=pin_repo_,
        settings=settings,
    )


def _seed_model(repo: ModelRepository, **overrides) -> ModelCatalogEntry:
    defaults = dict(
        provider="ollama",
        display_name="model",
        access_status=AccessStatus.AVAILABLE.value,
        context_window=8192,
        is_local=True,
        tier_eligibility=[0, 1, 2],
        capabilities={},
        cost_per_million_tokens=0.0,
        is_enabled=True,
    )
    defaults.update(overrides)
    return repo.upsert(ModelCatalogEntry(**defaults))


def _restrict(tier_service: TierService, tier: int, model_ids: Iterable[str]) -> None:
    """The test DB persists across this whole file's tests (see conftest.py),
    so any test that cares about an exact candidate set scopes the tier's
    Layer 2 `allowed_models` to just what it seeded -- this is real Router
    behavior (a user restricting a tier's eligible models), not a test hack.
    """
    tier_service.set_models(tier, list(model_ids))


def _dynamic(pin_repo: RoutingPinRepository, tier: int, task_type: Optional[str] = None) -> None:
    """Same persistence caveat as `_restrict`, but for pins: clears both the
    (tier, task_type) and tier-level (tier, None) pin slots so a test meant
    to exercise *dynamic* scoring can't be silently hijacked by a pin some
    other test (in this file or another) left active for the same tier.
    `RoutingPinRepository.get_pin()` checks exactly those two slots.
    """
    if task_type is not None:
        pin_repo.remove_pin(tier, task_type=task_type)
    pin_repo.remove_pin(tier, task_type=None)


# --- tier resolution -------------------------------------------------------------


def test_resolve_tier_explicit_wins_over_directive(router_: Router):
    tier, prompt = router_.resolve_tier(1, "/t4 hello")
    assert tier == 1
    assert prompt == "hello"  # directive is still stripped


def test_resolve_tier_directive_wins_over_auto(router_: Router):
    tier, prompt = router_.resolve_tier(None, "/t4 hello")
    assert tier == 4
    assert prompt == "hello"


def test_resolve_tier_auto_uses_classifier(router_: Router):
    tier, prompt = router_.resolve_tier("auto", "hi")
    assert tier == 0
    assert prompt == "hi"


# --- dynamic scoring ---------------------------------------------------------------


def test_dynamic_routing_picks_highest_scoring_candidate(
    router_: Router, model_repo_: ModelRepository, tier_service_: TierService, pin_repo_: RoutingPinRepository
):
    _dynamic(pin_repo_, 1, "coding")
    _seed_model(model_repo_, id="ollama/router-weak", tier_eligibility=[0, 1, 2], capabilities={"coding": 0.2})
    _seed_model(model_repo_, id="ollama/router-strong", tier_eligibility=[0, 1, 2], capabilities={"coding": 0.9})
    _restrict(tier_service_, 1, ["ollama/router-weak", "ollama/router-strong"])

    plan = router_.build_execution_plan(RoutingRequest(prompt="hi", tier=1, task_type="coding"))

    assert plan.source == "dynamic"
    assert plan.selections[0].model_id == "ollama/router-strong"


def test_curated_tier_one_preserves_order_then_adds_local_ollama_fallback(
    router_: Router, model_repo_: ModelRepository, tier_service_: TierService, pin_repo_: RoutingPinRepository
):
    _dynamic(pin_repo_, 1, "general")
    curated = model_ids_for_tier(1)
    for index, model_id in enumerate(curated):
        _seed_model(
            model_repo_, id=model_id, provider=f"remote{index}", is_local=False,
            tier_eligibility=[1], capabilities={"general": 0.99 - index / 100},
        )
    _seed_model(
        model_repo_, id="ollama/hf.co/ThalisAI/Qwen3-VL-8B-Instruct-heretic:Q8_0", provider="ollama", is_local=True,
        tier_eligibility=[0, 1], capabilities={"general": 0.1},
    )
    tier_service_.set_models(1, curated)

    plan = router_.build_execution_plan(RoutingRequest(prompt="hi", tier=1))

    assert [selection.model_id for selection in plan.selections] == curated + ["ollama/hf.co/ThalisAI/Qwen3-VL-8B-Instruct-heretic:Q8_0"]
    assert [selection.role for selection in plan.selections] == ["primary"] + ["fallback"] * 5


def test_curated_tier_two_does_not_admit_a_model_reserved_for_tier_three(
    router_: Router, model_repo_: ModelRepository, tier_service_: TierService, pin_repo_: RoutingPinRepository
):
    _dynamic(pin_repo_, 2, "general")
    curated = model_ids_for_tier(2)
    for index, model_id in enumerate(curated):
        _seed_model(model_repo_, id=model_id, provider=f"remote{index}", is_local=False, tier_eligibility=[2])
    _seed_model(model_repo_, id="mistral/reserved-t3", provider="mistral", is_local=False, tier_eligibility=[3])
    _seed_model(
        model_repo_, id="ollama/hf.co/ThalisAI/Qwen3-VL-8B-Instruct-heretic:Q8_0",
        provider="ollama", is_local=True, tier_eligibility=[0, 1],
    )
    tier_service_.set_models(2, curated)

    plan = router_.build_execution_plan(RoutingRequest(prompt="hi", tier=2))

    assert "mistral/reserved-t3" not in [selection.model_id for selection in plan.selections]
    assert [selection.model_id for selection in plan.selections] == curated + ["ollama/hf.co/ThalisAI/Qwen3-VL-8B-Instruct-heretic:Q8_0"]
    assert plan.selections[0].role == "primary"


def test_dynamic_routing_respects_tier_eligibility(
    router_: Router, model_repo_: ModelRepository, tier_service_: TierService, pin_repo_: RoutingPinRepository
):
    _dynamic(pin_repo_, 3, "general")
    _seed_model(model_repo_, id="ollama/router-t0-only", tier_eligibility=[0])
    _restrict(tier_service_, 3, [])

    with pytest.raises(NoEligibleModelError):
        router_.build_execution_plan(RoutingRequest(prompt="hi", tier=3, task_type="general"))


def test_tier_zero_text_uses_configured_local_text_role(
    router_: Router, model_repo_: ModelRepository, tier_service_: TierService, pin_repo_: RoutingPinRepository
):
    _dynamic(pin_repo_, 0, "general")
    _seed_model(
        model_repo_, id="ollama/hf.co/ThalisAI/Qwen3-VL-8B-Instruct-heretic:Q8_0",
        provider="ollama", is_local=True, tier_eligibility=[0], capabilities={"vision": False},
    )

    plan = router_.build_execution_plan(RoutingRequest(prompt="hi", tier=0, task_type="general"))

    assert plan.source == "local-role-policy"
    assert plan.selections[0].model_id == "ollama/hf.co/ThalisAI/Qwen3-VL-8B-Instruct-heretic:Q8_0"


def test_image_attachment_uses_configured_vision_role_even_with_text_override(
    router_: Router, model_repo_: ModelRepository, tier_service_: TierService, pin_repo_: RoutingPinRepository
):
    _dynamic(pin_repo_, 0, "general")
    _seed_model(
        model_repo_, id="ollama/qwen2.5vl:7b", tier_eligibility=[0], capabilities={"vision": True},
    )
    _seed_model(
        model_repo_, id="ollama/hf.co/ThalisAI/Qwen3-VL-8B-Instruct-heretic:Q8_0",
        tier_eligibility=[0], capabilities={"vision": False},
    )

    plan = router_.build_execution_plan(
        RoutingRequest(
            prompt="describe this", tier=0,
            force_model="ollama/hf.co/ThalisAI/Qwen3-VL-8B-Instruct-heretic:Q8_0",
            attachments=[Attachment(filename="sample.png", mime_type="image/png", data_base64="aGVsbG8=")],
        )
    )

    assert plan.source == "local-role-policy"
    assert plan.selections[0].model_id == "ollama/qwen2.5vl:7b"


def test_image_attachment_fails_when_vision_role_is_not_explicitly_marked_capable(
    router_: Router, model_repo_: ModelRepository, tier_service_: TierService, pin_repo_: RoutingPinRepository
):
    _dynamic(pin_repo_, 0, "general")
    _seed_model(model_repo_, id="ollama/qwen2.5vl:7b", tier_eligibility=[0], capabilities={"vision": False})

    with pytest.raises(NoEligibleModelError, match="marked vision-capable"):
        router_.build_execution_plan(
            RoutingRequest(
                prompt="describe this", tier=0,
                attachments=[Attachment(filename="sample.png", mime_type="image/png", data_base64="aGVsbG8=")],
            )
        )


def test_dynamic_routing_respects_persistent_allowed_models_layer(
    router_: Router, model_repo_: ModelRepository, tier_service_: TierService, pin_repo_: RoutingPinRepository
):
    _dynamic(pin_repo_, 1, "general")
    _seed_model(model_repo_, id="ollama/router-a", tier_eligibility=[0, 1, 2])
    _seed_model(model_repo_, id="ollama/router-b", tier_eligibility=[0, 1, 2])
    _restrict(tier_service_, 1, ["ollama/router-b"])

    plan = router_.build_execution_plan(RoutingRequest(prompt="hi", tier=1, task_type="general"))

    assert plan.selections[0].model_id == "ollama/router-b"


def test_no_eligible_model_raises_when_nothing_available(
    router_: Router, tier_service_: TierService, pin_repo_: RoutingPinRepository
):
    _dynamic(pin_repo_, 2, "general")
    _restrict(tier_service_, 2, [])

    with pytest.raises(NoEligibleModelError):
        router_.build_execution_plan(RoutingRequest(prompt="hi", tier=2, task_type="general"))


# --- quota-aware routing -----------------------------------------------------------


def test_low_quota_model_is_passed_over_for_a_healthier_alternative(
    router_: Router,
    model_repo_: ModelRepository,
    tier_service_: TierService,
    pin_repo_: RoutingPinRepository,
    db_session: Session,
):
    _dynamic(pin_repo_, 3, "coding")
    quota_repo = QuotaRepository(session_factory=lambda: db_session)
    _seed_model(
        model_repo_, id="codex/router-starved", provider="codex", is_local=False,
        tier_eligibility=[3], capabilities={"coding": 0.99},  # best capability, but...
    )
    _seed_model(
        model_repo_, id="claude/router-healthy", provider="claude", is_local=False,
        tier_eligibility=[3], capabilities={"coding": 0.5},
    )
    _restrict(tier_service_, 3, ["codex/router-starved", "claude/router-healthy"])
    quota_repo.record_usage(provider="codex", model="codex/router-starved", input_tokens=9900, output_tokens=50)

    plan = router_.build_execution_plan(RoutingRequest(prompt="hi", tier=3, task_type="coding"))

    # The starved model has better raw capability but gets excluded from the
    # preferred pool outright, so this is the *normal* path, not a degraded one.
    assert plan.selections[0].model_id == "claude/router-healthy"
    assert "quota critical threshold" not in plan.reason


def test_falls_back_to_low_quota_model_when_it_is_the_only_candidate(
    router_: Router,
    model_repo_: ModelRepository,
    tier_service_: TierService,
    pin_repo_: RoutingPinRepository,
    db_session: Session,
):
    _dynamic(pin_repo_, 3, "general")
    quota_repo = QuotaRepository(session_factory=lambda: db_session)
    _seed_model(
        model_repo_, id="codex/router-only-starved", provider="codex", is_local=False, tier_eligibility=[3],
    )
    _restrict(tier_service_, 3, ["codex/router-only-starved"])
    quota_repo.record_usage(provider="codex", model="codex/router-only-starved", input_tokens=9900, output_tokens=50)

    plan = router_.build_execution_plan(RoutingRequest(prompt="hi", tier=3, task_type="general"))

    assert plan.selections[0].model_id == "codex/router-only-starved"
    assert "quota critical threshold" in plan.reason


# --- manual pinning -----------------------------------------------------------------


def test_strategy_pin_takes_precedence_over_dynamic_scoring(
    router_: Router, model_repo_: ModelRepository, tier_service_: TierService, pin_repo_: RoutingPinRepository
):
    _seed_model(model_repo_, id="ollama/router-ignored", tier_eligibility=[0, 1, 2])
    _restrict(tier_service_, 1, ["ollama/router-ignored"])
    pin_repo_.set_pin(tier=1, task_type="coding", strategy_id="coding_t1_pinned_v1", pinned_by="benchmark")

    plan = router_.build_execution_plan(RoutingRequest(prompt="hi", tier=1, task_type="coding"))

    assert plan.source == "pin"
    assert plan.strategy_id == "coding_t1_pinned_v1"
    assert plan.selections == []


def test_model_pin_takes_precedence_over_dynamic_scoring(
    router_: Router, model_repo_: ModelRepository, tier_service_: TierService, pin_repo_: RoutingPinRepository
):
    _seed_model(model_repo_, id="ollama/router-pinned", tier_eligibility=[1])
    _seed_model(model_repo_, id="ollama/router-would-win", tier_eligibility=[1], capabilities={"general": 0.99})
    _restrict(tier_service_, 1, ["ollama/router-pinned", "ollama/router-would-win"])
    pin_repo_.set_pin(tier=1, model_id="ollama/router-pinned", pinned_by="benchmark")

    plan = router_.build_execution_plan(RoutingRequest(prompt="hi", tier=1, task_type="general"))

    assert plan.source == "pin"
    assert plan.selections[0].model_id == "ollama/router-pinned"


def test_force_model_overrides_an_active_pin(
    router_: Router, model_repo_: ModelRepository, tier_service_: TierService, pin_repo_: RoutingPinRepository
):
    _seed_model(model_repo_, id="ollama/router-pinned-2", tier_eligibility=[1])
    _seed_model(model_repo_, id="ollama/router-forced", tier_eligibility=[1])
    _restrict(tier_service_, 1, ["ollama/router-pinned-2", "ollama/router-forced"])
    pin_repo_.set_pin(tier=1, model_id="ollama/router-pinned-2", pinned_by="benchmark")

    plan = router_.build_execution_plan(
        RoutingRequest(prompt="hi", tier=1, task_type="general", force_model="ollama/router-forced")
    )

    assert plan.source == "override"
    assert plan.selections[0].model_id == "ollama/router-forced"


def test_force_strategy_produces_an_empty_selection_plan(
    router_: Router, model_repo_: ModelRepository, tier_service_: TierService, pin_repo_: RoutingPinRepository
):
    _dynamic(pin_repo_, 1, "general")
    _seed_model(model_repo_, id="ollama/router-unused", tier_eligibility=[1])
    _restrict(tier_service_, 1, ["ollama/router-unused"])

    plan = router_.build_execution_plan(
        RoutingRequest(prompt="hi", tier=1, force_strategy="coding_t1_custom_v1")
    )

    assert plan.source == "override"
    assert plan.strategy_id == "coding_t1_custom_v1"
    assert plan.selections == []


def test_force_model_raises_when_it_does_not_match_any_candidate(
    router_: Router, model_repo_: ModelRepository, tier_service_: TierService, pin_repo_: RoutingPinRepository
):
    _dynamic(pin_repo_, 1, "general")
    _seed_model(model_repo_, id="ollama/router-exists", tier_eligibility=[1])
    _restrict(tier_service_, 1, ["ollama/router-exists"])

    with pytest.raises(NoEligibleModelError):
        router_.build_execution_plan(
            RoutingRequest(prompt="hi", tier=1, force_model="ollama/router-does-not-exist")
        )


# --- multi-model roles and auto-retrieval -----------------------------------------


def test_tier_five_assigns_primary_refiner_and_critic_roles_when_thinking_enabled(
    router_: Router, model_repo_: ModelRepository, tier_service_: TierService, pin_repo_: RoutingPinRepository
):
    _dynamic(pin_repo_, 5, "general")
    ids = []
    for i in range(3):
        model_id = f"claude/router-t5-{i}"
        _seed_model(
            model_repo_, id=model_id, provider="claude", is_local=False,
            tier_eligibility=[5], capabilities={"general": 0.9 - i * 0.1},
        )
        ids.append(model_id)
    _restrict(tier_service_, 5, ids)

    plan = router_.build_execution_plan(RoutingRequest(prompt="hi", tier=5, task_type="general", thinking=True))

    roles = [s.role for s in plan.selections]
    assert roles == ["primary", "refiner", "critic"]


def test_tier_zero_never_assigns_extra_roles(
    router_: Router, model_repo_: ModelRepository, tier_service_: TierService, pin_repo_: RoutingPinRepository
):
    _dynamic(pin_repo_, 0, "general")
    _seed_model(
        model_repo_, id="ollama/hf.co/ThalisAI/Qwen3-VL-8B-Instruct-heretic:Q8_0", tier_eligibility=[0]
    )

    plan = router_.build_execution_plan(RoutingRequest(prompt="hi", tier=0, task_type="general"))

    assert plan.source == "local-role-policy"
    assert [s.role for s in plan.selections] == ["primary"]


def test_high_tiers_do_not_auto_enable_web_and_memory_without_flag(
    router_: Router, model_repo_: ModelRepository, tier_service_: TierService, pin_repo_: RoutingPinRepository
):
    _dynamic(pin_repo_, 4, "general")
    _seed_model(model_repo_, id="claude/router-t4", provider="claude", is_local=False, tier_eligibility=[4])
    _restrict(tier_service_, 4, ["claude/router-t4"])

    plan = router_.build_execution_plan(RoutingRequest(prompt="hi", tier=4, task_type="general"))

    assert plan.needs_web is False
    assert plan.use_memory is False


def test_auto_retrieval_flag_enables_web_and_memory(
    router_: Router, model_repo_: ModelRepository, tier_service_: TierService, pin_repo_: RoutingPinRepository
):
    _dynamic(pin_repo_, 4, "general")
    _seed_model(model_repo_, id="claude/router-t4-auto", provider="claude", is_local=False, tier_eligibility=[4])
    _restrict(tier_service_, 4, ["claude/router-t4-auto"])

    plan = router_.build_execution_plan(
        RoutingRequest(prompt="hi", tier=4, task_type="general", auto_retrieval=True)
    )

    assert plan.needs_web is True
    assert plan.use_memory is True


def test_high_tier_without_thinking_stays_single_step(
    router_: Router, model_repo_: ModelRepository, tier_service_: TierService, pin_repo_: RoutingPinRepository
):
    _dynamic(pin_repo_, 5, "general")
    ids = []
    for i in range(3):
        model_id = f"claude/router-t5-single-{i}"
        _seed_model(
            model_repo_, id=model_id, provider="claude", is_local=False,
            tier_eligibility=[5], capabilities={"general": 0.9 - i * 0.1},
        )
        ids.append(model_id)
    _restrict(tier_service_, 5, ids)

    plan = router_.build_execution_plan(RoutingRequest(prompt="hi", tier=5, task_type="general"))

    assert [s.role for s in plan.selections] == ["primary"]


def test_low_tiers_respect_explicit_web_request(
    router_: Router, model_repo_: ModelRepository, tier_service_: TierService, pin_repo_: RoutingPinRepository
):
    _dynamic(pin_repo_, 0, "general")
    _seed_model(
        model_repo_, id="ollama/hf.co/ThalisAI/Qwen3-VL-8B-Instruct-heretic:Q8_0", tier_eligibility=[0]
    )

    plan = router_.build_execution_plan(RoutingRequest(prompt="hi", tier=0, needs_web=True))

    assert plan.source == "local-role-policy"
    assert plan.needs_web is True  # explicit request, not tier-driven
    assert plan.use_memory is False

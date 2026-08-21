from __future__ import annotations

from datetime import datetime, timedelta, timezone
from typing import List, Optional

import pytest
from sqlalchemy.orm import Session

from lib.core.settings import Settings
from lib.core.time_utils import ensure_utc
from lib.dal.models import AccessStatus, ModelCatalogEntry
from lib.dal.repositories.model_repository import ModelRepository
from lib.engine.discovery.base import DiscoveredModel, ProviderDiscoveryError
from lib.engine.executor import build_default_drivers
from lib.engine.registry_service import ModelRegistryService, build_default_registry_service


class _FakeDiscovery:
    def __init__(
        self,
        provider: str,
        models: Optional[List[DiscoveredModel]] = None,
        error: Optional[str] = None,
    ) -> None:
        self.provider = provider
        self._models = models or []
        self._error = error

    def discover(self) -> List[DiscoveredModel]:
        if self._error:
            raise ProviderDiscoveryError(self.provider, self._error)
        return self._models


def _seed(
    repo: ModelRepository,
    id: str,
    provider: str,
    display_name: str,
    access_status: str,
    **overrides,
) -> ModelCatalogEntry:
    """Every NOT NULL column set explicitly: `upsert` copies whatever the
    passed-in entry holds, and unset ORM attributes aren't backfilled with
    column defaults until a real INSERT happens.
    """
    defaults = dict(
        status_reason=None,
        parameter_size=None,
        context_window=8192,
        is_local=False,
        tier_eligibility=[],
        capabilities={},
        cost_per_million_tokens=0.0,
        is_enabled=True,
    )
    defaults.update(overrides)
    return repo.upsert(
        ModelCatalogEntry(
            id=id, provider=provider, display_name=display_name, access_status=access_status, **defaults
        )
    )


@pytest.fixture
def registry_repo(db_session: Session) -> ModelRepository:
    return ModelRepository(session_factory=lambda: db_session)


def test_sync_inserts_new_models_with_defaults(registry_repo: ModelRepository):
    discovered = [
        DiscoveredModel(
            id="ollama/reg-insert-dolphin",
            provider="ollama",
            display_name="dolphin3:8b",
            access_status=AccessStatus.AVAILABLE.value,
            status_reason="Local (0 cost)",
            is_local=True,
            tier_eligibility=[0, 1, 2],
        )
    ]
    service = ModelRegistryService(
        discoveries=[_FakeDiscovery("ollama", models=discovered)], repository=registry_repo
    )

    service.sync()

    stored = registry_repo.get_by_id("ollama/reg-insert-dolphin")
    assert stored is not None
    assert stored.access_status == AccessStatus.AVAILABLE.value
    assert stored.is_enabled is True
    assert stored.tier_eligibility == [0, 1, 2]


def test_sync_preserves_manual_curation_on_existing_model(registry_repo: ModelRepository):
    _seed(
        registry_repo,
        "agy/reg-curated-gemini-pro",
        "agy",
        "gemini-3.1-pro-high (old name)",
        AccessStatus.AVAILABLE.value,
        tier_eligibility=[3, 4],  # curated by hand, above the discovery default
        capabilities={"reasoning": 0.9},
        cost_per_million_tokens=12.5,
    )
    discovered = [
        DiscoveredModel(
            id="agy/reg-curated-gemini-pro",
            provider="agy",
            display_name="gemini-3.1-pro-high",
            access_status=AccessStatus.AVAILABLE.value,
            status_reason="OAuth OK",
            tier_eligibility=[1, 2, 3],  # discovery's generic default
        )
    ]
    service = ModelRegistryService(
        discoveries=[_FakeDiscovery("agy", models=discovered)], repository=registry_repo
    )

    service.sync()

    stored = registry_repo.get_by_id("agy/reg-curated-gemini-pro")
    assert stored.display_name == "gemini-3.1-pro-high"  # discovery-observed field updates
    assert stored.tier_eligibility == [3, 4]  # curated field survives
    assert stored.capabilities == {"reasoning": 0.9}  # curated field survives
    assert stored.cost_per_million_tokens == 12.5  # curated field survives


def test_sync_marks_only_the_failing_provider_offline(registry_repo: ModelRepository):
    _seed(
        registry_repo,
        "ollama/reg-offline-dolphin",
        "ollama",
        "dolphin3:8b",
        AccessStatus.AVAILABLE.value,
    )
    _seed(
        registry_repo,
        "claude/reg-offline-sonnet",
        "claude",
        "Claude Sonnet 5",
        AccessStatus.AVAILABLE.value,
    )
    service = ModelRegistryService(
        discoveries=[
            _FakeDiscovery("ollama", error="Ollama unreachable at http://localhost:11434"),
            _FakeDiscovery(
                "claude",
                models=[
                    DiscoveredModel(
                        id="claude/reg-offline-sonnet",
                        provider="claude",
                        display_name="Claude Sonnet 5",
                        access_status=AccessStatus.AVAILABLE.value,
                    )
                ],
            ),
        ],
        repository=registry_repo,
    )

    service.sync()

    offline_model = registry_repo.get_by_id("ollama/reg-offline-dolphin")
    assert offline_model.access_status == AccessStatus.OFFLINE.value
    assert "unreachable" in offline_model.status_reason

    unaffected_model = registry_repo.get_by_id("claude/reg-offline-sonnet")
    assert unaffected_model.access_status == AccessStatus.AVAILABLE.value


def test_sync_never_resurrects_a_manually_disabled_model(registry_repo: ModelRepository):
    _seed(
        registry_repo,
        "codex/reg-disabled-o3",
        "codex",
        "OpenAI o3",
        AccessStatus.DISABLED_MANUALLY.value,
        status_reason="Disabled by user",
        is_enabled=False,
    )
    discovered = [
        DiscoveredModel(
            id="codex/reg-disabled-o3",
            provider="codex",
            display_name="OpenAI o3",
            access_status=AccessStatus.AVAILABLE.value,
        )
    ]
    service = ModelRegistryService(
        discoveries=[_FakeDiscovery("codex", models=discovered)], repository=registry_repo
    )

    service.sync()

    stored = registry_repo.get_by_id("codex/reg-disabled-o3")
    assert stored.access_status == AccessStatus.DISABLED_MANUALLY.value


def test_list_available_for_router_excludes_disabled_and_unavailable(registry_repo: ModelRepository):
    _seed(
        registry_repo,
        "ollama/reg-router-dolphin",
        "ollama",
        "dolphin3:8b",
        AccessStatus.AVAILABLE.value,
        tier_eligibility=[0, 1],
    )
    _seed(
        registry_repo,
        "codex/reg-router-o3",
        "codex",
        "OpenAI o3",
        AccessStatus.REQUIRES_SUBSCRIPTION.value,
        tier_eligibility=[4, 5],
    )
    _seed(
        registry_repo,
        "claude/reg-router-opus",
        "claude",
        "Claude Opus 5",
        AccessStatus.AVAILABLE.value,
        tier_eligibility=[4, 5],
        is_enabled=False,
    )
    service = ModelRegistryService(discoveries=[], repository=registry_repo)

    # Scoped to membership, not full-set equality: the test DB persists across
    # this whole file's tests (see conftest.py), so other tests' rows coexist.
    available_ids = {m.id for m in service.list_available_for_router()}

    assert "ollama/reg-router-dolphin" in available_ids
    assert "codex/reg-router-o3" not in available_ids  # REQUIRES_SUBSCRIPTION
    assert "claude/reg-router-opus" not in available_ids  # is_enabled=False


def test_update_config_disable_then_enable_round_trips_status(registry_repo: ModelRepository):
    _seed(
        registry_repo,
        "ollama/reg-toggle-qwen",
        "ollama",
        "qwen3:8b",
        AccessStatus.AVAILABLE.value,
    )
    service = ModelRegistryService(discoveries=[], repository=registry_repo)

    disabled = service.update_config("ollama/reg-toggle-qwen", is_enabled=False)
    assert disabled.is_enabled is False
    assert disabled.access_status == AccessStatus.DISABLED_MANUALLY.value

    re_enabled = service.update_config("ollama/reg-toggle-qwen", is_enabled=True)
    assert re_enabled.is_enabled is True
    assert re_enabled.access_status == AccessStatus.OFFLINE.value  # pending next sync


def test_sync_does_not_resurrect_an_active_cooldown(registry_repo: ModelRepository):
    future = datetime.now(timezone.utc) + timedelta(minutes=10)
    _seed(
        registry_repo,
        "codex/reg-cooldown-active-o3",
        "codex",
        "OpenAI o3",
        AccessStatus.COOLING_DOWN.value,
        status_reason="Rate limited (429)",
        cooldown_until=future,
    )
    discovered = [
        DiscoveredModel(
            id="codex/reg-cooldown-active-o3",
            provider="codex",
            display_name="OpenAI o3",
            access_status=AccessStatus.AVAILABLE.value,
        )
    ]
    service = ModelRegistryService(
        discoveries=[_FakeDiscovery("codex", models=discovered)], repository=registry_repo
    )

    service.sync()

    stored = registry_repo.get_by_id("codex/reg-cooldown-active-o3")
    assert stored.access_status == AccessStatus.COOLING_DOWN.value
    assert ensure_utc(stored.cooldown_until) == future


def test_sync_clears_an_expired_cooldown(registry_repo: ModelRepository):
    past = datetime.now(timezone.utc) - timedelta(minutes=1)
    _seed(
        registry_repo,
        "codex/reg-cooldown-expired-o3",
        "codex",
        "OpenAI o3",
        AccessStatus.COOLING_DOWN.value,
        status_reason="Rate limited (429)",
        cooldown_until=past,
    )
    discovered = [
        DiscoveredModel(
            id="codex/reg-cooldown-expired-o3",
            provider="codex",
            display_name="OpenAI o3",
            access_status=AccessStatus.AVAILABLE.value,
        )
    ]
    service = ModelRegistryService(
        discoveries=[_FakeDiscovery("codex", models=discovered)], repository=registry_repo
    )

    service.sync()

    stored = registry_repo.get_by_id("codex/reg-cooldown-expired-o3")
    assert stored.access_status == AccessStatus.AVAILABLE.value
    assert stored.cooldown_until is None


def test_sync_scoped_to_providers_only_probes_those(registry_repo: ModelRepository):
    codex_discovery = _FakeDiscovery(
        "codex",
        models=[
            DiscoveredModel(
                id="codex/reg-scoped-o3", provider="codex", display_name="OpenAI o3",
                access_status=AccessStatus.AVAILABLE.value,
            )
        ],
    )
    claude_discovery = _FakeDiscovery(
        "claude",
        models=[
            DiscoveredModel(
                id="claude/reg-scoped-sonnet", provider="claude", display_name="Claude Sonnet 5",
                access_status=AccessStatus.AVAILABLE.value,
            )
        ],
    )
    service = ModelRegistryService(discoveries=[codex_discovery, claude_discovery], repository=registry_repo)

    service.sync(providers={"codex"})

    assert registry_repo.get_by_id("codex/reg-scoped-o3") is not None
    assert registry_repo.get_by_id("claude/reg-scoped-sonnet") is None  # claude was never probed


def test_sync_without_providers_filter_probes_everyone(registry_repo: ModelRepository):
    discovery = _FakeDiscovery(
        "codex",
        models=[
            DiscoveredModel(
                id="codex/reg-unscoped-o3", provider="codex", display_name="OpenAI o3",
                access_status=AccessStatus.AVAILABLE.value,
            )
        ],
    )
    service = ModelRegistryService(discoveries=[discovery], repository=registry_repo)

    service.sync()  # providers=None: unrestricted, matches pre-#13 behavior

    assert registry_repo.get_by_id("codex/reg-unscoped-o3") is not None


def test_build_default_registry_service_skips_disabled_standard_providers():
    settings = Settings(disabled_providers=["claude", "codex"])

    service = build_default_registry_service(settings=settings)
    providers = {discovery.provider for discovery in service._discoveries}

    assert "claude" not in providers
    assert "codex" not in providers
    assert "agy" in providers


def test_build_default_registry_service_can_ignore_disabled_providers_for_calibration():
    settings = Settings(disabled_providers=["claude", "codex"])

    service = build_default_registry_service(settings=settings, respect_disabled_providers=False)
    providers = {discovery.provider for discovery in service._discoveries}

    assert "claude" in providers
    assert "codex" in providers
    assert "agy" in providers


def test_build_default_drivers_skips_disabled_providers():
    settings = Settings(disabled_providers=["claude"])

    drivers = build_default_drivers(settings=settings)

    assert "claude" not in drivers
    assert "codex" in drivers

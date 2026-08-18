from __future__ import annotations

from typing import List, Optional

import pytest
from sqlalchemy.orm import Session

from lib.dal.models import AccessStatus, ModelCatalogEntry
from lib.dal.repositories.model_repository import ModelRepository
from lib.engine.discovery.base import DiscoveredModel, ProviderDiscoveryError
from lib.engine.registry_service import ModelRegistryService


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

from __future__ import annotations

from datetime import datetime, timezone
from pathlib import Path
from typing import Iterable, List, Optional

from sqlalchemy.orm import Session

from lib.core.settings import Settings, get_settings
from lib.core.time_utils import ensure_utc
from lib.dal.local.database import session_scope
from lib.dal.models import AccessStatus, ModelCatalogEntry
from lib.dal.repositories.model_repository import ModelRepository
from lib.engine.discovery.antigravity import AntigravityDiscovery
from lib.engine.discovery.base import DiscoveredModel, ProviderDiscovery, ProviderDiscoveryError
from lib.engine.discovery.claude_docker import ClaudeDockerDiscovery
from lib.engine.discovery.codex import CodexDiscovery
from lib.engine.discovery.ollama import OllamaDiscovery


class ModelRegistryService:
    """The Model Registry: cached reads plus provider-driven sync.

    Discovery adapters only observe what a provider reports right now; this
    service is what reconciles that with the persisted catalog:
    - manually curated fields (`tier_eligibility`, `capabilities`, `cost`,
      `is_enabled`) survive a sync untouched;
    - `DISABLED_MANUALLY` models are left alone until the user re-enables them;
    - a `COOLING_DOWN` model (set by the Quota Tracker on a 429) stays that way
      until its `cooldown_until` elapses, even if the provider answers again
      in the meantime;
    - a provider that can't be probed at all only affects *its own* cached
      models (marked `OFFLINE`), never the whole catalog.
    """

    def __init__(
        self,
        discoveries: Iterable[ProviderDiscovery],
        repository: Optional[ModelRepository] = None,
    ) -> None:
        self._discoveries = list(discoveries)
        self._repository = repository or ModelRepository()

    # -- cached reads (sub-millisecond, straight from SQLite) -----------------

    def list_models(
        self,
        provider: Optional[str] = None,
        tier: Optional[int] = None,
        access_status: Optional[str] = None,
        is_enabled: Optional[bool] = None,
    ) -> List[ModelCatalogEntry]:
        return self._repository.list_models(
            provider=provider, tier=tier, access_status=access_status, is_enabled=is_enabled
        )

    def get_model(self, model_id: str) -> Optional[ModelCatalogEntry]:
        return self._repository.get_by_id(model_id)

    def list_available_for_router(self, tier: Optional[int] = None) -> List[ModelCatalogEntry]:
        """The only view the Router should ever read from: enabled and AVAILABLE."""
        return self._repository.list_models(
            tier=tier, access_status=AccessStatus.AVAILABLE.value, is_enabled=True
        )

    # -- manual configuration --------------------------------------------------

    def update_config(
        self,
        model_id: str,
        tier_eligibility: Optional[List[int]] = None,
        is_enabled: Optional[bool] = None,
    ) -> Optional[ModelCatalogEntry]:
        updated = self._repository.update_config(
            model_id, tier_eligibility=tier_eligibility, is_enabled=is_enabled
        )
        if updated is None:
            return None
        if is_enabled is False:
            self._repository.update_status(
                model_id, AccessStatus.DISABLED_MANUALLY, reason="Disabled by user"
            )
        elif is_enabled is True and updated.access_status == AccessStatus.DISABLED_MANUALLY.value:
            self._repository.update_status(
                model_id, AccessStatus.OFFLINE, reason="Re-enabled; pending next sync"
            )
        return self._repository.get_by_id(model_id)

    # -- live discovery / sync --------------------------------------------------

    def sync(self, providers: Optional[Iterable[str]] = None) -> List[ModelCatalogEntry]:
        """Probes every provider and merges the results into the cache.

        Each discovery adapter is isolated: one provider failing to respond
        doesn't stop the others from updating. Pass `providers` to scope the
        live probe to just those (e.g. the ones a cooldown just cleared for)
        instead of hitting every provider on every call.
        """
        wanted = set(providers) if providers is not None else None
        with session_scope() as session:
            for discovery in self._discoveries:
                if wanted is not None and discovery.provider not in wanted:
                    continue
                try:
                    discovered = discovery.discover()
                except ProviderDiscoveryError as exc:
                    self._mark_provider_offline(discovery.provider, str(exc), session)
                    continue
                for model in discovered:
                    self._merge(model, session)
            return self._repository.list_models(session=session)

    def _mark_provider_offline(self, provider: str, reason: str, session: Session) -> None:
        for existing in self._repository.list_models(provider=provider, session=session):
            if existing.access_status == AccessStatus.DISABLED_MANUALLY.value:
                continue
            self._repository.update_status(
                existing.id, AccessStatus.OFFLINE, reason=reason, session=session
            )

    def _merge(self, discovered: DiscoveredModel, session: Session) -> ModelCatalogEntry:
        existing = self._repository.get_by_id(discovered.id, session=session)
        if existing is not None and existing.access_status == AccessStatus.DISABLED_MANUALLY.value:
            return existing  # manual disable wins over live discovery
        if existing is not None and self._still_cooling_down(existing):
            return existing  # a 429 cooldown outlives a provider simply being reachable again

        entry = ModelCatalogEntry(
            id=discovered.id,
            provider=discovered.provider,
            display_name=discovered.display_name,
            access_status=discovered.access_status,
            status_reason=discovered.status_reason,
            parameter_size=discovered.parameter_size,
            context_window=discovered.context_window,
            is_local=discovered.is_local,
            tier_eligibility=existing.tier_eligibility if existing else discovered.tier_eligibility,
            capabilities=existing.capabilities if existing else discovered.capabilities,
            cost_per_million_tokens=(
                existing.cost_per_million_tokens if existing else discovered.cost_per_million_tokens
            ),
            is_enabled=existing.is_enabled if existing else True,
        )
        return self._repository.upsert(entry, session=session)

    @staticmethod
    def _still_cooling_down(entry: ModelCatalogEntry) -> bool:
        if entry.access_status != AccessStatus.COOLING_DOWN.value or entry.cooldown_until is None:
            return False
        return ensure_utc(entry.cooldown_until) > datetime.now(timezone.utc)


def build_default_registry_service(settings: Optional[Settings] = None) -> ModelRegistryService:
    """Wires the registry service with the real provider discovery adapters.

    The presentation layer (CLI/API, issues #10 and #12) should construct the
    service through this, not by hand-assembling discovery adapters itself.
    """
    settings = settings or get_settings()
    return ModelRegistryService(
        discoveries=[
            OllamaDiscovery(
                base_url=settings.ollama_base_url, timeout=settings.discovery_timeout_seconds
            ),
            AntigravityDiscovery(
                command=settings.agy_command, timeout=settings.discovery_timeout_seconds
            ),
            ClaudeDockerDiscovery(credentials_path=Path(settings.claude_credentials_path)),
            CodexDiscovery(auth_path=Path(settings.codex_auth_path)),
        ]
    )

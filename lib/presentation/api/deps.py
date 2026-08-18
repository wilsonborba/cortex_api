from __future__ import annotations

from functools import lru_cache

from lib.dal.repositories.pin_repository import RoutingPinRepository
from lib.dal.repositories.telemetry_repository import TelemetryRepository
from lib.engine.executor import Executor, build_default_executor
from lib.engine.quota import QuotaTracker
from lib.engine.registry_service import ModelRegistryService, build_default_registry_service
from lib.engine.router import Router, build_default_router
from lib.engine.tiers import TierService

# One instance per process, same pattern as lib.core.settings.get_settings.
# Overridable per-test via FastAPI's `app.dependency_overrides[get_x] = ...`.


@lru_cache(maxsize=1)
def get_registry() -> ModelRegistryService:
    return build_default_registry_service()


@lru_cache(maxsize=1)
def get_quota_tracker() -> QuotaTracker:
    return QuotaTracker()


@lru_cache(maxsize=1)
def get_tier_service() -> TierService:
    return TierService()


@lru_cache(maxsize=1)
def get_pin_repo() -> RoutingPinRepository:
    return RoutingPinRepository()


@lru_cache(maxsize=1)
def get_telemetry_repo() -> TelemetryRepository:
    return TelemetryRepository()


@lru_cache(maxsize=1)
def get_router() -> Router:
    return build_default_router()


@lru_cache(maxsize=1)
def get_executor() -> Executor:
    return build_default_executor()

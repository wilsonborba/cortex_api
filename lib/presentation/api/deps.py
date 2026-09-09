from __future__ import annotations

from functools import lru_cache

from lib.dal.repositories.conversation_repository import ConversationRepository
from lib.dal.repositories.pin_repository import RoutingPinRepository
from lib.dal.repositories.telemetry_repository import TelemetryRepository
from lib.engine.executor import Executor, build_default_executor
from lib.engine.prompt_normalizer import PromptNormalizer
from lib.engine.quota import QuotaTracker
from lib.engine.registry_service import ModelRegistryService, build_default_registry_service
from lib.engine.router import Router, build_default_router
from lib.engine.tiers import TierService
from lib.engine.video_ingest import VideoIngestor
from lib.engine.video_jobs import VideoJobStore, get_default_video_job_store
from lib.core.security import SecurityShield
from lib.core.settings import get_settings

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
def get_conversation_repo() -> ConversationRepository:
    return ConversationRepository()


@lru_cache(maxsize=1)
def get_telemetry_repo() -> TelemetryRepository:
    return TelemetryRepository()


@lru_cache(maxsize=1)
def get_router() -> Router:
    return build_default_router()


@lru_cache(maxsize=1)
def get_executor() -> Executor:
    return build_default_executor()


@lru_cache(maxsize=1)
def get_prompt_normalizer() -> PromptNormalizer:
    executor = get_executor()
    return PromptNormalizer(registry=get_registry(), drivers=executor.drivers)


@lru_cache(maxsize=1)
def get_security_shield() -> SecurityShield:
    settings = get_settings()
    executor = get_executor()
    return SecurityShield(
        registry=get_registry(),
        drivers=executor.drivers,
        enabled=settings.security_shield_enabled,
    )


@lru_cache(maxsize=1)
def get_video_job_store() -> VideoJobStore:
    return get_default_video_job_store()


def _summarize_via_router(text: str) -> str:
    """Fuses a video's transcript + frame captions into prose, via the
    normal Router/Executor pipeline (T1: cheap, no multi-step needed) --
    not a bespoke summarization component."""
    import asyncio

    from lib.engine.router import RoutingRequest

    router = get_router()
    executor = get_executor()
    prompt = (
        "Summarize what happens in this video, in a few sentences, using "
        "only the information below. Do not invent details.\n\n" + text
    )
    plan = router.build_execution_plan(RoutingRequest(prompt=prompt, tier=1, task_type="video_summary"))
    result = asyncio.run(executor.execute(plan))
    return result.response_text


@lru_cache(maxsize=1)
def get_video_ingestor() -> VideoIngestor:
    return VideoIngestor(text_summarizer=_summarize_via_router)


@lru_cache(maxsize=1)
def get_hippocampus_client():
    from lib.engine.retrieval.hippocampus import build_default_hippocampus_client
    return build_default_hippocampus_client()


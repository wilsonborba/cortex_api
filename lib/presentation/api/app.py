from __future__ import annotations

import asyncio
import contextlib
from contextlib import asynccontextmanager
from typing import AsyncIterator, Optional

from fastapi import FastAPI, Request
from fastapi.middleware.cors import CORSMiddleware
from fastapi.responses import JSONResponse

from lib.core.logs import LogTarget, configure_logging, get_logger
from lib.core.settings import Settings, get_settings
from lib.dal.migrations import upgrade_db
from lib.engine.executor import UnresolvedStrategyError
from lib.engine.quota import QuotaTracker, refresh_and_resync
from lib.engine.registry_service import build_default_registry_service
from lib.engine.router import NoEligibleModelError
from lib.engine.tiers import TierService
from lib.presentation.api.routes import execute, logs_stream, models, openai_facade, pins, quota, telemetry, tiers

logger = get_logger(__name__)


async def _cooldown_refresh_loop(settings: Settings) -> None:
    tracker = QuotaTracker(settings=settings)
    registry = build_default_registry_service(settings=settings)
    while True:
        await asyncio.sleep(settings.api_cooldown_refresh_interval_seconds)
        try:
            # Not just refresh_cooldowns() alone: that only guesses OFFLINE
            # from the clock. refresh_and_resync (#13) live-probes whatever
            # just cleared, so it comes back AVAILABLE only when it's real.
            refresh_and_resync(tracker, registry)
        except Exception:
            logger.warning("cooldown refresh/resync failed", exc_info=True)


def _build_lifespan(settings: Settings):
    @asynccontextmanager
    async def _lifespan(app: FastAPI) -> AsyncIterator[None]:
        configure_logging(target=LogTarget.API, log_file=settings.log_file)

        try:
            upgrade_db(settings.database_url)
        except Exception:
            logger.warning("DB migration check failed on startup", exc_info=True)

        try:
            TierService().ensure_seeded()
        except Exception:
            logger.warning("tier policy seeding failed on startup", exc_info=True)

        if settings.api_sync_models_on_startup:
            try:
                build_default_registry_service(settings=settings).sync()
            except Exception:
                logger.warning("initial Model Registry sync failed", exc_info=True)

        background_task: Optional[asyncio.Task] = None
        if settings.api_background_tasks_enabled:
            background_task = asyncio.create_task(_cooldown_refresh_loop(settings))

        try:
            yield
        finally:
            if background_task is not None:
                background_task.cancel()
                with contextlib.suppress(asyncio.CancelledError):
                    await background_task

    return _lifespan


def create_app(settings: Optional[Settings] = None) -> FastAPI:
    settings = settings or get_settings()
    app = FastAPI(title="Cortex", version="0.1.0", lifespan=_build_lifespan(settings))

    app.add_middleware(
        CORSMiddleware,
        allow_origins=settings.cors_allow_origins,
        allow_credentials=True,
        allow_methods=["*"],
        allow_headers=["*"],
    )

    @app.exception_handler(NoEligibleModelError)
    async def _no_eligible_model(request: Request, exc: NoEligibleModelError) -> JSONResponse:
        return JSONResponse(status_code=409, content={"error": "no_eligible_model", "detail": str(exc)})

    @app.exception_handler(UnresolvedStrategyError)
    async def _unresolved_strategy(request: Request, exc: UnresolvedStrategyError) -> JSONResponse:
        return JSONResponse(status_code=501, content={"error": "unresolved_strategy", "detail": str(exc)})

    @app.exception_handler(Exception)
    async def _unhandled(request: Request, exc: Exception) -> JSONResponse:
        logger.error("unhandled error on %s %s", request.method, request.url.path, exc_info=True)
        return JSONResponse(status_code=500, content={"error": "internal_error", "detail": str(exc)})

    app.include_router(execute.router)
    app.include_router(models.router)
    app.include_router(quota.router)
    app.include_router(tiers.router)
    app.include_router(pins.router)
    app.include_router(telemetry.router)
    app.include_router(logs_stream.router)
    app.include_router(openai_facade.router)

    return app

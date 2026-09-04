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
from lib.presentation.api.openapi_i18n import get_localized_openapi
from lib.presentation.api.routes import execute, logs_stream, models, openai_facade, pins, quota, system, telemetry, tiers, video

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
        finally:
            # alembic's env.py calls logging.config.fileConfig() on every run
            # (from alembic.ini's [loggers]/[handlers] sections), which resets
            # the ROOT logger's handlers wholesale — silently detaching the
            # rotating file handler configure_logging() just attached above.
            # Every log line after this point (including the /logs/stream
            # websocket's entire reason to exist) would otherwise go to
            # alembic's bare console handler only, forever, for the life of
            # the process. Re-attach ours now that alembic is done.
            configure_logging(target=LogTarget.API, log_file=settings.log_file)

        try:
            TierService().ensure_seeded()
        except Exception:
            logger.warning("tier policy seeding failed on startup", exc_info=True)

        if settings.api_sync_models_on_startup:
            async def _async_sync() -> None:
                try:
                    await asyncio.to_thread(build_default_registry_service(settings=settings).sync)
                except Exception:
                    logger.warning("initial Model Registry sync failed", exc_info=True)

            asyncio.create_task(_async_sync())

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


from fastapi.responses import HTMLResponse, JSONResponse

OPENAPI_TAGS_METADATA = [
    {"name": "execute", "description": "Dynamic multi-tier execution engine across providers and pipelines."},
    {"name": "models", "description": "Model registry, live provider discovery, status inspection, and configuration."},
    {"name": "openai-facade", "description": "OpenAI-compatible /v1/chat/completions and /v1/models protocol facade."},
    {"name": "tiers", "description": "Tier envelopes (T0 to T5) policy configuration and latency bounds."},
    {"name": "pins", "description": "Routing pins for pinning specific models or strategies to tiers/tasks."},
    {"name": "quota", "description": "Sliding-window token quota usage, budget tracking, and cooldown status."},
    {"name": "telemetry", "description": "Execution telemetry stats, aggregated metrics, and event audit trail."},
    {"name": "logs", "description": "Real-time WebSocket streaming of live system logs."},
    {"name": "video", "description": "Async video ingest, multi-modal frame extraction, and transcription jobs."},
    {"name": "system", "description": "System runtime profile, hardware capabilities introspection, and feature status."},
]


def create_app(settings: Optional[Settings] = None) -> FastAPI:
    settings = settings or get_settings()
    app = FastAPI(
        title="Cortex Multi-Model AI Orchestrator API",
        version="0.1.0",
        description="Unified dynamic orchestration, quota tracking, evidence-based routing, and OpenAI-compatible facade across 14+ AI providers.",
        openapi_tags=OPENAPI_TAGS_METADATA,
        openapi_url=None,
        docs_url=None,
        redoc_url=None,
        lifespan=_build_lifespan(settings),
    )
    app.state.settings = settings

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

    @app.get("/openapi.json", include_in_schema=False)
    async def openapi_endpoint(lang: str = "en") -> JSONResponse:
        return JSONResponse(get_localized_openapi(app, lang=lang))

    @app.get("/openapi-en.json", include_in_schema=False)
    async def openapi_en() -> JSONResponse:
        return JSONResponse(get_localized_openapi(app, lang="en"))

    @app.get("/openapi-pt.json", include_in_schema=False)
    async def openapi_pt() -> JSONResponse:
        return JSONResponse(get_localized_openapi(app, lang="pt"))

    @app.get("/openapi-th.json", include_in_schema=False)
    async def openapi_th() -> JSONResponse:
        return JSONResponse(get_localized_openapi(app, lang="th"))

    @app.get("/docs", response_class=HTMLResponse, include_in_schema=False)
    @app.get("/scalar", response_class=HTMLResponse, include_in_schema=False)
    @app.get("/docs/scalar", response_class=HTMLResponse, include_in_schema=False)
    async def scalar_html(lang: Optional[str] = "en"):
        cur_lang = (lang or "en").lower()
        if cur_lang not in ("en", "pt", "th"):
            cur_lang = "en"

        openapi_url = f"/openapi.json?lang={cur_lang}"

        en_active = "active" if cur_lang == "en" else ""
        pt_active = "active" if cur_lang == "pt" else ""
        th_active = "active" if cur_lang == "th" else ""

        html = f"""<!doctype html>
<html lang="{cur_lang}">
  <head>
    <title>Cortex API Documentation - Scalar</title>
    <meta charset="utf-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1" />
    <style>
      body {{ margin: 0; font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif; }}
      .cortex-nav {{
        background: #181528;
        color: #fff;
        display: flex;
        align-items: center;
        justify-content: space-between;
        padding: 10px 24px;
        font-size: 14px;
        border-bottom: 1px solid #2d264a;
        box-shadow: 0 2px 8px rgba(0, 0, 0, 0.3);
      }}
      .cortex-brand {{
        display: flex;
        align-items: center;
        gap: 10px;
        font-weight: 600;
        letter-spacing: 0.3px;
      }}
      .cortex-badge {{
        background: #7c3aed;
        color: #fff;
        font-size: 11px;
        padding: 2px 8px;
        border-radius: 12px;
        font-weight: bold;
      }}
      .cortex-links {{
        display: flex;
        align-items: center;
        gap: 16px;
      }}
      .cortex-links a {{
        color: #c4b5fd;
        text-decoration: none;
        font-weight: 500;
        transition: color 0.15s ease;
      }}
      .cortex-links a:hover {{
        color: #fff;
        text-decoration: underline;
      }}
      .lang-switcher {{
        display: inline-flex;
        align-items: center;
        background: #251f3d;
        border: 1px solid #433870;
        border-radius: 8px;
        padding: 3px;
        gap: 4px;
      }}
      .lang-btn {{
        background: transparent;
        color: #bfa5ff;
        border: 1px solid transparent;
        border-radius: 6px;
        padding: 4px 10px;
        font-size: 13px;
        font-weight: 600;
        cursor: pointer;
        text-decoration: none;
        display: inline-flex;
        align-items: center;
        gap: 6px;
        transition: all 0.15s ease;
      }}
      .lang-btn:hover {{
        color: #fff;
        background: rgba(124, 58, 237, 0.2);
      }}
      .lang-btn.active {{
        background: #7c3aed;
        color: #ffffff;
        border-color: #9333ea;
        box-shadow: 0 0 10px rgba(124, 58, 237, 0.4);
      }}
    </style>
  </head>
  <body>
    <div class="cortex-nav">
      <div class="cortex-brand">
        <span>🧠 Cortex Multi-Model AI Orchestrator</span>
        <span class="cortex-badge">v0.1.0</span>
      </div>
      <div class="cortex-links">
        <a href="/openapi.json?lang={cur_lang}" target="_blank">📋 OpenAPI JSON</a>
        <a href="/system/capabilities" target="_blank">⚡ Capabilities</a>
        <div class="lang-switcher">
          <a href="?lang=en" class="lang-btn {en_active}">🇬🇧 English</a>
          <a href="?lang=pt" class="lang-btn {pt_active}">🇧🇷 Português</a>
          <a href="?lang=th" class="lang-btn {th_active}">🇹🇭 ไทย</a>
        </div>
      </div>
    </div>
    <script
      id="api-reference"
      data-url="{openapi_url}"
      data-configuration='{{"theme": "purple", "layout": "modern", "showSidebar": true}}'>
    </script>
    <script src="https://cdn.jsdelivr.net/npm/@scalar/api-reference"></script>
  </body>
</html>"""
        return HTMLResponse(html)

    app.include_router(execute.router)
    app.include_router(models.router)
    app.include_router(quota.router)
    app.include_router(tiers.router)
    app.include_router(pins.router)
    app.include_router(telemetry.router)
    app.include_router(logs_stream.router)
    app.include_router(openai_facade.router)
    app.include_router(video.router)
    app.include_router(system.router)

    return app


app = create_app()

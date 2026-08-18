from __future__ import annotations

import asyncio
from importlib.metadata import PackageNotFoundError, version
from typing import Optional

import typer

from lib.core.logs import LogTarget, configure_logging, get_logger
from lib.core.settings import get_settings
from lib.engine.executor import UnresolvedStrategyError
from lib.engine.router import NoEligibleModelError, RoutingRequest
from lib.presentation.api.deps import get_executor, get_pin_repo, get_quota_tracker, get_registry, get_router, get_tier_service
from lib.presentation.api.schemas.execute import ExecuteResponse
from lib.presentation.api.schemas.quota import QuotaOut
from lib.presentation.cli import models_cmd, pins_cmd, telemetry_cmd, tiers_cmd
from lib.presentation.cli.output import CliState, console, emit, error_exit, print_table

logger = get_logger(__name__)

app = typer.Typer(
    name="cortex",
    help="Multi-model AI orchestration: dynamic effort tiers, quota tracking, evidence-based routing.",
    no_args_is_help=True,
)
app.add_typer(models_cmd.app, name="models")
app.add_typer(tiers_cmd.app, name="tiers")
app.add_typer(pins_cmd.app, name="pin")
app.add_typer(telemetry_cmd.app, name="telemetry")


def _version() -> str:
    try:
        return version("cortex")
    except PackageNotFoundError:
        return "0.1.0"


def _version_callback(show: bool) -> None:
    if show:
        console.print(f"cortex {_version()}")
        raise typer.Exit()


@app.callback()
def main(
    ctx: typer.Context,
    verbose: bool = typer.Option(False, "--verbose", "-v", help="Show debug-level logs"),
    json_output: bool = typer.Option(False, "--json", help="Force JSON output"),
    version_: bool = typer.Option(
        False, "--version", callback=_version_callback, is_eager=True, help="Show the version and exit"
    ),
) -> None:
    ctx.obj = CliState(verbose=verbose, json_output=json_output)
    settings = get_settings()
    configure_logging(target=LogTarget.CLI, verbose=verbose, log_file=settings.log_file)


# -- run --------------------------------------------------------------------------


@app.command()
def run(
    ctx: typer.Context,
    prompt: str = typer.Argument(..., help="The prompt to execute (supports leading /t3 or [T4] directives)"),
    tier: Optional[str] = typer.Option(None, "--tier", help="0-5 or 'auto' (default: auto)"),
    task: str = typer.Option("general", "--task"),
    web: bool = typer.Option(False, "--web", help="Enable live web search context"),
    memory: bool = typer.Option(False, "--memory", help="Enable hippocampus memory context"),
    memory_topic: Optional[str] = typer.Option(None, "--memory-topic"),
    force_model: Optional[str] = typer.Option(None, "--force-model"),
    force_provider: Optional[str] = typer.Option(None, "--force-provider"),
    force_strategy: Optional[str] = typer.Option(None, "--force-strategy"),
) -> None:
    routing_request = RoutingRequest(
        prompt=prompt, tier=tier, task_type=task, needs_web=web, use_memory=memory, memory_topic=memory_topic,
        force_model=force_model, force_provider=force_provider, force_strategy=force_strategy,
    )
    try:
        plan = get_router().build_execution_plan(routing_request)
        result = asyncio.run(get_executor().execute(plan))
    except NoEligibleModelError as exc:
        error_exit(str(exc))
        return
    except UnresolvedStrategyError as exc:
        error_exit(str(exc))
        return

    response = ExecuteResponse.from_result(result)

    def _render() -> None:
        console.print(response.response)
        print_table(
            "Execution",
            ["TIER", "STRATEGY", "SUCCESS", "TOKENS", "COST ($)", "LATENCY (ms)", "ERROR"],
            [[
                f"{response.tier_requested} -> {response.tier_executed}", response.strategy_id, response.success,
                response.total_tokens, round(response.cost_usd, 5), response.latency_ms, response.error_type,
            ]],
        )

    emit(ctx, json_data=response.model_dump(), table=_render)
    if not response.success:
        raise typer.Exit(code=1)


# -- quota ------------------------------------------------------------------------


@app.command()
def quota(ctx: typer.Context, provider: Optional[str] = typer.Option(None, "--provider")) -> None:
    tracker = get_quota_tracker()

    def _row(r: QuotaOut) -> list:
        return [r.provider, r.window_hours, r.window_limit_tokens, r.consumed_tokens, r.remaining_tokens,
                round(r.quota_factor, 3), r.requests_count]

    columns = ["PROVIDER", "WINDOW (h)", "LIMIT", "CONSUMED", "REMAINING", "FACTOR", "REQUESTS"]

    if provider:
        # Mirrors GET /quota/{provider}: a single object, not a one-item list.
        single = QuotaOut.from_provider_quota(tracker.get_quota(provider))
        emit(ctx, json_data=single.model_dump(), table=lambda: print_table("Quota", columns, [_row(single)]))
        return

    rows = [QuotaOut.from_provider_quota(q) for q in tracker.get_summary()]
    emit(ctx, json_data=[r.model_dump() for r in rows], table=lambda: print_table("Quota", columns, [_row(r) for r in rows]))


# -- stream -------------------------------------------------------------------------


@app.command()
def stream(ctx: typer.Context) -> None:
    """Connects to the running API's `WS /logs/stream` and prints lines live.

    Requires `cortex-api` to already be running: this is a thin client over
    the same websocket issue #2 built, not a second reader of the log file.
    """
    import websockets

    settings = get_settings()
    url = f"ws://{settings.api_host}:{settings.api_port}/logs/stream"

    async def _run() -> None:
        async with websockets.connect(url) as ws:
            async for message in ws:
                console.print(message)

    try:
        asyncio.run(_run())
    except KeyboardInterrupt:
        pass
    except OSError as exc:
        error_exit(f"could not connect to {url} (is `cortex-api` running?): {exc}")


# -- db ---------------------------------------------------------------------------


db_app = typer.Typer(help="Database maintenance.")
app.add_typer(db_app, name="db")


@db_app.command("upgrade")
def db_upgrade(ctx: typer.Context) -> None:
    from lib.dal.migrations import upgrade_db

    upgrade_db()
    console.print("Database upgraded to head.")

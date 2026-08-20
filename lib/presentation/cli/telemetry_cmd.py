from __future__ import annotations

from typing import Optional

import typer

from lib.presentation.api.deps import get_telemetry_repo
from lib.presentation.api.schemas.telemetry import TelemetryEventOut, TelemetryStatsOut
from lib.presentation.cli.output import emit, print_table

app = typer.Typer(help="Telemetry: aggregate stats and execution history (issue #9).")


@app.command("stats")
def telemetry_stats(
    ctx: typer.Context,
    tier: Optional[int] = typer.Option(None, "--tier"),
    task: Optional[str] = typer.Option(None, "--task"),
    strategy: Optional[str] = typer.Option(None, "--strategy"),
) -> None:
    rows = get_telemetry_repo().get_stats(tier=tier, task_type=task, strategy_id=strategy)
    stats = [TelemetryStatsOut.from_row(r) for r in rows]
    emit(
        ctx,
        json_data=[s.model_dump() for s in stats],
        table=lambda: print_table(
            "Telemetry Stats",
            ["STRATEGY", "TASK", "TIER", "RUNS", "AVG LATENCY (s)", "AVG TOKENS", "AVG COST ($)", "SUCCESS %"],
            [
                [s.strategy_id, s.task_type, s.tier_requested, s.total_runs, s.avg_latency_seconds,
                 s.avg_total_tokens, s.avg_cost_usd, s.success_rate]
                for s in stats
            ],
        ),
    )


@app.command("history")
def telemetry_history(ctx: typer.Context, limit: int = typer.Option(20, "--limit")) -> None:
    events = [TelemetryEventOut.from_event(e) for e in get_telemetry_repo().list_events(limit=limit)]
    emit(
        ctx,
        json_data=[e.model_dump() for e in events],
        table=lambda: print_table(
            "Telemetry History",
            ["STARTED", "PROVIDER", "MODEL", "ROLE", "TOKENS", "LATENCY (ms)", "SUCCESS", "ERROR"],
            [
                [e.started_at, e.provider, e.model, e.role, e.total_tokens, e.latency_ms, e.success, e.error_type]
                for e in events
            ],
        ),
    )

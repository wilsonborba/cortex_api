from __future__ import annotations

from typing import List, Optional

import typer

from lib.presentation.api.deps import get_registry
from lib.presentation.api.schemas.models import ModelOut
from lib.presentation.cli.output import emit, error_exit, print_table

app = typer.Typer(help="Model Registry: list, inspect, and configure models (issue #4).")


def _table(models: List[ModelOut]) -> None:
    print_table(
        "Models",
        ["ID", "PROVIDER", "SOURCE", "TIERS", "STATUS", "REASON", "ENABLED", "CTX FORMAT", "PIN", "PIN EXPIRES"],
        [
            [
                m.id, m.provider, m.source_kind, m.tier_eligibility, m.access_status, m.status_reason, m.is_enabled,
                m.context_format_computed, m.context_format_pin, m.context_format_pin_expires_at,
            ]
            for m in models
        ],
    )


@app.command("list")
def models_list(
    ctx: typer.Context,
    provider: Optional[str] = typer.Option(None, "--provider"),
    tier: Optional[int] = typer.Option(None, "--tier"),
    status: Optional[str] = typer.Option(None, "--status"),
    update: bool = typer.Option(False, "--update", help="Live-probe every provider before listing"),
) -> None:
    registry = get_registry()
    entries = registry.sync() if update else registry.list_models(provider=provider, tier=tier, access_status=status)
    models = [ModelOut.from_entry(m) for m in entries]
    emit(ctx, json_data=[m.model_dump() for m in models], table=lambda: _table(models))


@app.command("get")
def models_get(ctx: typer.Context, model_id: str) -> None:
    entry = get_registry().get_model(model_id)
    if entry is None:
        error_exit(f"model {model_id!r} not found")
    model = ModelOut.from_entry(entry)
    emit(ctx, json_data=model.model_dump(), table=lambda: _table([model]))


@app.command("config")
def models_config(
    ctx: typer.Context,
    model_id: str,
    tiers: Optional[str] = typer.Option(None, "--tiers", help="comma-separated tier ints, e.g. 0,1,2"),
    enable: Optional[bool] = typer.Option(None, "--enable/--disable"),
    context_format: Optional[str] = typer.Option(
        None, "--context-format",
        help="Pin internal communication format: 'toon', 'json', or 'none' to clear the pin (issue #17)",
    ),
    context_format_ttl: Optional[int] = typer.Option(
        None, "--context-format-ttl", help="Pin expiry in seconds from now; omit for a pin with no expiry"
    ),
) -> None:
    tier_eligibility = [int(t) for t in tiers.split(",")] if tiers else None
    try:
        updated = get_registry().update_config(
            model_id, tier_eligibility=tier_eligibility, is_enabled=enable,
            context_format_pin=context_format, context_format_pin_ttl_seconds=context_format_ttl,
        )
    except ValueError as exc:
        error_exit(str(exc))
        return
    if updated is None:
        error_exit(f"model {model_id!r} not found")
    model = ModelOut.from_entry(updated)
    emit(ctx, json_data=model.model_dump(), table=lambda: _table([model]))


@app.command("sync")
def models_sync(ctx: typer.Context) -> None:
    entries = get_registry().sync()
    models = [ModelOut.from_entry(m) for m in entries]
    emit(ctx, json_data=[m.model_dump() for m in models], table=lambda: _table(models))

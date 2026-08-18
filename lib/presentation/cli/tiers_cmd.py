from __future__ import annotations

from typing import List, Optional

import typer

from lib.presentation.api.deps import get_tier_service
from lib.presentation.api.schemas.tiers import TierOut
from lib.presentation.cli.output import emit, error_exit, print_table

app = typer.Typer(help="Tier envelopes: budget/policy per tier T0-T5 (issue #8).")


def _table(tiers: List[TierOut]) -> None:
    print_table(
        "Tiers",
        ["TIER", "NAME", "MAX LATENCY (s)", "MULTI-MODEL", "EXTERNAL", "RETRIEVAL", "VERIFY", "MAX CALLS", "ALLOWED MODELS"],
        [
            [t.tier, t.name, t.max_latency_seconds, t.allow_multi_model, t.allow_external,
             t.retrieval_mode, t.require_verification, t.max_model_calls, t.allowed_models]
            for t in tiers
        ],
    )


@app.command("list")
def tiers_list(ctx: typer.Context) -> None:
    tiers = [TierOut.from_policy(p) for p in get_tier_service().list_envelopes()]
    emit(ctx, json_data=[t.model_dump() for t in tiers], table=lambda: _table(tiers))


@app.command("config")
def tiers_config(
    ctx: typer.Context,
    tier: int,
    max_latency: Optional[int] = typer.Option(None, "--max-latency"),
    allow_external: Optional[bool] = typer.Option(None, "--allow-external/--no-allow-external"),
    allow_multi_model: Optional[bool] = typer.Option(None, "--allow-multi-model/--no-allow-multi-model"),
    retrieval_mode: Optional[str] = typer.Option(None, "--retrieval-mode"),
    require_verification: Optional[bool] = typer.Option(None, "--require-verification/--no-require-verification"),
    max_model_calls: Optional[int] = typer.Option(None, "--max-model-calls"),
) -> None:
    service = get_tier_service()
    updated = service.configure(
        tier, max_latency_seconds=max_latency, allow_multi_model=allow_multi_model,
        allow_external=allow_external, retrieval_mode=retrieval_mode,
        require_verification=require_verification, max_model_calls=max_model_calls,
    )
    if updated is None:
        error_exit(f"tier {tier} not found")
    out = TierOut.from_policy(updated)
    emit(ctx, json_data=out.model_dump(), table=lambda: _table([out]))


@app.command("set-models")
def tiers_set_models(
    ctx: typer.Context, tier: int, models: str = typer.Option(..., "--models", help="comma-separated model ids")
) -> None:
    updated = get_tier_service().set_models(tier, models.split(","))
    if updated is None:
        error_exit(f"tier {tier} not found")
    out = TierOut.from_policy(updated)
    emit(ctx, json_data=out.model_dump(), table=lambda: _table([out]))


@app.command("add-model")
def tiers_add_model(ctx: typer.Context, tier: int, model: str = typer.Option(..., "--model")) -> None:
    updated = get_tier_service().add_model(tier, model)
    if updated is None:
        error_exit(f"tier {tier} not found")
    out = TierOut.from_policy(updated)
    emit(ctx, json_data=out.model_dump(), table=lambda: _table([out]))

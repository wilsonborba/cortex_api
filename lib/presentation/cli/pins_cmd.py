from __future__ import annotations

from typing import List, Optional

import typer

from lib.presentation.api.deps import get_pin_repo
from lib.presentation.api.schemas.pins import PinOut
from lib.presentation.cli.output import console, emit, error_exit, print_table

app = typer.Typer(help="Manual routing pins for reproducible A/B testing (issue #8).")


def _table(pins: List[PinOut]) -> None:
    print_table(
        "Pins",
        ["TIER", "TASK", "STRATEGY", "MODEL", "PINNED BY", "ACTIVE"],
        [[p.tier, p.task, p.strategy_id, p.model_id, p.pinned_by, p.is_active] for p in pins],
    )


@app.command("set")
def pin_set(
    ctx: typer.Context,
    tier: int = typer.Option(..., "--tier"),
    task: Optional[str] = typer.Option(None, "--task"),
    strategy: Optional[str] = typer.Option(None, "--strategy"),
    model: Optional[str] = typer.Option(None, "--model"),
    pinned_by: Optional[str] = typer.Option(None, "--pinned-by"),
) -> None:
    if not strategy and not model:
        error_exit("pass at least one of --strategy or --model")
    pin = get_pin_repo().set_pin(tier=tier, task_type=task, strategy_id=strategy, model_id=model, pinned_by=pinned_by)
    out = PinOut.from_pin(pin)
    emit(ctx, json_data=out.model_dump(), table=lambda: _table([out]))


@app.command("list")
def pin_list(ctx: typer.Context) -> None:
    pins = [PinOut.from_pin(p) for p in get_pin_repo().list_pins()]
    emit(ctx, json_data=[p.model_dump() for p in pins], table=lambda: _table(pins))


@app.command("remove")
def pin_remove(ctx: typer.Context, tier: int = typer.Option(..., "--tier"), task: Optional[str] = typer.Option(None, "--task")) -> None:
    removed = get_pin_repo().remove_pin(tier, task_type=task)
    if not removed:
        error_exit(f"no pin for tier={tier} task={task!r}")
    payload = {"removed": True, "tier": tier, "task": task}
    emit(ctx, json_data=payload, table=lambda: console.print(f"Removed pin for tier={tier} task={task!r}"))

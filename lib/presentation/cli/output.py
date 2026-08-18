from __future__ import annotations

import json as _json
import sys
from dataclasses import dataclass
from typing import Any, Iterable, Sequence

import typer
from rich.console import Console
from rich.table import Table

console = Console()
error_console = Console(stderr=True)


@dataclass
class CliState:
    """Global `cortex` options, set once in main.py's root callback and
    inherited by every subcommand via `ctx.obj` (standard Click/Typer
    pattern for state shared across a command tree)."""

    verbose: bool = False
    json_output: bool = False


def state_from(ctx: typer.Context) -> CliState:
    return ctx.find_object(CliState) or CliState()


def json_mode(ctx: typer.Context) -> bool:
    """`--json` always wins; otherwise a non-interactive stdout (piped or
    redirected) defaults to JSON too, so `cortex ... | jq` just works
    without remembering the flag."""
    if state_from(ctx).json_output:
        return True
    return not sys.stdout.isatty()


def print_json(data: Any) -> None:
    # Deliberately not routed through Rich's Console: it word-wraps plain
    # text to the terminal width, which silently corrupts long JSON lines
    # (splits them with real newlines) once output would exceed one line --
    # exactly what --json / a pipe destination must never do.
    print(_json.dumps(data, default=str, indent=2))


def print_table(title: str, columns: Sequence[str], rows: Iterable[Sequence[Any]]) -> None:
    table = Table(title=title)
    for column in columns:
        table.add_column(column)
    row_count = 0
    for row in rows:
        table.add_row(*[_cell(v) for v in row])
        row_count += 1
    if row_count == 0:
        console.print(f"[dim]{title}: no results.[/dim]")
        return
    console.print(table)


def _cell(value: Any) -> str:
    if value is None:
        return "-"
    if isinstance(value, bool):
        return "yes" if value else "no"
    if isinstance(value, (list, tuple)):
        return ", ".join(str(v) for v in value) or "-"
    if isinstance(value, dict):
        return ", ".join(f"{k}={v}" for k, v in value.items()) or "-"
    return str(value)


def error_exit(message: str, code: int = 1) -> None:
    error_console.print(f"[bold red]Error:[/bold red] {message}")
    raise typer.Exit(code=code)


def emit(ctx: typer.Context, *, json_data: Any, table: Any = None) -> None:
    """`json_data` is always a plain dict/list (from a Pydantic schema's
    `.model_dump()`); `table` is a no-arg callable that renders the Rich
    table when not in JSON mode, so building it is skipped entirely when
    it won't be used."""
    if json_mode(ctx):
        print_json(json_data)
    elif table is not None:
        table()

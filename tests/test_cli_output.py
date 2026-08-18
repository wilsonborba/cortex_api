from __future__ import annotations

from typing import Optional

import pytest
import typer

from lib.presentation.cli.output import CliState, _cell, json_mode, state_from


def _ctx(state: Optional[CliState]) -> typer.Context:
    dummy = typer.Typer()

    @dummy.command()
    def cmd() -> None:
        pass

    ctx = typer.Context(typer.main.get_command(dummy))
    ctx.obj = state
    return ctx


def test_state_from_returns_default_when_unset():
    ctx = _ctx(None)
    assert state_from(ctx) == CliState()


def test_state_from_returns_the_set_state():
    ctx = _ctx(CliState(verbose=True, json_output=True))
    assert state_from(ctx).json_output is True


def test_json_mode_true_when_explicitly_requested(monkeypatch: pytest.MonkeyPatch):
    monkeypatch.setattr("sys.stdout.isatty", lambda: True)  # interactive terminal
    ctx = _ctx(CliState(json_output=True))
    assert json_mode(ctx) is True  # --json wins even in a real terminal


def test_json_mode_true_when_stdout_is_not_a_tty(monkeypatch: pytest.MonkeyPatch):
    monkeypatch.setattr("sys.stdout.isatty", lambda: False)  # piped/redirected
    ctx = _ctx(CliState(json_output=False))
    assert json_mode(ctx) is True


def test_json_mode_false_in_an_interactive_terminal_without_the_flag(monkeypatch: pytest.MonkeyPatch):
    monkeypatch.setattr("sys.stdout.isatty", lambda: True)
    ctx = _ctx(CliState(json_output=False))
    assert json_mode(ctx) is False


@pytest.mark.parametrize(
    "value,expected",
    [
        (None, "-"),
        (True, "yes"),
        (False, "no"),
        ([1, 2, 3], "1, 2, 3"),
        ([], "-"),
        ({"a": 1, "b": 2}, "a=1, b=2"),
        ("plain", "plain"),
        (42, "42"),
    ],
)
def test_cell_formatting(value, expected):
    assert _cell(value) == expected

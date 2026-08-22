from __future__ import annotations

import subprocess
from typing import Any, Callable, List, Optional

from lib.engine.drivers.base import DriverResult, failed, looks_like_rate_limit, parse_json_or_none


class CodexDriver:
    """Executes a prompt via `codex exec --json -m <model> "<prompt>"`.

    Output is JSONL (one event per line): a `turn.completed` event carries
    token usage, an `item.completed` / `agent_message` event carries the
    final response text.
    """

    provider = "codex"

    def __init__(
        self,
        command: str = "codex",
        timeout: float = 300.0,
        runner: Optional[Callable[..., subprocess.CompletedProcess]] = None,
    ) -> None:
        self._command = command
        self._timeout = timeout
        self._runner = runner or subprocess.run

    def run(self, model: str, prompt: str, images: Optional[List[str]] = None) -> DriverResult:
        # `images` unused: not verified that the `codex` CLI accepts a file/image attachment.
        try:
            result = self._runner(
                [self._command, "exec", "--json", "-m", model, prompt],
                capture_output=True,
                text=True,
                timeout=self._timeout,
            )
        except (OSError, subprocess.TimeoutExpired) as exc:
            return failed("unreachable", str(exc))

        events = _parse_jsonl(result.stdout)
        response_text = _extract_response_text(events)
        usage = _extract_usage(events)

        if usage is None or result.returncode != 0:
            combined = f"{result.stdout}\n{result.stderr}"
            error_type = "rate_limit" if looks_like_rate_limit(combined) else "cli_error"
            return failed(error_type, combined.strip()[:500], response_text=response_text)

        return DriverResult(
            success=True,
            response_text=response_text,
            input_tokens=int(usage.get("input_tokens", 0) or 0),
            output_tokens=int(usage.get("output_tokens", 0) or 0),
            latency_ms=0,  # not reported by `codex exec --json`
            raw={"events": events},
        )


def _parse_jsonl(stdout: str) -> List[dict[str, Any]]:
    events: List[dict[str, Any]] = []
    for line in stdout.splitlines():
        line = line.strip()
        if not line:
            continue
        parsed = parse_json_or_none(line)
        if parsed is not None:
            events.append(parsed)
    return events


def _extract_usage(events: List[dict[str, Any]]) -> Optional[dict[str, Any]]:
    for event in events:
        if event.get("type") == "turn.completed":
            return event.get("usage")
    return None


def _extract_response_text(events: List[dict[str, Any]]) -> str:
    for event in events:
        if event.get("type") != "item.completed":
            continue
        item = event.get("item") or {}
        if item.get("type") == "agent_message":
            return item.get("text", "")
    return ""

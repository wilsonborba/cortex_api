from __future__ import annotations

import subprocess
from typing import Callable, Optional

from lib.engine.drivers.base import DriverResult, failed, looks_like_rate_limit, parse_json_or_none


class AgyDockerDriver:
    """Executes a prompt via `agy-docker -p "<prompt>" --output-format json`."""

    provider = "agy"

    def __init__(
        self,
        command: str = "agy-docker",
        timeout: float = 180.0,
        runner: Optional[Callable[..., subprocess.CompletedProcess]] = None,
    ) -> None:
        self._command = command
        self._timeout = timeout
        self._runner = runner or subprocess.run

    def run(self, model: str, prompt: str) -> DriverResult:
        try:
            result = self._runner(
                [self._command, "-p", prompt, "--output-format", "json", "--model", model],
                capture_output=True,
                text=True,
                timeout=self._timeout,
            )
        except (OSError, subprocess.TimeoutExpired) as exc:
            return failed("unreachable", str(exc))

        combined = f"{result.stdout}\n{result.stderr}"
        data = parse_json_or_none(result.stdout)
        if data is None:
            return failed("rate_limit" if looks_like_rate_limit(combined) else "cli_error", combined.strip()[:500])

        status = str(data.get("status") or "").upper()
        if status != "SUCCESS" or result.returncode != 0:
            error_type = "rate_limit" if looks_like_rate_limit(str(data), combined) else "cli_error"
            return DriverResult(
                success=False,
                response_text=data.get("response", ""),
                input_tokens=0,
                output_tokens=0,
                latency_ms=int((data.get("duration_seconds", 0) or 0) * 1000),
                error_type=error_type,
                error_message=status or "error",
                raw=data,
            )

        usage = data.get("usage") or {}
        return DriverResult(
            success=True,
            response_text=data.get("response", ""),
            input_tokens=int(usage.get("input_tokens", 0) or 0),
            output_tokens=int(usage.get("output_tokens", 0) or 0),
            latency_ms=int((data.get("duration_seconds", 0) or 0) * 1000),
            raw=data,
        )

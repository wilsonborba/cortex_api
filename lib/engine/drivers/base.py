from __future__ import annotations

import json
from dataclasses import dataclass, field
from typing import Any, Optional, Protocol

# Structured-output errors only: no free-text scraping. A driver falls back to
# these markers only when a CLI/HTTP call fails outright and there's no JSON
# payload to read `is_error`/`api_error_status` from.
RATE_LIMIT_MARKERS = (
    "rate_limit",
    "rate limit",
    "429",
    "usage limit",
    "quota exceeded",
    "try again later",
)


def looks_like_rate_limit(*texts: Optional[str]) -> bool:
    haystack = " ".join(t for t in texts if t).lower()
    return any(marker in haystack for marker in RATE_LIMIT_MARKERS)


@dataclass(frozen=True)
class DriverResult:
    """The outcome of one non-interactive execution against a provider."""

    success: bool
    response_text: str
    input_tokens: int
    output_tokens: int
    latency_ms: int
    cost_usd: float = 0.0
    error_type: Optional[str] = None  # "rate_limit" | "unreachable" | "cli_error" | "http_error"
    error_message: Optional[str] = None
    raw: dict[str, Any] = field(default_factory=dict)


def parse_json_or_none(text: str) -> Optional[dict[str, Any]]:
    try:
        return json.loads(text)
    except (ValueError, TypeError):
        return None


def failed(error_type: str, message: str, response_text: str = "", latency_ms: int = 0) -> DriverResult:
    return DriverResult(
        success=False,
        response_text=response_text,
        input_tokens=0,
        output_tokens=0,
        latency_ms=latency_ms,
        error_type=error_type,
        error_message=message,
    )


class ExecutionDriver(Protocol):
    """Contract every execution driver implements."""

    provider: str

    def run(self, model: str, prompt: str) -> DriverResult:
        """Executes `prompt` against `model` non-interactively and returns
        structured token/latency/cost usage parsed from the provider's own
        JSON output (never regex over free text)."""
        ...

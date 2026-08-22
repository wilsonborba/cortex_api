from __future__ import annotations

import time
from typing import Callable, List, Optional

import httpx

from lib.engine.drivers.base import DriverResult, failed


class OpenAICompatibleDriver:
    """Executes a prompt via any provider's OpenAI-compatible
    `POST /chat/completions`, authenticated with a bearer token.

    Base class for the free-tier provider adapters (`docs/free-tier-adapters-plan.md`).
    """

    def __init__(
        self,
        provider: str,
        base_url: str,
        api_key: Optional[str],
        chat_path: str = "/chat/completions",
        timeout: float = 60.0,
        client_factory: Optional[Callable[[], httpx.Client]] = None,
    ) -> None:
        self.provider = provider
        self._base_url = base_url.rstrip("/")
        self._api_key = api_key
        self._chat_path = chat_path
        self._timeout = timeout
        self._client_factory = client_factory or (lambda: httpx.Client(timeout=self._timeout))

    def run(self, model: str, prompt: str, images: Optional[List[str]] = None) -> DriverResult:
        if not self._api_key:
            return failed("unreachable", f"{self.provider}: no API key configured")

        content: object = prompt
        if images:
            content = [{"type": "text", "text": prompt}] + [
                {"type": "image_url", "image_url": {"url": data_uri}} for data_uri in images
            ]

        started = time.monotonic()
        try:
            with self._client_factory() as client:
                response = client.post(
                    f"{self._base_url}{self._chat_path}",
                    headers=self._headers(),
                    json={"model": model, "messages": [{"role": "user", "content": content}]},
                )
                response.raise_for_status()
                data = response.json()
        except httpx.HTTPStatusError as exc:
            status = exc.response.status_code if exc.response is not None else None
            if status in (401, 403):
                err_type = "auth_failure"
            elif status == 429:
                err_type = "rate_limit_exceeded"
            elif status == 404:
                err_type = "provider_not_found"
            elif status and status >= 500:
                err_type = "provider_server_error"
            else:
                err_type = "http_error"
            return failed(err_type, str(exc))
        except httpx.TimeoutException as exc:
            return failed("provider_timeout", str(exc))
        except (httpx.HTTPError, ValueError) as exc:
            return failed("unreachable", str(exc))

        latency_ms = int((time.monotonic() - started) * 1000)
        choices = data.get("choices") or []
        message = (choices[0].get("message") or {}) if choices else {}
        text = message.get("content") or ""
        if not text.strip():
            return failed("empty_response", f"{self.provider}: returned empty response text")

        usage = data.get("usage") or {}
        return DriverResult(
            success=True,
            response_text=text,
            input_tokens=int(usage.get("prompt_tokens", 0) or 0),
            output_tokens=int(usage.get("completion_tokens", 0) or 0),
            latency_ms=latency_ms,
            raw=data,
        )

    def _headers(self) -> dict[str, str]:
        return {"Authorization": f"Bearer {self._api_key}", "Content-Type": "application/json"}

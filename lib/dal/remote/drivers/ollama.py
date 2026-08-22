from __future__ import annotations

from typing import Any, Callable, List, Optional

import httpx

from lib.engine.drivers.base import DriverResult, failed


class OllamaDriver:
    """Executes a prompt against a local Ollama model via `POST /api/generate`."""

    provider = "ollama"

    def __init__(
        self,
        base_url: str,
        timeout: float = 180.0,
        client_factory: Optional[Callable[[], httpx.Client]] = None,
    ) -> None:
        self._base_url = base_url.rstrip("/")
        self._timeout = timeout
        self._client_factory = client_factory or (lambda: httpx.Client(timeout=self._timeout))

    def run(self, model: str, prompt: str, images: Optional[List[str]] = None) -> DriverResult:
        body: dict[str, Any] = {"model": model, "prompt": prompt, "stream": False}
        if images:
            body["images"] = [_strip_data_uri_prefix(img) for img in images]

        try:
            with self._client_factory() as client:
                response = client.post(f"{self._base_url}/api/generate", json=body)
                response.raise_for_status()
                data = response.json()
        except httpx.HTTPStatusError as exc:
            status = exc.response.status_code if exc.response is not None else None
            return failed("rate_limit" if status == 429 else "http_error", str(exc))
        except (httpx.HTTPError, ValueError) as exc:
            return failed("unreachable", str(exc))

        return DriverResult(
            success=True,
            response_text=data.get("response", ""),
            input_tokens=int(data.get("prompt_eval_count", 0) or 0),
            output_tokens=int(data.get("eval_count", 0) or 0),
            latency_ms=int((data.get("total_duration", 0) or 0) / 1_000_000),
            raw=data,
        )


def _strip_data_uri_prefix(image: str) -> str:
    """Ollama's `images` field wants raw base64, no `data:` URI prefix."""
    if image.startswith("data:") and "," in image:
        return image.split(",", 1)[1]
    return image

"""Fallback audio transcription via Groq's hosted Whisper endpoint.

Only used when local transcription (`whisper_local.py`) is unavailable or
fails and a `groq_api_key` is configured -- never the default path.
"""

from __future__ import annotations

from typing import Callable, Optional

import httpx

DEFAULT_BASE_URL = "https://api.groq.com/openai/v1"
DEFAULT_MODEL = "whisper-large-v3-turbo"


class GroqTranscriptionError(RuntimeError):
    pass


class GroqTranscriptionDriver:
    def __init__(
        self,
        api_key: Optional[str],
        base_url: str = DEFAULT_BASE_URL,
        model: str = DEFAULT_MODEL,
        timeout: float = 60.0,
        client_factory: Optional[Callable[[], httpx.Client]] = None,
    ) -> None:
        self._api_key = api_key
        self._base_url = base_url.rstrip("/")
        self._model = model
        self._timeout = timeout
        self._client_factory = client_factory or (lambda: httpx.Client(timeout=self._timeout))

    def transcribe(self, audio_bytes: bytes, filename: str = "audio.wav") -> str:
        if not self._api_key:
            raise GroqTranscriptionError("groq_api_key not configured")

        try:
            with self._client_factory() as client:
                response = client.post(
                    f"{self._base_url}/audio/transcriptions",
                    headers={"Authorization": f"Bearer {self._api_key}"},
                    files={"file": (filename, audio_bytes)},
                    data={"model": self._model},
                )
                response.raise_for_status()
                data = response.json()
        except (httpx.HTTPError, ValueError) as exc:
            raise GroqTranscriptionError(f"Groq transcription failed: {exc}") from exc

        return str(data.get("text", "")).strip()

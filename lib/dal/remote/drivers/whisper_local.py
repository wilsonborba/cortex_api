"""Local audio transcription via `pywhispercpp` (whisper.cpp bindings).

Single model, resolved once (`ggml-large-v3-turbo-q5_0.bin`, multilingual --
covers EN/PT/ES/TH). The model is loaded lazily and cached on the instance:
reloading a 570MB model per call would dominate transcription latency.
"""

from __future__ import annotations

import tempfile
from pathlib import Path
from typing import Optional

from lib.core.logs import get_logger

logger = get_logger(__name__)


class LocalWhisperUnavailableError(RuntimeError):
    """Raised when `pywhispercpp` isn't installed or the model can't load."""


class LocalWhisperTranscriber:
    def __init__(self, model_name: str, models_dir: str, n_threads: int = 8) -> None:
        self._model_name = model_name
        self._models_dir = models_dir
        self._n_threads = n_threads
        self._model = None  # lazy-loaded on first transcribe() call

    def _load_model(self):
        if self._model is not None:
            return self._model
        try:
            from pywhispercpp.model import Model
        except ImportError as exc:
            raise LocalWhisperUnavailableError(
                "pywhispercpp is not installed (pip install 'cortex[media]')"
            ) from exc

        Path(self._models_dir).mkdir(parents=True, exist_ok=True)
        try:
            self._model = Model(
                self._model_name, models_dir=self._models_dir, n_threads=self._n_threads, print_progress=False
            )
        except Exception as exc:  # pragma: no cover -- depends on network/disk at model-download time
            raise LocalWhisperUnavailableError(f"failed to load local whisper model: {exc}") from exc
        return self._model

    def transcribe(self, audio_bytes: bytes, suffix: str = ".wav") -> str:
        """Transcribes raw audio bytes (any format ffmpeg/whisper.cpp can read)."""
        model = self._load_model()
        with tempfile.NamedTemporaryFile(suffix=suffix) as tmp:
            tmp.write(audio_bytes)
            tmp.flush()
            segments = model.transcribe(tmp.name)
        return " ".join(segment.text.strip() for segment in segments).strip()


_default_transcriber: Optional[LocalWhisperTranscriber] = None


def get_default_transcriber(model_name: str, models_dir: str) -> LocalWhisperTranscriber:
    """Process-wide singleton so the model loads at most once per process."""
    global _default_transcriber
    if _default_transcriber is None or _default_transcriber._model_name != model_name:
        _default_transcriber = LocalWhisperTranscriber(model_name=model_name, models_dir=models_dir)
    return _default_transcriber

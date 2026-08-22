"""CUDA-backed local audio transcription via ``faster-whisper``.

The model is loaded lazily and cached per process.  Unlike the PyPI
``pywhispercpp`` wheel, which is CPU-only, CTranslate2's Linux wheel can use
the NVIDIA CUDA runtime installed with the application.
"""

from __future__ import annotations

import ctypes
import importlib.util
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
            self._preload_cuda_libraries()
            from faster_whisper import WhisperModel
        except ImportError as exc:
            raise LocalWhisperUnavailableError(
                "faster-whisper is not installed (pip install 'cortex[media]')"
            ) from exc

        Path(self._models_dir).mkdir(parents=True, exist_ok=True)
        try:
            self._model = WhisperModel(
                self._resolve_model_name(),
                device="cuda",
                compute_type="int8_float16",
                download_root=self._models_dir,
            )
        except Exception as exc:  # pragma: no cover -- depends on network/disk at model-download time
            raise LocalWhisperUnavailableError(f"failed to load local whisper model: {exc}") from exc
        return self._model

    def _resolve_model_name(self) -> str:
        """Map the former whisper.cpp quantized name to Faster-Whisper's ID."""
        if self._model_name == "large-v3-turbo-q5_0":
            return "large-v3-turbo"
        return self._model_name

    @staticmethod
    def _preload_cuda_libraries() -> None:
        """Make pip-installed CUDA libraries visible to CTranslate2 under systemd.

        ``LD_LIBRARY_PATH`` is read at process start.  Loading the absolute
        library paths here keeps the service unit simple and avoids relying on
        a shell-specific environment variable.
        """
        package_dirs = (
            "nvidia.cuda_runtime.lib",
            "nvidia.cublas.lib",
            "nvidia.cudnn.lib",
        )
        directories = []
        for package in package_dirs:
            spec = importlib.util.find_spec(package)
            if spec and spec.submodule_search_locations:
                directories.append(Path(next(iter(spec.submodule_search_locations))))

        required = (
            "libcudart.so.12",
            "libcublasLt.so.12",
            "libcublas.so.12",
            "libcudnn.so.9",
            "libcudnn_ops.so.9",
            "libcudnn_cnn.so.9",
            "libcudnn_adv.so.9",
            "libcudnn_graph.so.9",
        )
        for library in required:
            path = next((directory / library for directory in directories if (directory / library).is_file()), None)
            if path is None:
                raise LocalWhisperUnavailableError(f"CUDA runtime library {library!r} is unavailable")
            ctypes.CDLL(str(path), mode=ctypes.RTLD_GLOBAL)

    def transcribe(self, audio_bytes: bytes, suffix: str = ".wav") -> str:
        """Transcribes raw audio bytes (any format ffmpeg/whisper.cpp can read)."""
        model = self._load_model()
        with tempfile.NamedTemporaryFile(suffix=suffix) as tmp:
            tmp.write(audio_bytes)
            tmp.flush()
            segments, _info = model.transcribe(tmp.name, beam_size=1)
            return " ".join(segment.text.strip() for segment in segments).strip()


_default_transcriber: Optional[LocalWhisperTranscriber] = None


def get_default_transcriber(model_name: str, models_dir: str) -> LocalWhisperTranscriber:
    """Process-wide singleton so the model loads at most once per process."""
    global _default_transcriber
    if _default_transcriber is None or _default_transcriber._model_name != model_name:
        _default_transcriber = LocalWhisperTranscriber(model_name=model_name, models_dir=models_dir)
    return _default_transcriber

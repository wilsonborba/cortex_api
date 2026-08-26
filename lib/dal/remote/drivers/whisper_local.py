"""Compatibility import for the canonical CUDA local transcription driver.

Historically this module duplicated the engine implementation. Keep this path
for callers outside the engine while ensuring one runtime implementation.
"""

from lib.engine.drivers.whisper_local import (
    LocalWhisperTranscriber,
    LocalWhisperUnavailableError,
    get_default_transcriber,
)

__all__ = [
    "LocalWhisperTranscriber",
    "LocalWhisperUnavailableError",
    "get_default_transcriber",
]

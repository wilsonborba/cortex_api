from __future__ import annotations

import base64

import pytest

from lib.core.settings import Settings
from lib.engine.attachments import Attachment, AttachmentIngestor
from lib.engine.drivers.groq_audio import GroqTranscriptionError
from lib.engine.drivers.whisper_local import LocalWhisperUnavailableError


class _FakeLocalTranscriber:
    def __init__(self, text: str = "hello from audio", raises: Exception | None = None) -> None:
        self._text = text
        self._raises = raises

    def transcribe(self, audio_bytes: bytes, suffix: str = ".wav") -> str:
        if self._raises:
            raise self._raises
        return self._text


class _FakeGroqTranscriber:
    def __init__(self, text: str = "hello from groq", raises: Exception | None = None) -> None:
        self._text = text
        self._raises = raises

    def transcribe(self, audio_bytes: bytes, filename: str = "audio.wav") -> str:
        if self._raises:
            raise self._raises
        return self._text


def _audio_attachment(filename: str = "note.wav") -> Attachment:
    return Attachment(filename=filename, mime_type="audio/wav", data_base64=base64.b64encode(b"fake-audio").decode())


def _image_attachment(filename: str = "photo.png") -> Attachment:
    return Attachment(filename=filename, mime_type="image/png", data_base64=base64.b64encode(b"fake-png").decode())


def test_audio_attachment_transcribed_via_local_whisper():
    ingestor = AttachmentIngestor(
        settings=Settings(whisper_local_enabled=True),
        local_transcriber=_FakeLocalTranscriber("transcribed text"),
    )

    result = ingestor.ingest([_audio_attachment()])

    assert result.ok
    assert "transcribed text" in result.text_context


def test_audio_attachment_falls_back_to_groq_when_local_unavailable():
    ingestor = AttachmentIngestor(
        settings=Settings(whisper_local_enabled=True, groq_api_key="fake-key"),
        local_transcriber=_FakeLocalTranscriber(raises=LocalWhisperUnavailableError("not installed")),
        groq_transcriber=_FakeGroqTranscriber("groq transcript"),
    )

    result = ingestor.ingest([_audio_attachment()])

    assert result.ok
    assert "groq transcript" in result.text_context


def test_audio_attachment_errors_when_no_backend_available(monkeypatch):
    # `.env` may define a real Groq key: init kwargs lose to dotenv for
    # aliased Settings fields (see tests/test_settings.py), so force it
    # unset via the environment instead of the constructor kwarg.
    monkeypatch.setenv("CORTEX_GROQ_API_KEY", "")
    ingestor = AttachmentIngestor(
        settings=Settings(whisper_local_enabled=True),
        local_transcriber=_FakeLocalTranscriber(raises=LocalWhisperUnavailableError("not installed")),
    )

    result = ingestor.ingest([_audio_attachment()])

    assert not result.ok
    assert "no transcription backend available" in result.errors[0]


def test_audio_attachment_errors_when_groq_backend_fails():
    ingestor = AttachmentIngestor(
        settings=Settings(whisper_local_enabled=False, groq_api_key="fake-key"),
        groq_transcriber=_FakeGroqTranscriber(raises=GroqTranscriptionError("boom")),
    )

    result = ingestor.ingest([_audio_attachment()])

    assert not result.ok
    assert "transcription failed" in result.errors[0]


def test_image_attachment_passed_through_when_model_is_vision_capable():
    ingestor = AttachmentIngestor(settings=Settings())

    result = ingestor.ingest([_image_attachment()], model_is_vision_capable=True)

    assert result.ok
    assert result.image_data_uris == ["data:image/png;base64," + base64.b64encode(b"fake-png").decode()]


def test_image_attachment_rejected_when_model_is_not_vision_capable():
    ingestor = AttachmentIngestor(settings=Settings())

    result = ingestor.ingest([_image_attachment()], model_is_vision_capable=False)

    assert not result.ok
    assert "not marked vision-capable" in result.errors[0]


def test_unsupported_mime_type_is_rejected_explicitly():
    ingestor = AttachmentIngestor(settings=Settings())
    attachment = Attachment(filename="clip.mp4", mime_type="video/mp4", data_base64="AAAA")

    result = ingestor.ingest([attachment])

    assert not result.ok
    assert "unsupported attachment mime type" in result.errors[0]

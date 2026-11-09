from __future__ import annotations

import base64
import io

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
    assert "audio transcription is unavailable" in result.errors[0]


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


def _pdf_attachment(text: str | None = "Hello from a real PDF", filename: str = "doc.pdf") -> Attachment:
    import pymupdf

    document = pymupdf.open()
    page = document.new_page(width=200, height=200)
    if text:
        page.insert_text((20, 40), text)
    pdf_bytes = document.tobytes()
    document.close()
    return Attachment(filename=filename, mime_type="application/pdf", data_base64=base64.b64encode(pdf_bytes).decode())


def test_pdf_attachment_with_real_text_is_extracted():
    ingestor = AttachmentIngestor(settings=Settings())

    result = ingestor.ingest([_pdf_attachment("Hello from a real PDF")])

    assert result.ok
    assert "Hello from a real PDF" in result.text_context


def test_pdf_attachment_with_no_extractable_text_errors_explicitly():
    ingestor = AttachmentIngestor(settings=Settings())

    result = ingestor.ingest([_pdf_attachment(text=None)])

    assert not result.ok
    assert "no extractable text" in result.errors[0]


def test_pdf_attachment_invalid_bytes_errors_explicitly():
    ingestor = AttachmentIngestor(settings=Settings())
    attachment = Attachment(
        filename="broken.pdf", mime_type="application/pdf", data_base64=base64.b64encode(b"not a pdf").decode()
    )

    result = ingestor.ingest([attachment])

    assert not result.ok
    assert "could not read PDF" in result.errors[0]


def test_plain_text_attachment_is_decoded_and_included():
    ingestor = AttachmentIngestor(settings=Settings())
    attachment = Attachment(
        filename="notes.txt", mime_type="text/plain", data_base64=base64.b64encode(b"Meeting notes here").decode()
    )

    result = ingestor.ingest([attachment])

    assert result.ok
    assert "Meeting notes here" in result.text_context


def test_non_utf8_text_attachment_errors_explicitly():
    ingestor = AttachmentIngestor(settings=Settings())
    attachment = Attachment(
        filename="bad.txt", mime_type="text/plain", data_base64=base64.b64encode(b"\xff\xfe\x00").decode()
    )

    result = ingestor.ingest([attachment])

    assert not result.ok
    assert "not valid UTF-8" in result.errors[0]


_DOCX_MIME = "application/vnd.openxmlformats-officedocument.wordprocessingml.document"


def _docx_attachment(text: str | None = "Hello from a Word document", filename: str = "doc.docx") -> Attachment:
    import docx

    document = docx.Document()
    if text:
        document.add_paragraph(text)
    buf = io.BytesIO()
    document.save(buf)
    return Attachment(filename=filename, mime_type=_DOCX_MIME, data_base64=base64.b64encode(buf.getvalue()).decode())


def test_docx_attachment_with_real_text_is_extracted():
    ingestor = AttachmentIngestor(settings=Settings())

    result = ingestor.ingest([_docx_attachment("Hello from a Word document")])

    assert result.ok
    assert "Hello from a Word document" in result.text_context


def test_docx_attachment_with_no_text_errors_explicitly():
    ingestor = AttachmentIngestor(settings=Settings())

    result = ingestor.ingest([_docx_attachment(text=None)])

    assert not result.ok
    assert "no text content" in result.errors[0]


def test_docx_attachment_invalid_bytes_errors_explicitly():
    ingestor = AttachmentIngestor(settings=Settings())
    attachment = Attachment(filename="broken.docx", mime_type=_DOCX_MIME, data_base64=base64.b64encode(b"not a docx").decode())

    result = ingestor.ingest([attachment])

    assert not result.ok
    assert "could not read Word document" in result.errors[0]

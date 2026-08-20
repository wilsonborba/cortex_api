"""Attachment ingestion: audio -> transcript, image -> vision payload.

Never a silent fallback. Any mime type this can't genuinely handle, or any
backend that fails with nothing left to fall back to, comes back as an
explicit error on `IngestedAttachments.errors` -- the Executor aborts the
request rather than answering against a partially-ingested attachment.
"""

from __future__ import annotations

import base64
from dataclasses import dataclass, field
from typing import List, Optional, Protocol

from lib.core.logs import get_logger
from lib.core.settings import Settings, get_settings
from lib.engine.drivers.groq_audio import GroqTranscriptionDriver, GroqTranscriptionError
from lib.engine.drivers.whisper_local import LocalWhisperUnavailableError, get_default_transcriber

logger = get_logger(__name__)


@dataclass(frozen=True)
class Attachment:
    filename: str
    mime_type: str
    data_base64: str


@dataclass(frozen=True)
class IngestedAttachments:
    text_context: str = ""
    image_data_uris: List[str] = field(default_factory=list)
    errors: List[str] = field(default_factory=list)

    @property
    def ok(self) -> bool:
        return not self.errors


class _Transcriber(Protocol):
    def transcribe(self, audio_bytes: bytes, suffix: str = ".wav") -> str: ...


class AttachmentIngestor:
    def __init__(
        self,
        settings: Optional[Settings] = None,
        local_transcriber: Optional[_Transcriber] = None,
        groq_transcriber: Optional[GroqTranscriptionDriver] = None,
    ) -> None:
        self._settings = settings or get_settings()
        self._local_transcriber = local_transcriber
        self._groq_transcriber = groq_transcriber or GroqTranscriptionDriver(api_key=self._settings.groq_api_key)

    def _get_local_transcriber(self) -> _Transcriber:
        if self._local_transcriber is not None:
            return self._local_transcriber
        return get_default_transcriber(
            model_name=self._settings.whisper_local_model, models_dir=self._settings.whisper_models_dir
        )

    def ingest(self, attachments: List[Attachment], *, model_is_vision_capable: bool = False) -> IngestedAttachments:
        text_parts: List[str] = []
        image_data_uris: List[str] = []
        errors: List[str] = []

        for attachment in attachments:
            if attachment.mime_type.startswith("audio/"):
                transcript, error = self._transcribe(attachment)
                if error:
                    errors.append(error)
                elif transcript:
                    text_parts.append(f"[Attachment transcript: {attachment.filename}]\n{transcript}")
            elif attachment.mime_type.startswith("image/"):
                if not model_is_vision_capable:
                    errors.append(
                        f"attachment {attachment.filename!r} is an image but the routed model is not "
                        "marked vision-capable in the model registry"
                    )
                    continue
                image_data_uris.append(f"data:{attachment.mime_type};base64,{attachment.data_base64}")
            else:
                errors.append(f"unsupported attachment mime type {attachment.mime_type!r} ({attachment.filename!r})")

        return IngestedAttachments(text_context="\n\n".join(text_parts), image_data_uris=image_data_uris, errors=errors)

    def _transcribe(self, attachment: Attachment) -> tuple[str, Optional[str]]:
        try:
            audio_bytes = base64.b64decode(attachment.data_base64)
        except Exception as exc:
            return "", f"attachment {attachment.filename!r}: invalid base64 ({exc})"

        if self._settings.whisper_local_enabled:
            try:
                text = self._get_local_transcriber().transcribe(audio_bytes)
                return text, None
            except LocalWhisperUnavailableError as exc:
                logger.warning("local whisper unavailable, falling back to Groq: %s", exc)

        if self._settings.groq_api_key:
            try:
                text = self._groq_transcriber.transcribe(audio_bytes, filename=attachment.filename)
                return text, None
            except GroqTranscriptionError as exc:
                return "", f"attachment {attachment.filename!r}: transcription failed ({exc})"

        profile = getattr(self._settings, "profile", "complete")
        return "", (
            f"attachment {attachment.filename!r}: audio transcription is unavailable under profile '{profile}' "
            "(local Whisper is not installed/enabled and no CORTEX_GROQ_API_KEY is configured for cloud transcription. "
            "To enable: configure CORTEX_GROQ_API_KEY in .env or reinstall with './scripts/install.sh --profile complete')"
        )


def build_default_attachment_ingestor(settings: Optional[Settings] = None) -> AttachmentIngestor:
    return AttachmentIngestor(settings=settings or get_settings())

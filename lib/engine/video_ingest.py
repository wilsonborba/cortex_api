"""Video understanding pipeline: extract -> transcribe -> caption -> fuse.

Runs as a background job (see `lib/engine/video_jobs.py`), never inline on
the synchronous `/execute` path -- extraction + per-frame captioning is too
slow to fit a tier's latency budget. Reuses the Phase 2 pieces: the same
local-Whisper-first/Groq-fallback transcription
(`lib.engine.attachments.AttachmentIngestor`) and the single resolved
vision model (`settings.vision_model`, called through `OllamaDriver`) --
never a second vision model spun up just for video.
"""

from __future__ import annotations

import base64
import subprocess
import tempfile
from dataclasses import dataclass, field
from pathlib import Path
from typing import List, Optional

from lib.core.logs import get_logger
from lib.core.media_sanitize import sanitize_mp4
from lib.core.settings import Settings, get_settings
from lib.engine.attachments import Attachment, AttachmentIngestor
from lib.engine.drivers.ollama import OllamaDriver

logger = get_logger(__name__)

MAX_KEYFRAMES = 12  # hard cap: a long video with many scene changes still gets bounded captioning cost


@dataclass
class VideoIngestResult:
    transcript: str = ""
    frame_captions: List[str] = field(default_factory=list)
    summary: str = ""
    errors: List[str] = field(default_factory=list)
    dropped_frame_count: int = 0

    @property
    def ok(self) -> bool:
        return not self.errors


def _run_ffmpeg(args: List[str], timeout: float = 300.0) -> None:
    try:
        result = subprocess.run(["ffmpeg", "-y", *args], capture_output=True, text=True, timeout=timeout)
    except FileNotFoundError as exc:
        raise RuntimeError(
            "video processing requires 'ffmpeg' binary in system PATH (install via 'sudo apt install ffmpeg' or 'brew install ffmpeg')"
        ) from exc
    if result.returncode != 0:
        raise RuntimeError(f"ffmpeg failed: {result.stderr.strip()[:500]}")


def extract_audio_wav(video_bytes: bytes) -> bytes:
    with tempfile.TemporaryDirectory() as tmp:
        in_path = Path(tmp) / "in.mp4"
        out_path = Path(tmp) / "out.wav"
        in_path.write_bytes(video_bytes)
        _run_ffmpeg(["-i", str(in_path), "-vn", "-ar", "16000", "-ac", "1", "-f", "wav", str(out_path)])
        return out_path.read_bytes()


def extract_keyframes(video_bytes: bytes, scene_threshold: float = 0.4, max_frames: int = MAX_KEYFRAMES) -> tuple[List[bytes], int]:
    """Extracts scene-change frames (never raw 1fps -- unbounded on long video).

    Returns `(frames, dropped_count)`: `dropped_count` is how many detected
    scene-change frames were discarded past `max_frames`, so callers can
    report the cap instead of silently truncating.
    """
    with tempfile.TemporaryDirectory() as tmp:
        in_path = Path(tmp) / "in.mp4"
        in_path.write_bytes(video_bytes)
        pattern = Path(tmp) / "frame_%04d.png"
        _run_ffmpeg(
            ["-i", str(in_path), "-vf", f"select='gt(scene,{scene_threshold})'", "-vsync", "vfr", str(pattern)]
        )
        frame_paths = sorted(Path(tmp).glob("frame_*.png"))
        frames = [p.read_bytes() for p in frame_paths[:max_frames]]
        dropped = max(0, len(frame_paths) - max_frames)
        return frames, dropped


class VideoIngestor:
    def __init__(
        self,
        settings: Optional[Settings] = None,
        attachment_ingestor: Optional[AttachmentIngestor] = None,
        vision_driver: Optional[OllamaDriver] = None,
        text_summarizer=None,
    ) -> None:
        self._settings = settings or get_settings()
        self._attachment_ingestor = attachment_ingestor or AttachmentIngestor(settings=self._settings)
        self._vision_driver = vision_driver or OllamaDriver(base_url=self._settings.ollama_base_url)
        # Injected in tests / by the job runner: takes the assembled
        # transcript+captions text and returns a fused summary. Kept as a
        # plain callable rather than importing Executor here, to avoid a
        # video_ingest -> executor -> ... import cycle.
        self._text_summarizer = text_summarizer

    def process(self, video_bytes: bytes, filename: str) -> VideoIngestResult:
        cleaned_video, meta_stats = sanitize_mp4(video_bytes)
        if meta_stats.removed_count:
            logger.info("video %r: stripped metadata atoms %s", filename, meta_stats.removed_chunks)

        result = VideoIngestResult()

        try:
            audio_wav = extract_audio_wav(cleaned_video)
        except Exception as exc:
            result.errors.append(f"audio extraction failed: {exc}")
            audio_wav = b""

        if audio_wav:
            audio_attachment = Attachment(
                filename=f"{filename}.wav", mime_type="audio/wav", data_base64=base64.b64encode(audio_wav).decode()
            )
            ingested = self._attachment_ingestor.ingest([audio_attachment])
            if ingested.ok:
                result.transcript = ingested.text_context
                logger.info("video %r: transcript ready (%d chars)", filename, len(result.transcript))
            else:
                result.errors.extend(ingested.errors)

        try:
            frames, dropped = extract_keyframes(cleaned_video)
            result.dropped_frame_count = dropped
            if dropped:
                logger.info("video %r: %d scene-change frames dropped past the %d-frame cap", filename, dropped, MAX_KEYFRAMES)
        except Exception as exc:
            result.errors.append(f"frame extraction failed: {exc}")
            frames = []

        for index, frame_bytes in enumerate(frames):
            data_uri = f"data:image/png;base64,{base64.b64encode(frame_bytes).decode()}"
            caption_result = self._vision_driver.run(
                self._settings.vision_model, "Describe this video frame in one concise sentence.", images=[data_uri]
            )
            if caption_result.success:
                result.frame_captions.append(f"Frame {index + 1}: {caption_result.response_text.strip()}")
            else:
                logger.warning("video %r: frame %d captioning failed: %s", filename, index, caption_result.error_message)
            logger.info("video %r: captioned frame %d/%d", filename, index + 1, len(frames))

        if not result.transcript and not result.frame_captions:
            result.errors.append("could not extract any usable content from the video (no audio, no frames)")
            return result

        fusion_input = _build_fusion_prompt(result.transcript, result.frame_captions)
        if self._text_summarizer is not None:
            try:
                result.summary = self._text_summarizer(fusion_input)
                logger.info("video %r: summary ready", filename)
            except Exception as exc:
                result.errors.append(f"summary fusion failed: {exc}")
        else:
            result.summary = fusion_input  # no summarizer wired: fall back to the raw assembled context

        return result


def _build_fusion_prompt(transcript: str, frame_captions: List[str]) -> str:
    parts = []
    if transcript:
        parts.append(f"Audio transcript:\n{transcript}")
    if frame_captions:
        parts.append("Video frame descriptions (in order):\n" + "\n".join(frame_captions))
    return "\n\n".join(parts)

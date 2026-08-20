from __future__ import annotations

import shutil
import subprocess

import pytest

from lib.engine.attachments import IngestedAttachments
from lib.engine.drivers.base import DriverResult
from lib.engine.video_ingest import VideoIngestor, extract_audio_wav, extract_keyframes

_HAS_FFMPEG = shutil.which("ffmpeg") is not None
requires_ffmpeg = pytest.mark.skipif(not _HAS_FFMPEG, reason="ffmpeg not installed on this machine")


def _make_synthetic_video(tmp_path, segments: int = 2) -> bytes:
    """Builds a tiny video with `segments` hard color cuts + a silent audio track."""
    colors = ["red", "blue", "green", "yellow"][:segments]
    out_path = tmp_path / "synthetic.mp4"
    args = ["ffmpeg", "-y"]
    for color in colors:
        args += ["-f", "lavfi", "-i", f"color=c={color}:s=64x64:d=1"]
    args += ["-f", "lavfi", "-i", "anullsrc=r=16000:cl=mono"]
    inputs = "".join(f"[{i}:v]" for i in range(segments))
    args += [
        "-filter_complex", f"{inputs}concat=n={segments}:v=1:a=0[v]",
        "-map", "[v]", "-map", f"{segments}:a", "-shortest", "-t", str(segments),
        "-pix_fmt", "yuv420p", str(out_path),
    ]
    subprocess.run(args, capture_output=True, check=True)
    return out_path.read_bytes()


class _FakeAttachmentIngestor:
    def __init__(self, transcript: str = "someone said hello") -> None:
        self._transcript = transcript

    def ingest(self, attachments, *, model_is_vision_capable: bool = False):
        return IngestedAttachments(text_context=self._transcript)


class _FakeVisionDriver:
    def __init__(self) -> None:
        self.calls = 0

    def run(self, model, prompt, images=None):
        self.calls += 1
        return DriverResult(success=True, response_text=f"a frame ({self.calls})", input_tokens=0, output_tokens=0, latency_ms=0)


@requires_ffmpeg
def test_extract_audio_wav_produces_valid_wav_bytes(tmp_path):
    video_bytes = _make_synthetic_video(tmp_path, segments=2)

    wav_bytes = extract_audio_wav(video_bytes)

    assert wav_bytes[:4] == b"RIFF"
    assert wav_bytes[8:12] == b"WAVE"


@requires_ffmpeg
def test_extract_keyframes_detects_scene_cuts(tmp_path):
    video_bytes = _make_synthetic_video(tmp_path, segments=3)

    frames, dropped = extract_keyframes(video_bytes, scene_threshold=0.1)

    assert len(frames) >= 1
    assert all(f[:8] == b"\x89PNG\r\n\x1a\n" for f in frames)


@requires_ffmpeg
def test_extract_keyframes_caps_and_reports_dropped_count(tmp_path):
    video_bytes = _make_synthetic_video(tmp_path, segments=4)

    frames, dropped = extract_keyframes(video_bytes, scene_threshold=0.1, max_frames=1)

    assert len(frames) <= 1
    # 4 hard color cuts at a low threshold detect more than 1 scene change;
    # capping to 1 must report the rest as dropped, never silently discard them.
    assert dropped >= 1


@requires_ffmpeg
def test_video_ingestor_process_assembles_transcript_captions_and_summary(tmp_path):
    video_bytes = _make_synthetic_video(tmp_path, segments=2)
    vision_driver = _FakeVisionDriver()
    ingestor = VideoIngestor(
        attachment_ingestor=_FakeAttachmentIngestor("someone said hello"),
        vision_driver=vision_driver,
        text_summarizer=lambda text: f"SUMMARY: {text[:20]}",
    )

    result = ingestor.process(video_bytes, filename="clip.mp4")

    assert result.ok
    assert result.transcript == "someone said hello"
    assert len(result.frame_captions) >= 1
    assert result.summary.startswith("SUMMARY:")
    assert vision_driver.calls == len(result.frame_captions)


def test_video_ingestor_errors_when_nothing_could_be_extracted():
    ingestor = VideoIngestor(
        attachment_ingestor=_FakeAttachmentIngestor(""),
        vision_driver=_FakeVisionDriver(),
    )
    # Not a real video: ffmpeg extraction fails for both audio and frames,
    # so there is nothing to summarize -- must error, not return an empty summary.
    result = ingestor.process(b"not a real video", filename="broken.mp4")

    assert not result.ok
    assert result.summary == ""

from __future__ import annotations

import shutil
import struct
import subprocess

import pytest

from lib.core.media_sanitize import (
    classify_and_sanitize,
    sanitize_jpeg,
    sanitize_mp3,
    sanitize_mp4,
    sanitize_png,
    sanitize_wav,
)

_HAS_FFMPEG = shutil.which("ffmpeg") is not None and shutil.which("ffprobe") is not None
requires_ffmpeg = pytest.mark.skipif(not _HAS_FFMPEG, reason="ffmpeg/ffprobe not installed on this machine")


def _png_chunk(chunk_type: bytes, data: bytes) -> bytes:
    return struct.pack(">I", len(data)) + chunk_type + data + b"\x00\x00\x00\x00"  # dummy CRC, never validated


def _build_png(metadata: bool) -> bytes:
    out = b"\x89PNG\r\n\x1a\n"
    out += _png_chunk(b"IHDR", b"\x00" * 13)
    if metadata:
        out += _png_chunk(b"tEXt", b"Author\x00someone")
        out += _png_chunk(b"eXIf", b"fake-exif-blob")
    out += _png_chunk(b"IDAT", b"fake-pixels")
    out += _png_chunk(b"IEND", b"")
    return out


def test_sanitize_png_strips_text_and_exif_keeps_pixels():
    cleaned, stats = sanitize_png(_build_png(metadata=True))

    assert b"someone" not in cleaned
    assert b"fake-exif-blob" not in cleaned
    assert b"fake-pixels" in cleaned  # IDAT untouched
    assert stats.removed_chunks.get("tEXt") == 1
    assert stats.removed_chunks.get("eXIf") == 1


def test_sanitize_png_without_metadata_is_unchanged_besides_structure():
    clean_input = _build_png(metadata=False)
    cleaned, stats = sanitize_png(clean_input)

    assert cleaned == clean_input
    assert stats.removed_count == 0


def _jpeg_segment(marker: int, data: bytes) -> bytes:
    return bytes([0xFF, marker]) + struct.pack(">H", len(data) + 2) + data


def _build_jpeg(metadata: bool) -> bytes:
    out = b"\xff\xd8"  # SOI
    if metadata:
        out += _jpeg_segment(0xE1, b"Exif\x00\x00fake-exif")  # APP1/EXIF
    out += _jpeg_segment(0xDA, b"\x00\x00")  # SOS (minimal)
    out += b"scan-bytes-here"
    out += b"\xff\xd9"  # EOI
    return out


def test_sanitize_jpeg_strips_exif_app_segment():
    cleaned, stats = sanitize_jpeg(_build_jpeg(metadata=True))

    assert b"fake-exif" not in cleaned
    assert b"scan-bytes-here" in cleaned  # compressed scan data untouched
    assert stats.removed_chunks.get("APP1") == 1


def test_sanitize_jpeg_without_metadata_is_unchanged():
    clean_input = _build_jpeg(metadata=False)
    cleaned, stats = sanitize_jpeg(clean_input)

    assert cleaned == clean_input
    assert stats.removed_count == 0


def _riff_chunk(chunk_id: bytes, data: bytes) -> bytes:
    padded = data + (b"\x00" if len(data) % 2 else b"")
    return chunk_id + struct.pack("<I", len(data)) + padded


def _build_wav(metadata: bool) -> bytes:
    fmt = _riff_chunk(b"fmt ", b"\x01\x00\x02\x00" + b"\x00" * 12)
    data_chunk = _riff_chunk(b"data", b"fake-pcm-samples")
    body = fmt
    if metadata:
        body += _riff_chunk(b"LIST", b"INFOIART someone")
    body += data_chunk
    return b"RIFF" + struct.pack("<I", 4 + len(body)) + b"WAVE" + body


def test_sanitize_wav_strips_list_info_keeps_audio_data():
    cleaned, stats = sanitize_wav(_build_wav(metadata=True))

    assert b"someone" not in cleaned
    assert b"fake-pcm-samples" in cleaned
    assert stats.removed_chunks.get("LIST") == 1
    # RIFF size field stays consistent with the new (shorter) body.
    riff_size = struct.unpack("<I", cleaned[4:8])[0]
    assert riff_size == len(cleaned) - 8


def test_sanitize_mp3_strips_id3v2_and_id3v1():
    tag_body = b"fake-id3-frames-with-author-name"
    size = len(tag_body)
    synchsafe = bytes([(size >> 21) & 0x7F, (size >> 14) & 0x7F, (size >> 7) & 0x7F, size & 0x7F])
    id3v2 = b"ID3" + b"\x03\x00" + b"\x00" + synchsafe + tag_body
    audio_frames = b"\xff\xfb" + b"fake-mp3-audio-frames"
    id3v1 = b"TAG" + b"\x00" * 125
    raw = id3v2 + audio_frames + id3v1

    cleaned, stats = sanitize_mp3(raw)

    assert cleaned == audio_frames
    assert stats.removed_chunks.get("ID3v2") == 1
    assert stats.removed_chunks.get("ID3v1") == 1


def _mp4_atom(atom_type: bytes, data: bytes) -> bytes:
    return struct.pack(">I", 8 + len(data)) + atom_type + data


def _build_mp4(metadata: bool) -> bytes:
    ftyp = _mp4_atom(b"ftyp", b"isom" + b"\x00" * 8)
    mvhd = _mp4_atom(b"mvhd", b"\x00" * 20)
    moov_children = mvhd
    if metadata:
        moov_children += _mp4_atom(b"udta", b"fake-author-metadata")
    moov = _mp4_atom(b"moov", moov_children)
    mdat = _mp4_atom(b"mdat", b"fake-video-payload")
    return ftyp + moov + mdat


def test_sanitize_mp4_strips_udta_keeps_mdat_and_mvhd():
    cleaned, stats = sanitize_mp4(_build_mp4(metadata=True))

    assert b"fake-author-metadata" not in cleaned
    assert b"fake-video-payload" in cleaned  # mdat untouched
    assert stats.removed_chunks.get("udta") == 1


def test_sanitize_mp4_without_metadata_is_structurally_equivalent():
    clean_input = _build_mp4(metadata=False)
    cleaned, stats = sanitize_mp4(clean_input)

    assert cleaned == clean_input
    assert stats.removed_count == 0


# --- real-ffmpeg regression: stco/co64 offset correctness ------------------
#
# The hand-rolled unit tests above use synthetic atoms with dummy chunk
# offsets, so they can't catch a corrupted stco/co64 table (moov-shrink
# offset math gone wrong silently produces garbage NAL units, not a clean
# parse failure). These build a real, ffmpeg-encoded video with an actual
# sample layout and verify it still decodes -- and still contains the
# right amount of audio -- after sanitizing.


def _make_real_mp4(tmp_path, faststart: bool) -> bytes:
    out_path = tmp_path / ("faststart.mp4" if faststart else "normal.mp4")
    args = [
        "ffmpeg", "-y",
        "-f", "lavfi", "-i", "color=c=red:s=64x64:d=1",
        "-f", "lavfi", "-i", "anullsrc=r=16000:cl=mono",
        "-shortest", "-t", "1", "-pix_fmt", "yuv420p",
    ]
    if faststart:
        args += ["-movflags", "+faststart"]
    args += [str(out_path)]
    subprocess.run(args, capture_output=True, check=True)
    return out_path.read_bytes()


@requires_ffmpeg
def test_sanitize_mp4_moov_after_mdat_stays_decodable(tmp_path):
    raw = _make_real_mp4(tmp_path, faststart=False)
    cleaned, stats = sanitize_mp4(raw)
    assert stats.removed_count > 0  # ffmpeg always writes a udta/encoder tag here

    cleaned_path = tmp_path / "cleaned.mp4"
    cleaned_path.write_bytes(cleaned)
    wav_path = tmp_path / "out.wav"
    subprocess.run(["ffmpeg", "-v", "error", "-y", "-i", str(cleaned_path), "-f", "wav", str(wav_path)], check=True)

    assert wav_path.stat().st_size > 44  # more than just a WAV header: real audio decoded


@requires_ffmpeg
def test_sanitize_mp4_moov_before_mdat_stays_decodable(tmp_path):
    raw = _make_real_mp4(tmp_path, faststart=True)
    cleaned, stats = sanitize_mp4(raw)
    assert stats.removed_count > 0

    cleaned_path = tmp_path / "cleaned.mp4"
    cleaned_path.write_bytes(cleaned)
    wav_path = tmp_path / "out.wav"
    subprocess.run(["ffmpeg", "-v", "error", "-y", "-i", str(cleaned_path), "-f", "wav", str(wav_path)], check=True)

    assert wav_path.stat().st_size > 44


def test_classify_and_sanitize_routes_by_mime_type():
    cleaned, stats = classify_and_sanitize(_build_png(metadata=True), "image/png")
    assert stats.removed_count == 2

    unchanged, empty_stats = classify_and_sanitize(b"plain bytes", "application/octet-stream")
    assert unchanged == b"plain bytes"
    assert empty_stats.removed_count == 0

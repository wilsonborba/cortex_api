"""Deterministic metadata strip for image / audio / video containers.

Structural, format-aware removal of provenance/identifying metadata (EXIF,
XMP, ID3, RIFF INFO, MP4 `udta`/`meta` atoms) -- never touches pixel/audio
sample data. Pixel-domain or audio-domain watermarks (SynthID-class,
steganographic signal watermarks) are out of scope: this only strips
container-level metadata, the same "Layer A for files" idea as
`text_sanitize.py` is for text.
"""

from __future__ import annotations

import struct
from dataclasses import dataclass, field
from typing import Optional

PNG_MAGIC = b"\x89PNG\r\n\x1a\n"
JPEG_MAGIC = b"\xff\xd8"

# PNG ancillary chunks that carry metadata (text, timestamps, ICC/EXIF).
# Critical chunks (IHDR/PLTE/IDAT/IEND/...) are always kept.
_PNG_METADATA_CHUNKS = frozenset({b"tEXt", b"iTXt", b"zTXt", b"eXIf", b"tIME", b"iCCP", b"pHYs"})

# JPEG APPn markers that carry metadata: APP1 (EXIF/XMP), APP2 (ICC), APP11
# (JUMBF/C2PA), APP13 (Photoshop/IPTC), plus COM (comment). SOI/EOI/SOF/SOS
# and the scan data itself are never touched.
_JPEG_METADATA_MARKERS = frozenset({0xE1, 0xE2, 0xEB, 0xED, 0xFE})
_JPEG_SOS = 0xDA  # start of scan: everything after this is compressed image data


@dataclass
class MediaSanitizeStats:
    removed_chunks: dict[str, int] = field(default_factory=dict)

    @property
    def removed_count(self) -> int:
        return sum(self.removed_chunks.values())


def _record(stats: MediaSanitizeStats, label: str) -> None:
    stats.removed_chunks[label] = stats.removed_chunks.get(label, 0) + 1


def sanitize_png(data: bytes) -> tuple[bytes, MediaSanitizeStats]:
    stats = MediaSanitizeStats()
    if not data.startswith(PNG_MAGIC):
        return data, stats

    out = bytearray(PNG_MAGIC)
    pos = len(PNG_MAGIC)
    while pos + 8 <= len(data):
        length = struct.unpack(">I", data[pos : pos + 4])[0]
        chunk_type = data[pos + 4 : pos + 8]
        chunk_end = pos + 8 + length + 4  # length + type + data + crc
        if chunk_end > len(data):
            break  # truncated/corrupt tail: stop rather than emit garbage
        if chunk_type in _PNG_METADATA_CHUNKS:
            _record(stats, chunk_type.decode("ascii", errors="replace"))
        else:
            out += data[pos:chunk_end]
        pos = chunk_end
    return bytes(out), stats


def sanitize_jpeg(data: bytes) -> tuple[bytes, MediaSanitizeStats]:
    stats = MediaSanitizeStats()
    if not data.startswith(JPEG_MAGIC):
        return data, stats

    out = bytearray(data[:2])  # SOI
    pos = 2
    while pos + 4 <= len(data):
        if data[pos] != 0xFF:
            break  # not a marker where one was expected: stop, keep what's safe
        marker = data[pos + 1]
        if marker == _JPEG_SOS:
            out += data[pos:]  # scan data to EOI: copy verbatim, untouched
            break
        if marker in (0xD8, 0xD9):  # SOI/EOI have no length field
            out += data[pos : pos + 2]
            pos += 2
            continue
        seg_len = struct.unpack(">H", data[pos + 2 : pos + 4])[0]
        seg_end = pos + 2 + seg_len
        if seg_end > len(data):
            break
        if marker in _JPEG_METADATA_MARKERS:
            _record(stats, f"APP{marker - 0xE0}" if 0xE0 <= marker <= 0xEF else f"marker_{marker:02X}")
        else:
            out += data[pos:seg_end]
        pos = seg_end
    return bytes(out), stats


def sanitize_wav(data: bytes) -> tuple[bytes, MediaSanitizeStats]:
    """Strips RIFF `LIST`/`INFO` metadata chunks from a WAV file, keeping `fmt `/`data` untouched."""
    stats = MediaSanitizeStats()
    if not (data[:4] == b"RIFF" and data[8:12] == b"WAVE"):
        return data, stats

    out = bytearray(data[:12])
    pos = 12
    while pos + 8 <= len(data):
        chunk_id = data[pos : pos + 4]
        size = struct.unpack("<I", data[pos + 4 : pos + 8])[0]
        padded_size = size + (size % 2)
        chunk_end = pos + 8 + padded_size
        if chunk_end > len(data):
            break
        if chunk_id in (b"LIST", b"id3 ", b"ID3 "):
            _record(stats, chunk_id.decode("ascii", errors="replace").strip())
        else:
            out += data[pos:chunk_end]
        pos = chunk_end

    riff_size = len(out) - 8
    out[4:8] = struct.pack("<I", riff_size)
    return bytes(out), stats


def sanitize_mp3(data: bytes) -> tuple[bytes, MediaSanitizeStats]:
    """Strips a leading ID3v2 tag and a trailing ID3v1 tag; audio frames untouched."""
    stats = MediaSanitizeStats()
    out = data

    if out[:3] == b"ID3":
        tag_size = _id3v2_size(out[6:10])
        header_len = 10 + tag_size
        if header_len <= len(out):
            _record(stats, "ID3v2")
            out = out[header_len:]

    if len(out) >= 128 and out[-128:-125] == b"TAG":
        _record(stats, "ID3v1")
        out = out[:-128]

    return out, stats


def _id3v2_size(size_bytes: bytes) -> int:
    """ID3v2 sizes are encoded as 4 synchsafe bytes (7 bits each)."""
    b0, b1, b2, b3 = size_bytes
    return (b0 << 21) | (b1 << 14) | (b2 << 7) | b3


# MP4/MOV atoms that carry metadata but never touch `mdat` (the media
# payload) or `moov`'s structural boxes (`trak`, `mvhd`, ...).
_MP4_METADATA_ATOMS = frozenset({b"udta", b"meta", b"\xa9too", b"free", b"skip"})


def sanitize_mp4(data: bytes) -> tuple[bytes, MediaSanitizeStats]:
    """Strips top-level and `moov`-nested metadata atoms from an MP4/MOV container.

    `stco`/`co64` (the sample chunk-offset tables, nested under
    `moov/trak/mdia/minf/stbl`) hold *absolute file offsets* into `mdat`.
    Removing or shrinking anything positioned *before* `mdat` in the file
    shifts those offsets; getting this wrong silently corrupts every
    sample. Two layouts exist in the wild and both are handled correctly,
    never by guessing:

    - `moov` after `mdat` (ffmpeg's own default, no `faststart`): metadata
      inside `moov` is stripped freely -- it's all after `mdat`, so no
      offset shifts. A top-level `free`/`skip` *before* `mdat` is left
      alone instead: removing it would shift `mdat` itself, and top-level
      padding is not worth that risk for something this rarely load-bearing.
    - `moov` before `mdat` (e.g. `faststart`): shrinking `moov` is
      compensated by subtracting the exact byte delta from every
      `stco`/`co64` entry, so `mdat`'s sample offsets stay correct.
    """
    stats = MediaSanitizeStats()
    mdat_start = _find_mdat_start(data)
    out, _ = _strip_mp4_atoms(data, stats, depth=0, mdat_start=mdat_start)
    return out, stats


def _find_mdat_start(data: bytes) -> Optional[int]:
    pos = 0
    while pos + 8 <= len(data):
        size = struct.unpack(">I", data[pos : pos + 4])[0]
        atom_type = data[pos + 4 : pos + 8]
        if atom_type == b"mdat":
            return pos
        if size < 8:
            break
        pos += size
    return None


def _strip_mp4_atoms(data: bytes, stats: MediaSanitizeStats, depth: int, mdat_start: Optional[int]) -> tuple[bytes, bool]:
    out = bytearray()
    pos = 0
    changed = False
    while pos + 8 <= len(data):
        size = struct.unpack(">I", data[pos : pos + 4])[0]
        atom_type = data[pos + 4 : pos + 8]
        if size == 0:
            out += data[pos:]  # size 0 means "to end of file": copy the rest verbatim
            break
        if size == 1:
            break  # 64-bit atom size extension: not handled, stop rather than misparse
        atom_end = pos + size
        if atom_end > len(data):
            break

        # depth == 0 means `pos` is a real absolute file offset, so it can
        # be compared against `mdat_start`; nested calls pass mdat_start=None
        # and rely on the moov-shrink delta correction below instead.
        before_mdat = depth == 0 and mdat_start is not None and pos < mdat_start

        if atom_type in _MP4_METADATA_ATOMS and not before_mdat:
            _record(stats, atom_type.decode("ascii", errors="replace"))
            changed = True
        elif atom_type == b"moov" and depth < 4:
            original_moov_len = size
            inner, inner_changed = _strip_mp4_atoms(data[pos + 8 : atom_end], stats, depth + 1, mdat_start=None)
            new_moov_len = 8 + len(inner)
            if inner_changed and before_mdat:
                delta = original_moov_len - new_moov_len
                inner = _adjust_chunk_offsets(inner, delta)
            out += struct.pack(">I", new_moov_len) + atom_type + inner
            changed = changed or inner_changed
        else:
            out += data[pos:atom_end]
        pos = atom_end

    return bytes(out), changed


def _adjust_chunk_offsets(moov_body: bytes, delta: int) -> bytes:
    """Walks every box inside `moov_body` and shifts `stco`/`co64` sample
    offsets by `-delta` (moov shrank by `delta` bytes, so every absolute
    offset into `mdat` needs to move back by the same amount)."""
    if delta == 0:
        return moov_body

    out = bytearray()
    pos = 0
    while pos + 8 <= len(moov_body):
        size = struct.unpack(">I", moov_body[pos : pos + 4])[0]
        box_type = moov_body[pos + 4 : pos + 8]
        if size < 8 or pos + size > len(moov_body):
            out += moov_body[pos:]
            break
        box = moov_body[pos : pos + size]

        if box_type == b"stco":
            box = _rewrite_offset_table(box, entry_size=4, fmt=">I", delta=delta)
        elif box_type == b"co64":
            box = _rewrite_offset_table(box, entry_size=8, fmt=">Q", delta=delta)
        elif box_type in (b"trak", b"mdia", b"minf", b"stbl"):
            header, body = box[:8], box[8:]
            box = header + _adjust_chunk_offsets(body, delta)

        out += box
        pos += size

    return bytes(out)


def _rewrite_offset_table(box: bytes, entry_size: int, fmt: str, delta: int) -> bytes:
    header = box[:8]  # size + type
    version_flags = box[8:12]
    entry_count = struct.unpack(">I", box[12:16])[0]
    entries_start = 16
    out = bytearray(header + version_flags + box[12:16])
    for i in range(entry_count):
        start = entries_start + i * entry_size
        (offset,) = struct.unpack(fmt, box[start : start + entry_size])
        out += struct.pack(fmt, max(0, offset - delta))
    return bytes(out)


def classify_and_sanitize(data: bytes, mime_type: str) -> tuple[bytes, MediaSanitizeStats]:
    if mime_type == "image/png":
        return sanitize_png(data)
    if mime_type in ("image/jpeg", "image/jpg"):
        return sanitize_jpeg(data)
    if mime_type == "audio/wav" or mime_type == "audio/x-wav":
        return sanitize_wav(data)
    if mime_type == "audio/mpeg" or mime_type == "audio/mp3":
        return sanitize_mp3(data)
    if mime_type in ("video/mp4", "video/quicktime"):
        return sanitize_mp4(data)
    return data, MediaSanitizeStats()

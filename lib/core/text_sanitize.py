"""Deterministic hygiene pass over provider-returned text.

Strips invisible/format Unicode codepoints (zero-width, bidi controls, tag
characters, noncharacters, private-use) that some tools use as an
edit-based provenance/watermark channel, and normalizes exotic space
homoglyphs to a plain space. Deliberately does not touch statistical
(token-sampling) watermarks -- that is not decidable deterministically.
"""

from __future__ import annotations

from dataclasses import dataclass, field

# Zero-width/format controls that carry no legitimate meaning floating
# between ASCII text -- the classic edit-based watermark channel.
_STRIP_CODEPOINTS: frozenset[int] = frozenset(
    {
        0x00AD,  # soft hyphen
        0x061C,  # Arabic letter mark
        0x200B,  # zero width space
        0x200C,  # zero width non-joiner
        0x200D,  # zero width joiner
        0x200E,  # left-to-right mark
        0x200F,  # right-to-left mark
        0x202A,  # LRE
        0x202B,  # RLE
        0x202C,  # PDF
        0x202D,  # LRO
        0x202E,  # RLO
        0x2060,  # word joiner
        0x2066,  # LRI
        0x2067,  # RLI
        0x2068,  # FSI
        0x2069,  # PDI
        0xFEFF,  # BOM / zero width no-break space
    }
)

# Unicode tag characters (used by some steganographic emoji-flag/stego
# schemes) and their close variation-selector-supplement cousins.
_TAG_RANGE = range(0xE0001, 0xE0080)
_VS_SUPPLEMENT = range(0xFE00, 0xFE10)

# Private-use areas: no portable meaning in interchange text.
_PRIVATE_USE_RANGES: tuple[range, ...] = (
    range(0xE000, 0xF900),
    range(0xF0000, 0x100000),
    range(0x100000, 0x110000),
)

# Emoji base ranges: ZWJ/variation-selector adjacency to one of these keeps
# the char legitimate (family emoji, dingbat + VS16, etc).
_EMOJI_BASE_RANGES: tuple[range, ...] = (
    range(0x1F000, 0x1FB00),
    range(0x2190, 0x2600),
    range(0x2600, 0x27C0),
    range(0x2B00, 0x2C00),
)
_EMOJI_BASE_SINGLES: frozenset[int] = frozenset({0x00A9, 0x00AE, 0x2122, 0x3030, 0x303D, 0x3297, 0x3299})

SPACE_HOMOGLYPHS: dict[int, str] = {
    0x00A0: " ",  # no-break space
    0x1680: " ",  # Ogham space mark
    0x2000: " ",
    0x2001: " ",
    0x2002: " ",
    0x2003: " ",
    0x2004: " ",
    0x2005: " ",
    0x2006: " ",
    0x2007: " ",  # figure space
    0x2008: " ",
    0x2009: " ",
    0x200A: " ",
    0x202F: " ",  # narrow no-break space
    0x205F: " ",
    0x3000: " ",  # ideographic space
}


def _is_noncharacter(cp: int) -> bool:
    return 0xFDD0 <= cp <= 0xFDEF or (cp & 0xFFFE) == 0xFFFE


def _is_private_use(cp: int) -> bool:
    return any(cp in r for r in _PRIVATE_USE_RANGES)


def _is_emoji_base(cp: int | None) -> bool:
    if cp is None:
        return False
    if cp in _EMOJI_BASE_SINGLES:
        return True
    return any(cp in r for r in _EMOJI_BASE_RANGES)


def _is_emoji_glue(cp: int) -> bool:
    return cp == 0x200D or cp in _VS_SUPPLEMENT


def _char_label(ch: str) -> str:
    return f"U+{ord(ch):04X}"


@dataclass
class SanitizeStats:
    removed: dict[str, int] = field(default_factory=dict)
    replaced: dict[str, int] = field(default_factory=dict)

    @property
    def removed_count(self) -> int:
        return sum(self.removed.values())

    @property
    def replaced_count(self) -> int:
        return sum(self.replaced.values())


def sanitize_text(text: str, *, normalize_spaces: bool = True) -> tuple[str, SanitizeStats]:
    """Strip invisible/format Unicode carriers and normalize exotic spaces.

    Preserves emoji glue (ZWJ / variation selectors) when adjacent to an
    emoji-range codepoint, so real emoji sequences survive untouched.
    """
    stats = SanitizeStats()
    out: list[str] = []

    for i, ch in enumerate(text):
        cp = ord(ch)

        if _is_emoji_glue(cp):
            prev_cp = ord(text[i - 1]) if i > 0 else None
            next_cp = ord(text[i + 1]) if i + 1 < len(text) else None
            if _is_emoji_base(prev_cp) or _is_emoji_base(next_cp):
                out.append(ch)
                continue

        if (
            cp in _STRIP_CODEPOINTS
            or cp in _TAG_RANGE
            or cp in _VS_SUPPLEMENT
            or _is_noncharacter(cp)
            or _is_private_use(cp)
        ):
            stats.removed[_char_label(ch)] = stats.removed.get(_char_label(ch), 0) + 1
            continue

        if normalize_spaces and cp in SPACE_HOMOGLYPHS:
            stats.replaced[_char_label(ch)] = stats.replaced.get(_char_label(ch), 0) + 1
            out.append(SPACE_HOMOGLYPHS[cp])
            continue

        out.append(ch)

    return "".join(out), stats

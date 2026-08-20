from __future__ import annotations

from lib.core.text_sanitize import sanitize_text


def test_strips_isolated_zero_width_chars():
    text = "Hello​World‌‍!"
    cleaned, stats = sanitize_text(text)
    assert cleaned == "HelloWorld!"
    assert stats.removed_count == 3


def test_strips_bom_and_bidi_overrides():
    text = "﻿Secret‮‬alarm"
    cleaned, stats = sanitize_text(text)
    assert cleaned == "Secretalarm"
    assert stats.removed_count == 3


def test_strips_tag_characters_and_noncharacter():
    text = "flag" + chr(0xE0031) + chr(0xE0032) + "﷐" + "end"
    cleaned, stats = sanitize_text(text)
    assert cleaned == "flagend"
    assert stats.removed_count == 3


def test_normalizes_exotic_spaces_by_default():
    text = "a b c　d"
    cleaned, stats = sanitize_text(text)
    assert cleaned == "a b c d"
    assert stats.replaced_count == 3


def test_preserves_exotic_spaces_when_disabled():
    text = "a b"
    cleaned, stats = sanitize_text(text, normalize_spaces=False)
    assert cleaned == text
    assert stats.replaced_count == 0


def test_preserves_emoji_zwj_sequence():
    family = "\U0001F468‍\U0001F469‍\U0001F467"  # man-woman-girl family
    cleaned, stats = sanitize_text(family)
    assert cleaned == family
    assert stats.removed_count == 0


def test_preserves_variation_selector_after_dingbat():
    warning = "⚠️"  # warning sign + VS16
    cleaned, stats = sanitize_text(warning)
    assert cleaned == warning
    assert stats.removed_count == 0


def test_clean_text_passes_through_unchanged():
    text = "Plain ASCII text, nothing suspicious here."
    cleaned, stats = sanitize_text(text)
    assert cleaned == text
    assert stats.removed_count == 0
    assert stats.replaced_count == 0


def test_idempotent():
    text = "Hello​World there﻿!"
    once, _ = sanitize_text(text)
    twice, _ = sanitize_text(once)
    assert once == twice

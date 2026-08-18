from __future__ import annotations

import pytest

from lib.engine.classifier import classify_complexity, extract_tier_directive


@pytest.mark.parametrize(
    "prompt,expected_tier,expected_rest",
    [
        ("/t3 Refactor this function", 3, "Refactor this function"),
        ("[T4] Review this API for security issues", 4, "Review this API for security issues"),
        ("/t0 hi", 0, "hi"),
        ("no directive here", None, "no directive here"),
        ("  /t5   decompose this problem", 5, "decompose this problem"),
    ],
)
def test_extract_tier_directive(prompt, expected_tier, expected_rest):
    tier, rest = extract_tier_directive(prompt)
    assert tier == expected_tier
    assert rest == expected_rest


def test_extract_tier_directive_ignores_out_of_range_values():
    tier, rest = extract_tier_directive("/t9 hello")
    assert tier is None
    assert rest == "/t9 hello"


@pytest.mark.parametrize(
    "prompt,expected_tier",
    [
        ("hi", 0),
        ("what's the weather like", 0),
        ("Can you explain the difference between a list and a tuple in Python, with examples?", 1),
        ("Refactor this function to remove duplication and improve readability", 3),
    ],
)
def test_classify_complexity(prompt, expected_tier):
    assert classify_complexity(prompt) == expected_tier


def test_classify_complexity_long_heavy_prompt_is_tier_four():
    prompt = (
        "Design the complete architecture for a distributed rate limiter with strong "
        "consistency guarantees, including every failure mode, the exact algorithm used "
        "for coordination between nodes, how it degrades gracefully under network "
        "partitions, and how it should be tested under adversarial conditions"
    )
    assert len(prompt.split()) > 40
    assert classify_complexity(prompt) == 4


def test_classify_complexity_empty_prompt_is_tier_zero():
    assert classify_complexity("   ") == 0


def test_classify_complexity_detects_code_block():
    prompt = "What's wrong with this?\n```python\ndef foo():\n    pass\n```"
    assert classify_complexity(prompt) >= 3

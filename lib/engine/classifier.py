from __future__ import annotations

import re
from typing import Optional, Tuple

# Directive syntax at the very start of a prompt: `/t3 ...` or `[T4] ...`.
_DIRECTIVE_PATTERN = re.compile(r"^\s*(?:/t([0-5])\b|\[t([0-5])\])\s*", re.IGNORECASE)

_HEAVY_KEYWORDS = (
    "refactor", "refatora", "architecture", "arquitetura", "algorithm", "algoritmo",
    "optimi", "otimiz", "security", "segurança", "debug", "concurrency", "concorrência",
    "design pattern", "distributed", "distribuíd", "performance", "complexity", "complexidade",
    "prove", "proof", "prova matemática", "implement", "implementa", "vulnerabilit",
)
_LIGHT_MAX_WORDS = 12
_STANDARD_MAX_WORDS = 40


def extract_tier_directive(prompt: str) -> Tuple[Optional[int], str]:
    """Parses a leading `/t<0-5>` or `[T<0-5>]` directive off `prompt`.

    Returns `(tier_or_None, prompt_with_directive_stripped)`. The model never
    sees the directive text itself.
    """
    match = _DIRECTIVE_PATTERN.match(prompt)
    if not match:
        return None, prompt
    tier_str = match.group(1) or match.group(2)
    return int(tier_str), prompt[match.end():].lstrip()


def classify_complexity(prompt: str) -> int:
    """Heuristic T0-T5 guess for `--tier auto` / an omitted tier.

    Deliberately simple: word count plus a small keyword list, nothing
    learned. Per docs/specs.md this project's own philosophy is "rules first,
    then replace with evidence" once the telemetry this system collects
    (quality/latency per tier per task) is there to tune it on.
    """
    text = prompt.strip().lower()
    if not text:
        return 0

    word_count = len(text.split())
    has_heavy_keyword = any(keyword in text for keyword in _HEAVY_KEYWORDS)
    has_code_block = "```" in prompt or bool(re.search(r"\bdef |\bclass \w|\bfunction\b", prompt))

    if has_heavy_keyword or has_code_block:
        return 4 if word_count > _STANDARD_MAX_WORDS else 3
    if word_count <= _LIGHT_MAX_WORDS:
        return 0
    if word_count <= _STANDARD_MAX_WORDS:
        return 1
    return 2

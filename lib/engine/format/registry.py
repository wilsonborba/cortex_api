from __future__ import annotations

import json as _json
from typing import Any, Callable, Dict

from lib.engine.format.toon import encode_toon

TOON = "toon"
JSON = "json"


def encode_json(data: Any) -> str:
    return _json.dumps(data, default=str)


ENCODERS: Dict[str, Callable[[Any], str]] = {
    TOON: encode_toon,
    JSON: encode_json,
}


def register_format(name: str, encoder: Callable[[Any], str]) -> None:
    """Adds an internal communication format without a DB migration: #15's
    preference fields are plain strings, not an enum, so a new format only
    needs an encoder registered here to become usable everywhere #16
    resolves a format."""
    ENCODERS[name] = encoder


def encode(data: Any, format_name: str) -> str:
    try:
        encoder = ENCODERS[format_name]
    except KeyError as exc:
        raise ValueError(
            f"unknown context format {format_name!r}; registered: {sorted(ENCODERS)}"
        ) from exc
    return encoder(data)

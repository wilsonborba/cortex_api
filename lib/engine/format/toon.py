from __future__ import annotations

import re
from typing import Any, List, Mapping, Optional

# Grounded directly against github.com/toon-format/spec (SPEC.md, fetched
# verbatim while building this, not a secondary summary). Encoding only:
# nothing in Cortex needs to parse TOON back, it's an outbound-only format
# for prompt injection (see issue #14).

INDENT = "  "  # two spaces per depth, per every example in the spec

_UNQUOTED_KEY_RE = re.compile(r"^[A-Za-z_][A-Za-z0-9_.]*$")
_NUMERIC_RE = re.compile(r"^[+-]?[0-9]+(?:\.[0-9]+)?(?:e[+-]?[0-9]+)?$", re.IGNORECASE)
_RESERVED_LITERALS = {"true", "false", "null"}
_ESCAPES = {
    "\\": "\\\\",
    '"': '\\"',
    "\n": "\\n",
    "\r": "\\r",
    "\t": "\\t",
}
# Characters that force quoting per spec section 7.2, beyond the reserved
# literals/numeric-pattern/empty/whitespace checks below. "," is the active
# delimiter (comma is the only one this encoder supports -- tab/pipe
# delimiter modes from the spec aren't implemented, comma covers every use
# case Cortex has).
_FORCE_QUOTE_CHARS = (":", '"', "\\", "[", "]", "{", "}", ",")


def encode_toon(data: Any) -> str:
    """Encodes `data` (JSON-like dict/list/scalar values) as TOON.

    A dict at the top level is the normal case (each key becomes a line);
    a bare top-level list uses the same `[N]`/`[N]{fields}` syntax minus
    the leading key name -- that specific case is an inference from the
    keyed-array spec examples, not verified against a literal spec example,
    so prefer wrapping data in a dict at call sites where it matters.
    """
    if isinstance(data, Mapping):
        return "\n".join(_encode_object_fields(data, depth=0))
    if isinstance(data, (list, tuple)):
        return "\n".join(_encode_array(None, list(data), depth=0))
    return _encode_scalar(data)


def _encode_object_fields(obj: Mapping[str, Any], depth: int) -> List[str]:
    lines: List[str] = []
    for key, value in obj.items():
        lines.extend(_encode_field(key, value, depth))
    return lines


def _encode_field(key: str, value: Any, depth: int) -> List[str]:
    indent = INDENT * depth
    rendered_key = _render_key(key)
    if isinstance(value, Mapping):
        if not value:
            return [f"{indent}{rendered_key}:"]
        return [f"{indent}{rendered_key}:", *_encode_object_fields(value, depth + 1)]
    if isinstance(value, (list, tuple)):
        return _encode_array(rendered_key, list(value), depth)
    return [f"{indent}{rendered_key}: {_encode_scalar(value)}"]


def _encode_array(rendered_key: Optional[str], items: List[Any], depth: int) -> List[str]:
    indent = INDENT * depth
    prefix = rendered_key or ""
    count = len(items)

    if not items:
        return [f"{indent}{prefix}[0]:"]

    if all(_is_scalar(item) for item in items):
        values = ",".join(_encode_scalar(item) for item in items)
        return [f"{indent}{prefix}[{count}]: {values}"]

    if all(isinstance(item, Mapping) for item in items):
        fields = _uniform_fields(items)
        if fields is not None:
            return _encode_table(prefix, fields, items, depth)

    # Non-uniform / non-tabular array: one nested block per element rather
    # than producing invalid tabular output. TOON's nested-field-group
    # syntax (folding a uniform nested-object column into the header) would
    # be more compact here, but it isn't needed by anything Cortex actually
    # sends (flat SearchResult/MemoryChunk-shaped rows), so it's skipped.
    lines = [f"{indent}{prefix}[{count}]:"]
    child_depth = depth + 1
    for item in items:
        if isinstance(item, Mapping):
            lines.extend(_encode_object_fields(item, child_depth))
        elif isinstance(item, (list, tuple)):
            lines.extend(_encode_array(None, list(item), child_depth))
        else:
            lines.append(f"{INDENT * child_depth}{_encode_scalar(item)}")
    return lines


def _encode_table(prefix: str, fields: List[str], items: List[Mapping[str, Any]], depth: int) -> List[str]:
    indent = INDENT * depth
    header_fields = ",".join(_render_key(f) for f in fields)
    lines = [f"{indent}{prefix}[{len(items)}]{{{header_fields}}}:"]
    row_indent = INDENT * (depth + 1)
    for item in items:
        row = ",".join(_encode_scalar(item.get(field)) for field in fields)
        lines.append(f"{row_indent}{row}")
    return lines


def _uniform_fields(items: List[Mapping[str, Any]]) -> Optional[List[str]]:
    """The shared field list if every item has the same keys, in the same
    order, all scalar-valued (TOON's tabular form) -- else None."""
    first_keys = list(items[0].keys())
    for item in items:
        if list(item.keys()) != first_keys:
            return None
        if not all(_is_scalar(v) for v in item.values()):
            return None
    return first_keys


def _is_scalar(value: Any) -> bool:
    return value is None or isinstance(value, (str, int, float, bool))


def _render_key(key: str) -> str:
    if _UNQUOTED_KEY_RE.match(key):
        return key
    return _quote(key)


def _encode_scalar(value: Any) -> str:
    if value is None:
        return "null"
    if isinstance(value, bool):  # must precede the int check: bool is an int subclass
        return "true" if value else "false"
    if isinstance(value, (int, float)):
        return str(value)
    return _encode_string(str(value))


def _encode_string(value: str) -> str:
    return _quote(value) if _needs_quoting(value) else value


def _needs_quoting(value: str) -> bool:
    if value == "" or value != value.strip():
        return True
    if value in _RESERVED_LITERALS or _NUMERIC_RE.match(value):
        return True
    if any(ch in value for ch in _FORCE_QUOTE_CHARS):
        return True
    if any(ord(ch) < 0x20 for ch in value):
        return True
    if value == "-" or value.startswith("-") or value == "#" or value.startswith("#"):
        return True
    return False


def _quote(value: str) -> str:
    escaped = []
    for ch in value:
        if ch in _ESCAPES:
            escaped.append(_ESCAPES[ch])
        elif ord(ch) < 0x20:
            escaped.append(f"\\u{ord(ch):04x}")
        else:
            escaped.append(ch)
    return '"' + "".join(escaped) + '"'

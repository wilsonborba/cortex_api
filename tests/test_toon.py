from __future__ import annotations

import pytest

from lib.engine.format import ENCODERS, JSON, TOON, encode, encode_json, register_format
from lib.engine.format.toon import encode_toon


# --- literal spec examples (github.com/toon-format/spec, SPEC.md) ----------------
# Every expected string below is quoted verbatim from the spec fetch used to
# ground issue #14 -- not re-derived, not approximated.


def test_flat_object_of_scalars():
    data = {"id": 123, "name": "Ada", "active": True}
    assert encode_toon(data) == "id: 123\nname: Ada\nactive: true"


def test_nested_object():
    data = {"user": {"id": 123, "name": "Ada"}}
    assert encode_toon(data) == "user:\n  id: 123\n  name: Ada"


def test_inline_array_of_primitives():
    data = {"tags": ["admin", "ops", "dev"]}
    assert encode_toon(data) == "tags[3]: admin,ops,dev"


def test_tabular_array_of_uniform_objects():
    data = {"items": [{"sku": "A1", "qty": 2, "price": 9.99}, {"sku": "B2", "qty": 1, "price": 14.5}]}
    assert encode_toon(data) == "items[2]{sku,qty,price}:\n  A1,2,9.99\n  B2,1,14.5"


def test_key_requiring_quotes():
    data = {"my-key": [1, 2, 3]}
    assert encode_toon(data) == '"my-key"[3]: 1,2,3'


def test_string_containing_delimiter_char_gets_quoted_in_table():
    data = {"links": [{"id": 1, "url": "http://a:b"}, {"id": 2, "url": "https://example.com?q=a:b"}]}
    assert encode_toon(data) == (
        'links[2]{id,url}:\n  1,"http://a:b"\n  2,"https://example.com?q=a:b"'
    )


def test_empty_string_is_quoted():
    assert encode_toon({"name": ""}) == 'name: ""'


def test_reserved_literal_string_is_quoted():
    assert encode_toon({"enabled": "true"}) == 'enabled: "true"'


# --- quoting rule coverage (section 7.2), one behavior per rule --------------------


@pytest.mark.parametrize(
    "value,expected_quoted",
    [
        ("", True),  # empty
        (" leading", True),  # leading whitespace
        ("trailing ", True),  # trailing whitespace
        ("true", True),
        ("false", True),
        ("null", True),
        ("42", True),  # matches numeric pattern
        ("-3.14e10", True),  # matches numeric pattern (also starts with '-')
        ("a:b", True),  # colon
        ('a"b', True),  # quote
        ("a\\b", True),  # backslash
        ("a[b", True),  # bracket
        ("a{b", True),  # brace
        ("a,b", True),  # delimiter
        ("-leading-dash", True),
        ("#comment-like", True),
        ("plain", False),
        ("word_with_underscore", False),
    ],
)
def test_quoting_conditions(value: str, expected_quoted: bool):
    encoded = encode_toon({"v": value})
    is_quoted = encoded.startswith('v: "')
    assert is_quoted is expected_quoted, encoded


def test_control_characters_are_uxxxx_escaped():
    encoded = encode_toon({"v": "a\x01b"})
    assert encoded == 'v: "a\\u0001b"'


def test_newline_carriage_return_tab_use_short_escapes():
    encoded = encode_toon({"v": "a\nb\rc\td"})
    assert encoded == 'v: "a\\nb\\rc\\td"'


# --- non-uniform / empty / nested-in-array fallbacks -------------------------------


def test_empty_array():
    assert encode_toon({"items": []}) == "items[0]:"


def test_non_uniform_array_of_objects_falls_back_to_nested_blocks():
    data = {"items": [{"a": 1, "b": 2}, {"a": 1}]}  # different key sets: not tabular
    encoded = encode_toon(data)
    assert encoded == "items[2]:\n  a: 1\n  b: 2\n  a: 1"


def test_array_of_arrays_falls_back_to_nested_blocks():
    data = {"matrix": [[1, 2], [3, 4]]}
    encoded = encode_toon(data)
    assert encoded == "matrix[2]:\n  [2]: 1,2\n  [2]: 3,4"


def test_bare_top_level_list_of_scalars_does_not_crash():
    encoded = encode_toon(["a", "b", "c"])
    assert encoded == "[3]: a,b,c"


def test_bare_scalar_input():
    assert encode_toon("hello") == "hello"
    assert encode_toon(42) == "42"


# --- registry -----------------------------------------------------------------------


def test_registry_encodes_via_toon_and_json():
    data = {"a": 1}
    assert encode(data, TOON) == encode_toon(data)
    assert encode(data, JSON) == encode_json(data)


def test_registry_unknown_format_raises():
    with pytest.raises(ValueError, match="unknown context format"):
        encode({"a": 1}, "yaml")


def test_registering_a_new_format_is_a_pure_code_addition():
    register_format("upper", lambda data: str(data).upper())
    try:
        assert encode({"a": 1}, "upper") == "{'A': 1}"
    finally:
        del ENCODERS["upper"]  # keep the module-level registry clean for other tests

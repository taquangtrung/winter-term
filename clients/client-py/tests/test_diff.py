"""`_diff`/`LiveBlock.update_from`: minimal RFC 6902 patches between two values."""

from __future__ import annotations

import base64
import io
import json

import winter
from winter import _diff


def _parse_escape(escape: str) -> tuple[str, dict[str, str], object | None]:
    """Return (verb, params, decoded payload) from one live-block escape."""
    assert escape.startswith("\x1b]"), escape
    assert escape.endswith("\x1b\\"), escape
    fields = escape[2:-2].split(";")
    assert fields[0] == "9001", escape
    verb, params_field = fields[1], fields[2]
    params = dict(pair.split("=", 1) for pair in params_field.split(","))
    payload: object | None = None
    if len(fields) > 3:
        payload = json.loads(base64.standard_b64decode(fields[3]))
    return verb, params, payload


def test_diff_of_equal_values_is_empty():
    assert _diff({"a": 1}, {"a": 1}, "") == []
    assert _diff([1, 2], [1, 2], "") == []
    assert _diff("x", "x", "") == []


def test_diff_dict_adds_removes_and_replaces_top_level_keys():
    old = {"a": 1, "b": 2, "c": 3}
    new = {"a": 1, "b": 20, "d": 4}
    ops = _diff(old, new, "")
    assert {"op": "remove", "path": "/c"} in ops
    assert {"op": "add", "path": "/d", "value": 4} in ops
    assert {"op": "replace", "path": "/b", "value": 20} in ops
    assert len(ops) == 3


def test_diff_dict_recurses_into_nested_dicts_for_a_leaf_level_op():
    # A change three levels down must not blanket-replace the top object -
    # that's exactly the friction `update_from` exists to remove.
    old = {"table": {"rows": {"count": 1}}}
    new = {"table": {"rows": {"count": 2}}}
    assert _diff(old, new, "") == [
        {"op": "replace", "path": "/table/rows/count", "value": 2}
    ]


def test_diff_list_pure_append_emits_one_add_per_new_item():
    old = [1, 2]
    new = [1, 2, 3, 4]
    assert _diff(old, new, "/values") == [
        {"op": "add", "path": "/values/-", "value": 3},
        {"op": "add", "path": "/values/-", "value": 4},
    ]


def test_diff_list_changed_anywhere_but_the_tail_falls_back_to_replace():
    # Removed, reordered, or in-place-modified elements aren't a pure
    # append; a full minimal list diff is out of scope, so this is a
    # deliberate single whole-list replace instead.
    old = [1, 2, 3]
    new = [1, 5, 3]
    assert _diff(old, new, "/values") == [
        {"op": "replace", "path": "/values", "value": [1, 5, 3]}
    ]

    shrunk = [1]
    assert _diff(old, shrunk, "/values") == [
        {"op": "replace", "path": "/values", "value": [1]}
    ]


def test_diff_escapes_pointer_special_characters_in_keys():
    old = {}
    new = {"a/b": 1, "c~d": 2}
    ops = _diff(old, new, "")
    assert {"op": "add", "path": "/a~1b", "value": 1} in ops
    assert {"op": "add", "path": "/c~0d", "value": 2} in ops


def test_diff_type_change_is_a_single_replace():
    assert _diff({"a": 1}, [1, 2], "") == [
        {"op": "replace", "path": "", "value": [1, 2]}
    ]


def test_update_from_round_trips_through_patch_ops(monkeypatch):
    monkeypatch.setenv("WINTER", "1")
    out = io.StringIO()
    block = winter.live_block("application/json", {"count": 1}, stream=out)
    out.seek(0)
    out.truncate(0)

    block.update_from({"count": 1}, {"count": 2})

    verb, params, payload = _parse_escape(out.getvalue())
    assert verb == "patch"
    assert params["id"] == str(block.id)
    assert payload == [{"op": "replace", "path": "/count", "value": 2}]


def test_update_from_writes_nothing_when_the_values_are_equal(monkeypatch):
    monkeypatch.setenv("WINTER", "1")
    out = io.StringIO()
    block = winter.live_block("application/json", {"count": 1}, stream=out)
    out.seek(0)
    out.truncate(0)

    block.update_from({"count": 1}, {"count": 1})

    assert out.getvalue() == "", "an unchanged value must not emit an empty patch frame"

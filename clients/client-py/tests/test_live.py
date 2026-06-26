"""Live-block escapes: open/patch/close framing and out-of-terminal behavior."""

from __future__ import annotations

import base64
import io
import json

import winter


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


def test_open_frames_mime_and_spec(monkeypatch):
    monkeypatch.setenv("WINTER", "1")
    out = io.StringIO()

    block = winter.live_block("text/markdown", "# v0", stream=out)

    verb, params, payload = _parse_escape(out.getvalue())
    assert verb == "open"
    assert params["mime"] == "text/markdown"
    assert params["id"] == str(block.id)
    assert payload == "# v0"


def test_update_emits_a_root_replace_patch(monkeypatch):
    monkeypatch.setenv("WINTER", "1")
    out = io.StringIO()
    block = winter.live_block("text/markdown", "# v0", stream=out)
    out.seek(0)
    out.truncate(0)

    block.update("# v1")

    verb, params, payload = _parse_escape(out.getvalue())
    assert verb == "patch"
    assert params["id"] == str(block.id)
    assert payload == [{"op": "add", "path": "", "value": "# v1"}]


def test_patch_ops_pass_through(monkeypatch):
    monkeypatch.setenv("WINTER", "1")
    out = io.StringIO()
    block = winter.live_block(
        "application/vnd.vega-lite+json", {"values": [1]}, stream=out
    )
    out.seek(0)
    out.truncate(0)

    ops = [{"op": "add", "path": "/values/-", "value": 2}]
    block.patch_ops(ops)

    _, _, payload = _parse_escape(out.getvalue())
    assert payload == ops


def test_close_is_terminal_and_idempotent(monkeypatch):
    monkeypatch.setenv("WINTER", "1")
    out = io.StringIO()
    block = winter.live_block("text/plain", "x", stream=out)
    out.seek(0)
    out.truncate(0)

    block.close()
    escape = out.getvalue()
    verb, params, payload = _parse_escape(escape)
    assert verb == "close"
    assert params["id"] == str(block.id)
    assert payload is None

    block.close()
    block.update("ignored")
    assert out.getvalue() == escape, "a closed handle writes nothing further"


def test_two_blocks_get_distinct_ids(monkeypatch):
    monkeypatch.setenv("WINTER", "1")
    out = io.StringIO()
    a = winter.live_block("text/plain", "a", stream=out)
    b = winter.live_block("text/plain", "b", stream=out)
    assert a.id != b.id


def test_fallback_outside_winter_prints_text_once(monkeypatch):
    monkeypatch.delenv("WINTER", raising=False)
    monkeypatch.delenv("TERM_PROGRAM", raising=False)
    out = io.StringIO()

    block = winter.live_block("text/markdown", "# v0", text="just text", stream=out)
    block.update("# v1")
    block.close()

    assert out.getvalue() == "just text"

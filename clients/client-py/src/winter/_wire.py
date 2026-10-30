"""OSC 9001 framing and capability detection for TBP.

This module owns the wire form so the public API in :mod:`winter` stays about
*what* to render, not *how* it is encoded. The framing mirrors
``crates/winter-proto`` and ``docs/terminal-block-protocol-spec.md``:

    OSC 9001 ; emit ; v=1,id=N,trust=TIER ; base64(json bundle) ST

JSON is written compact with sorted keys, the form ``serde_json`` writes, so
an escape built here is byte-for-byte the one the Rust codec builds; the
shared vectors in ``docs/tbp-golden-vectors.json`` hold both to that.
"""

from __future__ import annotations

import base64
import itertools
import json
import os
import sys
from typing import TextIO

# ========================================================================
# Constants
# ========================================================================

PROTOCOL_VERSION = 1
TEXT_PLAIN = "text/plain"

_OSC_START = "\x1b]"
_ST = "\x1b\\"
_OSC_NUMBER = "9001"
_VERB_EMIT = "emit"
_VERB_OPEN = "open"
_VERB_PATCH = "patch"
_VERB_CLOSE = "close"

_WINTER_ENV = "WINTER"
_TERM_PROGRAM_ENV = "TERM_PROGRAM"
_WINTER_NAME = "winter"

# Variables a terminal multiplexer sets inside its panes (tmux, GNU screen).
# A multiplexer swallows TBP escapes, and ``WINTER`` leaks into it from the
# Winter pane it was started in, so inside one the fallback is what shows.
_MULTIPLEXER_ENVS = ("TMUX", "STY")

# Low bits of a block id that count blocks within this process; the bits
# above hold the process id, so two programs in one session never share ids
# (mirrors ``BlockId::process_scoped`` in ``winter-proto``).
BLOCK_SEQUENCE_BITS = 20
_BLOCK_SEQUENCE_MASK = (1 << BLOCK_SEQUENCE_BITS) - 1

# Bytes with a meaning in the framing, percent-encoded in parameter values.
_FRAMING_CHARS = frozenset(";,=%")
_FIRST_PRINTABLE = 0x20
_DELETE = 0x7F

_JSON_SEPARATORS = (",", ":")

_VALID_TIERS = ("isolated", "restricted", "trusted")

_block_sequence = itertools.count(1)


# ========================================================================
# Capability detection
# ========================================================================


def supported() -> bool:
    """Whether the current terminal is known to understand TBP.

    For now this is environment-based: Winter exports ``WINTER`` (and sets
    ``TERM_PROGRAM=winter``). Inside tmux or screen the answer is no, even
    when those were inherited from a Winter pane.
    """
    if any(os.environ.get(name) for name in _MULTIPLEXER_ENVS):
        return False
    if os.environ.get(_WINTER_ENV):
        return True
    return os.environ.get(_TERM_PROGRAM_ENV) == _WINTER_NAME


# ========================================================================
# Emission
# ========================================================================


def emit(
    mime: dict[str, str],
    *,
    title: str | None = None,
    height_hint: int | None = None,
    trust: str = "restricted",
    stream: TextIO | None = None,
) -> None:
    """Write ``mime`` as a TBP block, or its ``text/plain`` fallback elsewhere.

    ``mime`` maps each MIME type to its representation: a UTF-8 string for text
    and SVG payloads, a base64 string for binary images.
    """
    if trust not in _VALID_TIERS:
        raise ValueError(f"trust must be one of {_VALID_TIERS}, got {trust!r}")

    out = stream if stream is not None else sys.stdout
    if not supported():
        out.write(mime.get(TEXT_PLAIN, ""))
        out.flush()
        return

    out.write(
        frame_emit(
            next_block_id(), mime, title=title, height_hint=height_hint, trust=trust
        )
    )
    out.flush()


def frame_emit(
    block_id: int,
    mime: dict[str, str],
    *,
    title: str | None = None,
    height_hint: int | None = None,
    trust: str = "restricted",
) -> str:
    """The escape emitting ``mime`` as block ``block_id``."""
    bundle: dict[str, object] = {"mime": mime}
    meta: dict[str, object] = {}
    if title is not None:
        meta["title"] = title
    if height_hint is not None:
        meta["height_hint"] = height_hint
    if meta:
        bundle["meta"] = meta

    payload = _b64_json(bundle)
    params = f"v={PROTOCOL_VERSION},id={block_id},trust={_escape_param(trust)}"
    return f"{_OSC_START}{_OSC_NUMBER};{_VERB_EMIT};{params};{payload}{_ST}"


def next_block_id() -> int:
    """Reserve the next block id (shared by emit and live blocks)."""
    sequence = next(_block_sequence) & _BLOCK_SEQUENCE_MASK
    return (os.getpid() << BLOCK_SEQUENCE_BITS) | sequence


def _b64_json(value: object) -> str:
    """`value` as base64 JSON. NaN and the infinities are refused: written as
    `NaN`, which JSON has no word for, the terminal could not read the block
    and dropped it without a word."""
    try:
        text = json.dumps(
            value,
            allow_nan=False,
            separators=_JSON_SEPARATORS,
            sort_keys=True,
            ensure_ascii=False,
        )
    except ValueError as error:
        raise ValueError(f"a block cannot hold NaN or infinity: {error}") from error
    return base64.standard_b64encode(text.encode("utf-8")).decode("ascii")


def _escape_param(value: str) -> str:
    """Percent-encode the framing and control characters in a parameter value,
    which would otherwise split the frame or end the escape early."""
    return "".join(_escape_param_char(char) for char in value)


def _escape_param_char(char: str) -> str:
    code = ord(char)
    if char in _FRAMING_CHARS or code < _FIRST_PRINTABLE or code == _DELETE:
        return f"%{code:02X}"
    return char


def frame_open(block_id: int, mime: str, spec: object) -> str:
    """The escape opening a live block with its initial spec."""
    params = f"id={block_id},mime={_escape_param(mime)}"
    return f"{_OSC_START}{_OSC_NUMBER};{_VERB_OPEN};{params};{_b64_json(spec)}{_ST}"


def frame_patch(block_id: int, patch: list) -> str:
    """The escape applying an RFC 6902 operation array to a live block."""
    body = f"{_VERB_PATCH};id={block_id};{_b64_json(patch)}"
    return f"{_OSC_START}{_OSC_NUMBER};{body}{_ST}"


def frame_close(block_id: int) -> str:
    """The escape closing a live block."""
    return f"{_OSC_START}{_OSC_NUMBER};{_VERB_CLOSE};id={block_id}{_ST}"

"""OSC 9001 framing and capability detection for TBP.

This module owns the wire form so the public API in :mod:`winter` stays about
*what* to render, not *how* it is encoded. The framing mirrors
``crates/winter-proto`` and ``docs/terminal-block-protocol-spec.md``:

    OSC 9001 ; emit ; v=1,id=N,trust=TIER ; base64(json bundle) ST
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

_VALID_TIERS = ("isolated", "restricted", "trusted")

_block_ids = itertools.count(1)


# ========================================================================
# Capability detection
# ========================================================================


def supported() -> bool:
    """Whether the current terminal is known to understand TBP.

    For now this is environment-based: Winter exports ``WINTER`` (and sets
    ``TERM_PROGRAM=winter``).
    """
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

    out.write(_frame_emit(mime, title=title, height_hint=height_hint, trust=trust))
    out.flush()


def _frame_emit(
    mime: dict[str, str],
    *,
    title: str | None,
    height_hint: int | None,
    trust: str,
) -> str:
    bundle: dict[str, object] = {"mime": mime}
    meta: dict[str, object] = {}
    if title is not None:
        meta["title"] = title
    if height_hint is not None:
        meta["height_hint"] = height_hint
    if meta:
        bundle["meta"] = meta

    payload = _b64_json(bundle)
    params = f"v={PROTOCOL_VERSION},id={next_block_id()},trust={trust}"
    return f"{_OSC_START}{_OSC_NUMBER};{_VERB_EMIT};{params};{payload}{_ST}"


def next_block_id() -> int:
    """Reserve the next block id (shared by emit and live blocks)."""
    return next(_block_ids)


def _b64_json(value: object) -> str:
    return base64.standard_b64encode(json.dumps(value).encode("utf-8")).decode("ascii")


def frame_open(block_id: int, mime: str, spec: object) -> str:
    """The escape opening a live block with its initial spec."""
    params = f"id={block_id},mime={mime}"
    return f"{_OSC_START}{_OSC_NUMBER};{_VERB_OPEN};{params};{_b64_json(spec)}{_ST}"


def frame_patch(block_id: int, patch: list) -> str:
    """The escape applying an RFC 6902 operation array to a live block."""
    return f"{_OSC_START}{_OSC_NUMBER};{_VERB_PATCH};id={block_id};{_b64_json(patch)}{_ST}"


def frame_close(block_id: int) -> str:
    """The escape closing a live block."""
    return f"{_OSC_START}{_OSC_NUMBER};{_VERB_CLOSE};id={block_id}{_ST}"


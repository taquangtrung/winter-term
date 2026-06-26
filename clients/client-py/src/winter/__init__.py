"""Winter: emit rich Terminal Block Protocol (TBP) blocks from Python.

Quick use::

    import winter
    winter.display(dataframe)              # uses the object's _repr_*_ methods
    winter.display_svg(open("plot.svg").read())
    winter.display_image("chart.png")

Every block carries a ``text/plain`` fallback, and when Winter is not the active
terminal the fallback is printed instead, so scripts stay safe under tmux/ssh/CI.
"""

from __future__ import annotations

import base64
import sys
from pathlib import Path
from typing import TextIO

from winter._repr import mime_map_from_object
from winter._wire import (
    TEXT_PLAIN,
    emit,
    frame_close,
    frame_open,
    frame_patch,
    next_block_id,
    supported,
)

__all__ = [
    "LiveBlock",
    "display",
    "display_html",
    "display_image",
    "display_latex",
    "display_markdown",
    "display_svg",
    "live_block",
    "supported",
]

# ========================================================================
# Constants
# ========================================================================

_IMAGE_MIME_BY_SUFFIX = {
    ".gif": "image/gif",
    ".jpeg": "image/jpeg",
    ".jpg": "image/jpeg",
    ".png": "image/png",
    ".svg": "image/svg+xml",
    ".webp": "image/webp",
}

_SVG_MIME = "image/svg+xml"


# ========================================================================
# Public API
# ========================================================================


class LiveBlock:
    """A handle to an open live TBP block, updated in place via patches.

    Outside Winter the open call prints the fallback once and every later
    ``update``/``patch_ops``/``close`` is a no-op, so streaming loops stay
    safe under tmux/ssh/CI.
    """

    def __init__(
        self, block_id: int, mime: str, stream: TextIO, *, live: bool = True
    ) -> None:
        self._id = block_id
        self._mime = mime
        self._stream = stream
        self._live = live

    @property
    def id(self) -> int:
        return self._id

    @property
    def mime(self) -> str:
        return self._mime

    def update(self, spec: object) -> None:
        """Replace the block's whole spec with one patch."""
        self.patch_ops([{"op": "add", "path": "", "value": spec}])

    def update_from(self, old: object, new: object) -> None:
        """Patch from `old`'s shape to `new`'s with a minimal diff.

        Dict keys are added/removed/replaced (recursing into matching keys,
        so a change to one nested field emits one small op instead of
        replacing the whole object); a list that only grew a tail emits one
        `add` per appended item; anything else emits a single `replace`.
        """
        ops = _diff(old, new, "")
        if ops:
            self.patch_ops(ops)

    def patch_ops(self, ops: list[dict]) -> None:
        """Apply RFC 6902 operations to the block's current spec."""
        if not self._live:
            return
        self._stream.write(frame_patch(self._id, ops))
        self._stream.flush()

    def close(self) -> None:
        """End the block; the terminal freezes its last state."""
        if not self._live:
            return
        self._stream.write(frame_close(self._id))
        self._stream.flush()
        self._live = False


def live_block(
    mime: str,
    spec: object,
    *,
    text: str | None = None,
    stream: TextIO | None = None,
) -> LiveBlock:
    """Open a live block and return a handle for streaming updates::

        block = winter.live_block("text/markdown", "# title")
        block.update("# title\nmore")
        block.patch_ops([{"op": "add", "path": "/sections/-", "value": 1}])
        block.close()

    ``spec`` is the block's initial state: a string for text mimes, an
    object/array for structured ones (vega, JSON). Outside Winter, prints
    ``text`` (or nothing) once and returns an inert handle.
    """
    out = stream if stream is not None else sys.stdout
    if not supported():
        out.write(text if text is not None else "")
        out.flush()
        return LiveBlock(0, mime, out, live=False)
    block = LiveBlock(next_block_id(), mime, out)
    out.write(frame_open(block.id, mime, spec))
    out.flush()
    return block


# ========================================================================
# Diffing
# ========================================================================


def _escape_pointer_segment(segment: str) -> str:
    """Encode `segment` as one RFC 6901 JSON Pointer path component."""
    return segment.replace("~", "~0").replace("/", "~1")


def _diff(old: object, new: object, path: str) -> list[dict]:
    """The minimal RFC 6902 ops that turn `old` into `new` at `path`.

    Dicts recurse per key (`remove`/`add`/nested diff); a list that's `old`
    plus a tail becomes one `add` per appended item; anything else -
    scalars, a changed type, a list changed anywhere but its tail - becomes
    a single `replace`. A full minimal list diff (LCS) is out of scope.
    """
    if old == new:
        return []
    if isinstance(old, dict) and isinstance(new, dict):
        ops: list[dict] = []
        for key in old:
            if key not in new:
                ops.append(
                    {"op": "remove", "path": f"{path}/{_escape_pointer_segment(key)}"}
                )
        for key in new:
            key_path = f"{path}/{_escape_pointer_segment(key)}"
            if key not in old:
                ops.append({"op": "add", "path": key_path, "value": new[key]})
            elif old[key] != new[key]:
                ops.extend(_diff(old[key], new[key], key_path))
        return ops
    if isinstance(old, list) and isinstance(new, list) and new[: len(old)] == old:
        return [
            {"op": "add", "path": f"{path}/-", "value": item}
            for item in new[len(old) :]
        ]
    return [{"op": "replace", "path": path, "value": new}]


def display(
    obj: object,
    *,
    title: str | None = None,
    height_hint: int | None = None,
    trust: str = "restricted",
) -> None:
    """Render ``obj`` using its richest available representation."""
    emit(mime_map_from_object(obj), title=title, height_hint=height_hint, trust=trust)


def display_html(
    html: str,
    *,
    text: str | None = None,
    title: str | None = None,
    height_hint: int | None = None,
    trust: str = "restricted",
) -> None:
    """Render an HTML fragment inline."""
    emit(
        {"text/html": html, TEXT_PLAIN: text or "[html block]"},
        title=title,
        height_hint=height_hint,
        trust=trust,
    )


def display_svg(
    svg: str,
    *,
    text: str | None = None,
    title: str | None = None,
    height_hint: int | None = None,
    trust: str = "restricted",
) -> None:
    """Render an SVG document inline."""
    emit(
        {_SVG_MIME: svg, TEXT_PLAIN: text or "[svg image]"},
        title=title,
        height_hint=height_hint,
        trust=trust,
    )


def display_markdown(
    markdown: str,
    *,
    text: str | None = None,
    title: str | None = None,
    height_hint: int | None = None,
    trust: str = "restricted",
) -> None:
    """Render Markdown inline. The raw Markdown is the text fallback."""
    emit(
        {"text/markdown": markdown, TEXT_PLAIN: text or markdown},
        title=title,
        height_hint=height_hint,
        trust=trust,
    )


def display_latex(
    latex: str,
    *,
    text: str | None = None,
    title: str | None = None,
    height_hint: int | None = None,
    trust: str = "restricted",
) -> None:
    """Render a LaTeX expression inline. The raw source is the text fallback."""
    emit(
        {"text/latex": latex, TEXT_PLAIN: text or latex},
        title=title,
        height_hint=height_hint,
        trust=trust,
    )


def display_image(
    source: str | Path | bytes | bytearray,
    *,
    mime: str | None = None,
    text: str | None = None,
    title: str | None = None,
    height_hint: int | None = None,
    trust: str = "restricted",
) -> None:
    """Render an image from a file path or raw bytes.

    For a path the MIME type is inferred from the extension when not given; for
    bytes ``mime`` is required.
    """
    if isinstance(source, (bytes, bytearray)):
        if mime is None:
            raise ValueError("mime is required when source is bytes")
        data = bytes(source)
    else:
        path = Path(source)
        data = path.read_bytes()
        mime = mime or _IMAGE_MIME_BY_SUFFIX.get(path.suffix.lower())
        if mime is None:
            raise ValueError(f"cannot infer MIME type from suffix {path.suffix!r}")

    payload = (
        data.decode("utf-8")
        if mime == _SVG_MIME
        else base64.standard_b64encode(data).decode("ascii")
    )
    fallback = text or f"[{mime} image, {len(data)} bytes]"
    emit(
        {mime: payload, TEXT_PLAIN: fallback},
        title=title,
        height_hint=height_hint,
        trust=trust,
    )

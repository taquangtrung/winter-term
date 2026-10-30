"""The shared TBP golden vectors, checked against this client's framing.

``docs/tbp-golden-vectors.json`` holds the exact escape every client must
write for each message; the Rust codec and the shell client are checked
against the same file in ``crates/winter-proto/tests/golden_vectors.rs``.
"""

from __future__ import annotations

import json
from pathlib import Path

import pytest

from winter._wire import frame_close, frame_emit, frame_open, frame_patch

# ========================================================================
# Constants
# ========================================================================

_VECTORS_PATH = Path(__file__).parents[3] / "docs" / "tbp-golden-vectors.json"


# ========================================================================
# Helpers
# ========================================================================


def _vectors() -> list[dict]:
    return json.loads(_VECTORS_PATH.read_text(encoding="utf-8"))["vectors"]


def _frame(vector: dict) -> str:
    verb = vector["verb"]
    if verb == "emit":
        bundle = vector["bundle"]
        meta = bundle.get("meta", {})
        return frame_emit(
            vector["id"],
            bundle["mime"],
            title=meta.get("title"),
            height_hint=meta.get("height_hint"),
            trust=vector["trust"],
        )
    if verb == "open":
        return frame_open(vector["id"], vector["mime"], vector["spec"])
    if verb == "patch":
        return frame_patch(vector["id"], vector["patch"])
    if verb == "close":
        return frame_close(vector["id"])
    raise AssertionError(f"unknown verb {verb!r}")


# ========================================================================
# Tests
# ========================================================================


@pytest.mark.parametrize("vector", _vectors(), ids=lambda vector: vector["name"])
def test_the_client_writes_each_vector_exactly(vector: dict) -> None:
    assert _frame(vector) == vector["escape"]

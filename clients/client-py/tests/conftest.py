"""Shared fixtures: every test runs as if outside any terminal multiplexer."""

from __future__ import annotations

import pytest


@pytest.fixture(autouse=True)
def _outside_multiplexers(monkeypatch: pytest.MonkeyPatch) -> None:
    # The client gives tmux and screen the fallback; a suite run inside one
    # must not have every rich-block test take that path.
    monkeypatch.delenv("TMUX", raising=False)
    monkeypatch.delenv("STY", raising=False)

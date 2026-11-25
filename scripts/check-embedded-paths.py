"""Fail when an embedded file path escapes its crate root.

``cargo package`` copies only the files under a crate's own directory, so an
``include_str!`` or ``include_bytes!`` that reaches above it resolves fine in
the workspace and breaks for everyone installing from the registry. A publish
dry run catches this, but only for crates whose dependencies are already on
the index, which is never true for the crate at the top of the graph.
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

# ========================================================================
# Constants
# ========================================================================

EMBED_MACRO = re.compile(r'include_(?:str|bytes)!\s*\(\s*"([^"]+)"')

SOURCE_SUFFIX = "*.rs"
MANIFEST_NAME = "Cargo.toml"
SKIP_DIRS = frozenset({"target", ".git"})


# ========================================================================
# Functions
# ========================================================================


def crate_roots(workspace: Path) -> list[Path]:
    """Every directory holding a manifest, excluding the workspace root."""
    roots = [
        manifest.parent
        for manifest in workspace.rglob(MANIFEST_NAME)
        if not SKIP_DIRS & set(manifest.relative_to(workspace).parts)
    ]
    return sorted(root for root in roots if root != workspace)


def escaping_embeds(crate: Path) -> list[tuple[Path, int, str]]:
    """Embedded paths in this crate that resolve outside its own directory."""
    found: list[tuple[Path, int, str]] = []
    for source in sorted(crate.rglob(SOURCE_SUFFIX)):
        if SKIP_DIRS & set(source.relative_to(crate).parts):
            continue
        for number, line in enumerate(source.read_text().splitlines(), start=1):
            for embedded in EMBED_MACRO.findall(line):
                target = (source.parent / embedded).resolve()
                if not target.is_relative_to(crate.resolve()):
                    found.append((source, number, embedded))
    return found


def main() -> int:
    workspace = Path(__file__).resolve().parent.parent
    violations = [
        (crate, source, number, embedded)
        for crate in crate_roots(workspace)
        for source, number, embedded in escaping_embeds(crate)
    ]

    for crate, source, number, embedded in violations:
        location = source.relative_to(workspace)
        print(
            f"{location}:{number}: embeds {embedded!r}, which is outside "
            f"{crate.relative_to(workspace)} and will not be packaged",
            file=sys.stderr,
        )

    if violations:
        print(
            f"\n{len(violations)} embedded path(s) escape their crate. "
            "Move the file under the crate, or the published crate will not "
            "build.",
            file=sys.stderr,
        )
        return 1

    print(f"checked {len(crate_roots(workspace))} crates: no escaping embeds")
    return 0


if __name__ == "__main__":
    sys.exit(main())

"""Fail when a packaging file points at a source path that no longer exists.

The installers copy files in by relative path, so moving a file in the
repository breaks them silently: nothing references those paths from Rust, and
the break only surfaces when someone builds an installer on the right OS.
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

# ========================================================================
# Constants
# ========================================================================

# Build output, and paths carrying a shell or preprocessor variable, which
# cannot be resolved statically.
IGNORED = re.compile(r"^target\b|\{#|\$")

INNO_SOURCE = re.compile(r"(?:Source:\s*\"|SetupIconFile=|LicenseFile=)([^\";\n]+)")
PKGBUILD_SOURCE = re.compile(r"install -D[a-z0-9]* \"([^\"$][^\"]*)\"")


# ========================================================================
# Functions
# ========================================================================


def inno_paths(path: Path) -> list[str]:
    """Source paths an Inno Setup script reads, relative to the repository."""
    found = []
    for raw in INNO_SOURCE.findall(path.read_text()):
        rel = raw.strip().replace("\\", "/")
        if rel.startswith("../../"):
            rel = rel[len("../../") :]
        found.append(rel)
    return found


def pkgbuild_paths(path: Path) -> list[str]:
    """Source paths a PKGBUILD installs, relative to the repository."""
    return [p for p in PKGBUILD_SOURCE.findall(path.read_text())]


def main() -> int:
    workspace = Path(__file__).resolve().parent.parent
    readers = {
        "packaging/windows/installer.iss": inno_paths,
        "packaging/aur/PKGBUILD": pkgbuild_paths,
    }

    missing, checked = [], 0
    for rel, reader in readers.items():
        source = workspace / rel
        if not source.exists():
            continue
        for candidate in reader(source):
            if IGNORED.search(candidate):
                continue
            checked += 1
            if not (workspace / candidate).exists():
                missing.append(f"{rel}: {candidate!r} does not exist")

    for line in missing:
        print(line, file=sys.stderr)
    if missing:
        print(
            f"\n{len(missing)} packaging path(s) point at files that moved. "
            "Update the packaging file, or the installer build will fail.",
            file=sys.stderr,
        )
        return 1

    print(f"checked {checked} packaging paths: all resolve")
    return 0


if __name__ == "__main__":
    sys.exit(main())

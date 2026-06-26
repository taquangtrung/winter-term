#!/usr/bin/env bash
# Single PNG of a window. Click the window when the cursor changes.
#   ./scripts/demo/shot.sh docs/screenshot.png
set -euo pipefail
OUT="${1:-docs/screenshot.png}"
mkdir -p "$(dirname "$OUT")"
import -window "$(xdotool selectwindow)" "$OUT"
printf 'wrote %s (%s)\n' "$OUT" "$(du -h "$OUT" | cut -f1)"

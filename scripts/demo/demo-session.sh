#!/usr/bin/env bash
# A scripted tour of what makes Winter different, for recording a README demo.
# Run this *inside* a Winter window, then record that window.
#
#   ./scripts/demo/demo-session.sh
#
# It types slowly on purpose: a demo that scrolls faster than a viewer can read
# shows nothing. Adjust the pace with DEMO_SPEED (seconds per character).
set -euo pipefail

SPEED="${DEMO_SPEED:-0.045}"
PAUSE="${DEMO_PAUSE:-1.4}"
CLIENT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)/clients/client.sh"

# Echo a command as if typed, then run it.
type_run() {
    printf '$ '
    local i
    for ((i = 0; i < ${#1}; i++)); do
        printf '%s' "${1:i:1}"
        sleep "$SPEED"
    done
    printf '\n'
    eval "$1"
    sleep "$PAUSE"
}

say() { printf '\n\033[1;36m# %s\033[0m\n' "$1"; sleep "$PAUSE"; }

clear
say 'Ordinary output still looks ordinary.'
type_run "ls --color=always -1 crates"

say 'But a tool can hand the terminal a typed block instead of characters.'
cat > /tmp/winter-demo.svg <<'SVG'
<svg xmlns="http://www.w3.org/2000/svg" width="360" height="90">
  <rect width="360" height="90" rx="10" fill="#1e2030"/>
  <rect x="16" y="30" width="120" height="18" rx="4" fill="#7aa2f7"/>
  <rect x="16" y="54" width="232" height="18" rx="4" fill="#9ece6a"/>
  <text x="16" y="22" fill="#c0caf5" font-family="monospace" font-size="13">
    rendered inline, not printed
  </text>
</svg>
SVG
type_run "WINTER=1 $CLIENT svg /tmp/winter-demo.svg"

say 'Markdown too: a real table, not ASCII art.'
type_run "printf '| crate | role |\n|---|---|\n| winter-proto | wire format |\n| winter-core | block model |\n' | WINTER=1 $CLIENT markdown -"

say 'Every block carries a text/plain fallback, so piping still works.'
type_run "$CLIENT svg /tmp/winter-demo.svg | head -3"

say 'Now press Esc for Normal mode, and navigate with vim keys.'
sleep 3

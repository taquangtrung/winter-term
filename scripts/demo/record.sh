#!/usr/bin/env bash
# Record a window to a high-quality GIF. X11 only; needs ffmpeg.
#
#   ./scripts/demo/record.sh                 # click the window to record
#   ./scripts/demo/record.sh 20 out.gif      # 20 seconds, custom name
#
# Two-pass palette generation, because ffmpeg's default 256-colour quantiser
# turns terminal text into mud.
set -euo pipefail

SECONDS_TO_RECORD="${1:-15}"
OUT="${2:-docs/demo.gif}"
FPS="${FPS:-12}"          # 12 is plenty for a terminal and keeps the file small
GIF_WIDTH="${GIF_WIDTH:-900}"  # output width; GitHub renders READMEs around 900px

command -v ffmpeg >/dev/null || { echo "ffmpeg is required" >&2; exit 1; }
command -v xdotool >/dev/null || { echo "xdotool is required" >&2; exit 1; }

echo "Click the window you want to record..."
WIN=$(xdotool selectwindow)
eval "$(xdotool getwindowgeometry --shell "$WIN")"
# `getwindowgeometry --shell` sets X, Y, WIDTH, HEIGHT. x264 needs even dimensions.
W=$((WIDTH - WIDTH % 2)); H=$((HEIGHT - HEIGHT % 2))

xdotool windowactivate "$WIN"; sleep 0.4
echo "Recording ${W}x${H} at +${X},${Y} for ${SECONDS_TO_RECORD}s..."

mkdir -p "$(dirname "$OUT")"
TMP=$(mktemp -d); trap 'rm -rf "$TMP"' EXIT
ffmpeg -hide_banner -loglevel error -y \
    -f x11grab -framerate "$FPS" -video_size "${W}x${H}" -i "${DISPLAY}+${X},${Y}" \
    -t "$SECONDS_TO_RECORD" -c:v libx264 -pix_fmt yuv420p "$TMP/raw.mp4"

echo "Encoding GIF..."
FILTERS="fps=${FPS},scale=${GIF_WIDTH}:-1:flags=lanczos"
ffmpeg -hide_banner -loglevel error -y -i "$TMP/raw.mp4" \
    -vf "${FILTERS},palettegen=stats_mode=diff" "$TMP/palette.png"
ffmpeg -hide_banner -loglevel error -y -i "$TMP/raw.mp4" -i "$TMP/palette.png" \
    -lavfi "${FILTERS} [x]; [x][1:v] paletteuse=dither=bayer:bayer_scale=3" "$OUT"

printf 'wrote %s (%s)\n' "$OUT" "$(du -h "$OUT" | cut -f1)"
[ "$(stat -c%s "$OUT")" -gt 5000000 ] &&
    echo "Over 5MB: lower FPS or GIF_WIDTH, or record a shorter clip." >&2 || true

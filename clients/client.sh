#!/usr/bin/env bash
# client.sh: emit Terminal Block Protocol (TBP) blocks from the shell.
#
# Source it to use the winter_emit_* functions, or run it as a CLI:
#   ./client.sh svg plot.svg
#   ./client.sh image chart.png
#   cat report.md | ./client.sh markdown -
#
# Every block carries a text/plain fallback, and when Winter is not the active
# terminal (no $WINTER, $TERM_PROGRAM != winter) the fallback is printed instead,
# so pipelines stay safe under tmux/ssh/CI. Wire form mirrors
# docs/terminal-block-protocol-spec.md.
#
# Strict mode (`set -euo pipefail`) is turned on only when run as a CLI:
# sourced, it would hold for the user's interactive shell, where the first
# failing command would close the terminal.

# =============================================================================
# Constants
# =============================================================================

OSC_START=$'\e]'
ST=$'\e\\'
OSC_NUMBER="9001"
VERB_EMIT="emit"
VERB_OPEN="open"
VERB_PATCH="patch"
VERB_CLOSE="close"
PROTOCOL_VERSION="1"
# A live block's id on the wire is the caller's own number under the
# process the blocks belong to (pid << 20 | number), as every Winter client
# scopes them: two scripts both opening block 1 would otherwise open one.
BLOCK_SEQUENCE_BITS=20
BLOCK_SEQUENCE_MASK=$(( (1 << BLOCK_SEQUENCE_BITS) - 1 ))
# The process the blocks belong to, where the caller says: else the shell
# that sourced this file, or, run as a CLI (one process per call), the
# script calling it.
WINTER_BLOCK_PID="${WINTER_BLOCK_PID:-}"
DEFAULT_TRUST="restricted"
TEXT_PLAIN="text/plain"

# Variables a terminal multiplexer sets inside its panes (tmux, GNU screen).
# A multiplexer swallows TBP escapes, and $WINTER leaks into it from the
# Winter pane it was started in, so inside one the fallback is what shows.
MULTIPLEXER_ENVS=(TMUX STY)

# Low bits of an emitted block's id that count blocks within this shell; the
# bits above hold its pid, so two programs in one session never share ids
# (mirrors BlockId::process_scoped in winter-proto).
BLOCK_SEQUENCE_BITS=20
BLOCK_SEQUENCE_MASK=$(( (1 << BLOCK_SEQUENCE_BITS) - 1 ))

WINTER_BLOCK_SEQUENCE=0

# Report a failure and stop: the script when run, only the function when
# sourced, so a sourced client never exits the user's shell.
die() {
    printf 'client.sh: %s\n' "$*" >&2
    if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
        exit 1
    fi
    return 1
}

# =============================================================================
# Capability detection
# =============================================================================

winter_supported() {
    local name
    for name in "${MULTIPLEXER_ENVS[@]}"; do
        [[ -n "${!name:-}" ]] && return 1
    done
    [[ -n "${WINTER:-}" ]] && return 0
    [[ "${TERM_PROGRAM:-}" == "winter" ]]
}

# =============================================================================
# Emission
# =============================================================================

# Emit a UTF-8 text representation (svg/html/markdown/latex) with a fallback.
# Usage: winter_emit_text <mime> <value> <fallback> [trust]
winter_emit_text() {
    local mime="$1" value="$2" fallback="$3" trust="${4:-$DEFAULT_TRUST}"
    local bundle
    bundle="$(printf '{"mime":{"%s":"%s","%s":"%s"}}' \
        "$(json_escape "$mime")" "$(json_escape "$value")" \
        "$TEXT_PLAIN" "$(json_escape "$fallback")")"
    emit_bundle "$bundle" "$trust" "$fallback"
}

# Emit a base64 binary representation (images) with a fallback.
# Usage: winter_emit_binary <mime> <base64> <fallback> [trust]
winter_emit_binary() {
    local mime="$1" data_b64="$2" fallback="$3" trust="${4:-$DEFAULT_TRUST}"
    local bundle
    bundle="$(printf '{"mime":{"%s":"%s","%s":"%s"}}' \
        "$(json_escape "$mime")" "$data_b64" \
        "$TEXT_PLAIN" "$(json_escape "$fallback")")"
    emit_bundle "$bundle" "$trust" "$fallback"
}

# Frame a complete bundle JSON into an OSC 9001 escape, or print the fallback.
emit_bundle() {
    local bundle_json="$1" trust="$2" fallback="$3"
    if ! winter_supported; then
        printf '%s' "$fallback"
        return 0
    fi
    local payload id
    payload="$(printf '%s' "$bundle_json" | base64 | tr -d '\n')"
    WINTER_BLOCK_SEQUENCE=$((WINTER_BLOCK_SEQUENCE + 1))
    id=$(( ($$ << BLOCK_SEQUENCE_BITS) | (WINTER_BLOCK_SEQUENCE & BLOCK_SEQUENCE_MASK) ))
    printf '%s%s;%s;v=%s,id=%s,trust=%s;%s%s' \
        "$OSC_START" "$OSC_NUMBER" "$VERB_EMIT" \
        "$PROTOCOL_VERSION" "$id" "$(param_escape "$trust")" "$payload" "$ST"
}

# =============================================================================
# Typed helpers
# =============================================================================

winter_emit_svg() {
    local content; content="$(read_source "${1:--}")"
    winter_emit_text "image/svg+xml" "$content" "[svg image]"
}

winter_emit_html() {
    local content; content="$(read_source "${1:--}")"
    winter_emit_text "text/html" "$content" "[html block]"
}

winter_emit_markdown() {
    local content; content="$(read_source "${1:--}")"
    # Raw Markdown reads fine as plain text, so it is its own fallback.
    winter_emit_text "text/markdown" "$content" "$content"
}

winter_emit_latex() {
    local content; content="$(read_source "${1:--}")"
    winter_emit_text "text/latex" "$content" "$content"
}

winter_emit_image() {
    local file="$1" mime data_b64
    [[ -r "$file" ]] || die "cannot read image: $file"
    mime="$(image_mime_for "$file")"
    # SVG is markup, sent as text the way the Rust and Python clients send
    # it; as base64 the terminal took the encoded text for the markup.
    if [[ "$mime" == "image/svg+xml" ]]; then
        winter_emit_text "$mime" "$(cat "$file")" "[$mime image]"
        return
    fi
    data_b64="$(base64 < "$file" | tr -d '\n')"
    winter_emit_binary "$mime" "$data_b64" "[$mime image]"
}

# =============================================================================
# Live blocks
# =============================================================================

# The wire id of the caller's block <id> (see BLOCK_SEQUENCE_BITS).
block_wire_id() {
    local id="$1"
    [[ "$id" =~ ^[0-9]+$ ]] || { die "a block id is a number, not '$id'"; return 1; }
    local pid="${WINTER_BLOCK_PID:-$$}"
    printf '%s' "$(( (pid << BLOCK_SEQUENCE_BITS) | (id & BLOCK_SEQUENCE_MASK) ))"
}

# Open a live block and print its escape.
# Usage: winter_open_block <id> <mime> <spec-json> [fallback]
# The caller picks the id (1, 2, 3, ...) and reuses it for the matching
# patch/close calls: the same correlation the wire protocol requires, with
# no hidden state to lose across command substitutions. It is scoped by
# the calling process on the wire, so another script's 1 is another block. `spec` must be valid
# JSON: a string for text mimes, an object/array for structured ones (vega,
# JSON). Outside Winter the fallback is printed once and the later
# patch/close calls are no-ops.
winter_open_block() {
    local id mime="$2" spec="$3" fallback="${4:-}"
    [[ -n "${1:-}" ]] || die "open requires a block id"
    id="$(block_wire_id "$1")" || return 1
    if ! winter_supported; then
        printf '%s' "$fallback"
        return 0
    fi
    local payload
    payload="$(printf '%s' "$spec" | base64 | tr -d '\n')"
    printf '%s%s;%s;id=%s,mime=%s;%s%s' \
        "$OSC_START" "$OSC_NUMBER" "$VERB_OPEN" \
        "$id" "$(param_escape "$mime")" "$payload" "$ST"
}

# Patch a live block with an RFC 6902 operation array.
# Usage: winter_patch_block <id> <patch-json>
winter_patch_block() {
    local id patch="$2"
    [[ -n "${1:-}" ]] || die "patch requires a block id"
    id="$(block_wire_id "$1")" || return 1
    if ! winter_supported; then
        return 0
    fi
    local payload
    payload="$(printf '%s' "$patch" | base64 | tr -d '\n')"
    printf '%s%s;%s;id=%s;%s%s' \
        "$OSC_START" "$OSC_NUMBER" "$VERB_PATCH" "$id" "$payload" "$ST"
}

# Close a live block; the terminal freezes its last state.
# Usage: winter_close_block <id>
winter_close_block() {
    local id
    [[ -n "${1:-}" ]] || die "close requires a block id"
    id="$(block_wire_id "$1")" || return 1
    if ! winter_supported; then
        return 0
    fi
    printf '%s%s;%s;id=%s%s' \
        "$OSC_START" "$OSC_NUMBER" "$VERB_CLOSE" "$id" "$ST"
}

# =============================================================================
# Helpers
# =============================================================================

# JSON-escape a string for embedding inside a double-quoted JSON value. Covers
# the characters that appear in HTML/SVG/Markdown; binary data takes the base64
# path and never reaches here.
json_escape() {
    local s="$1" out="" char escaped
    # Whole-string replacements: walking the text a character at a time costs
    # time quadratic in its length, minutes for a 100 KB SVG.
    s="${s//\\/\\\\}"
    s="${s//\"/\\\"}"
    s="${s//$'\n'/\\n}"
    s="${s//$'\r'/\\r}"
    s="${s//$'\t'/\\t}"
    s="${s//$'\b'/\\b}"
    s="${s//$'\f'/\\f}"
    # Any other control character is rare: only then is the text walked.
    if [[ "$s" != *[[:cntrl:]]* ]]; then
        printf '%s' "$s"
        return
    fi
    while [[ -n "$s" ]]; do
        char="${s:0:1}"
        s="${s:1}"
        if [[ "$char" == [[:cntrl:]] ]]; then
            printf -v escaped '\\u%04x' "'$char"
            out+="$escaped"
        else
            out+="$char"
        fi
    done
    printf '%s' "$out"
}

# Percent-encode the characters a parameter value cannot carry as they are:
# the framing's own separators (`;` `,` `=` and `%`) and control characters,
# which would split the frame or end the escape early.
param_escape() {
    local s="$1" out="" i char escaped
    for (( i=0; i<${#s}; i++ )); do
        char="${s:i:1}"
        case "$char" in
            ';' | ',' | '=' | '%')
                printf -v escaped '%%%02X' "'$char"
                out+="$escaped"
                ;;
            *)
                if [[ "$char" == [[:cntrl:]] ]]; then
                    printf -v escaped '%%%02X' "'$char"
                    out+="$escaped"
                else
                    out+="$char"
                fi
                ;;
        esac
    done
    printf '%s' "$out"
}

read_source() {
    local src="${1:--}"
    if [[ "$src" == "-" ]]; then
        cat
    else
        [[ -r "$src" ]] || die "cannot read: $src"
        cat "$src"
    fi
}

image_mime_for() {
    case "${1##*.}" in
        png) printf 'image/png' ;;
        jpg | jpeg) printf 'image/jpeg' ;;
        gif) printf 'image/gif' ;;
        webp) printf 'image/webp' ;;
        svg) printf 'image/svg+xml' ;;
        *) printf 'application/octet-stream' ;;
    esac
}

usage() {
    cat >&2 <<'EOF'
usage: client.sh <command> [args]
  svg       <file|->     emit an SVG document
  html      <file|->     emit an HTML fragment
  markdown  <file|->     emit Markdown
  latex     <file|->     emit a LaTeX expression
  image     <file>       emit a raster image (mime from extension)
  open      <id> <mime> <spec-json> [fallback]
                        open a live block
  patch     <id> <patch-json>
                        apply an RFC 6902 patch to a live block
  close     <id>         close a live block
  supported              print yes/no for TBP capability
EOF
}

# =============================================================================
# CLI
# =============================================================================

main() {
    local command="${1:-}"
    shift || true
    case "$command" in
        svg) winter_emit_svg "$@" ;;
        html) winter_emit_html "$@" ;;
        markdown | md) winter_emit_markdown "$@" ;;
        latex) winter_emit_latex "$@" ;;
        image) winter_emit_image "$@" ;;
        open) winter_open_block "$@" ;;
        patch) winter_patch_block "$@" ;;
        close) winter_close_block "$@" ;;
        supported) winter_supported && echo yes || echo no ;;
        *) usage; return 2 ;;
    esac
}

if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
    set -euo pipefail
    # One process per call: the blocks are the calling script's.
    WINTER_BLOCK_PID="${WINTER_BLOCK_PID:-$PPID}"
    main "$@"
fi

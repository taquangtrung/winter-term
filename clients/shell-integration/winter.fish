# winter.fish - OSC 133 shell integration for fish.
#
# Source this from ~/.config/fish/config.fish:
#     test -r /usr/share/winter-term/shell-integration/winter.fish
#         and source /usr/share/winter-term/shell-integration/winter.fish
#
# It emits the marks Winter uses to cut the stream into command blocks:
#   OSC 133;A  before the prompt        OSC 133;C  before command output
#   OSC 133;B  at the end of the prompt OSC 133;D;<code> after the command
#
# Without these marks Winter still works, but the whole session is one rolling
# block: no per-command navigation, folding, or exit codes.
#
# fish emits OSC 7 itself in most builds, but not in every configuration, so
# this reports it too. A duplicate report is harmless: the terminal just
# overwrites the directory it already had.

# =============================================================================
# Guards
# =============================================================================

# Only mark up interactive shells running under Winter, and only once: a
# re-source must not wrap fish_prompt in a second copy of itself.
if not status is-interactive
    return 0
end

if not set -q WINTER; and test "$TERM_PROGRAM" != winter
    return 0
end

if set -q WINTER_SHELL_INTEGRATION
    return 0
end
set -g WINTER_SHELL_INTEGRATION 1

# =============================================================================
# Marks
# =============================================================================

# Report the working directory so Winter can show it and offer it to the
# "recent directories" palette. `string escape --style=url` percent-encodes
# per byte, which is what a URI needs: a non-ASCII directory has to encode as
# its UTF-8 bytes (%C3%A9), not its codepoint. It also escapes `/`, so the
# path is encoded segment by segment and rejoined.
function __winter_report_cwd
    set -l encoded (string split / -- $PWD | string escape --style=url | string join /)
    printf '\e]7;file://%s%s\e\\' (hostname 2>/dev/null) "$encoded"
end

# Fires after the user submits a command, before it runs.
function __winter_preexec --on-event fish_preexec
    printf '\e]133;C\e\\'
end

# Fires once the command has finished. `$status` is the command's, because this
# event handler is the first thing to run after it.
function __winter_postexec --on-event fish_postexec
    printf '\e]133;D;%s\e\\' $status
end

# =============================================================================
# Installation
# =============================================================================

# A and B bracket the prompt itself, so the prompt function is wrapped rather
# than hooked: A before it draws, B after, leaving the marks tight around the
# prompt text no matter what the user's prompt does.
#
# `functions --copy` snapshots the existing definition, so a prompt installed
# by starship, tide, or the user's own config keeps working unchanged.
#
# The `functions -q` first is load-bearing: fish autoloads a function on
# lookup, and `--copy` does not count as one. Without it the copy silently
# finds nothing, and the wrapper below *replaces* the prompt instead of
# wrapping it, leaving the user with a bare prompt and no A mark.
functions -q fish_prompt
functions --copy fish_prompt __winter_inner_fish_prompt

function fish_prompt
    printf '\e]133;A\e\\'
    __winter_report_cwd
    __winter_inner_fish_prompt
    printf '\e]133;B\e\\'
end

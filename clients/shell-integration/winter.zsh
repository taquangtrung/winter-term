# winter.zsh - OSC 133 shell integration for zsh.
#
# Source this from ~/.zshrc:
#     [ -r /usr/share/winter-term/shell-integration/winter.zsh ] && \
#         . /usr/share/winter-term/shell-integration/winter.zsh
#
# It emits the marks Winter uses to cut the stream into command blocks:
#   OSC 133;A  before the prompt        OSC 133;C  before command output
#   OSC 133;B  at the end of the prompt OSC 133;D;<code> after the command
# plus OSC 7 so Winter tracks the working directory.
#
# Without these marks Winter still works, but the whole session is one rolling
# block: no per-command navigation, folding, or exit codes.
#
# Uses zsh's precmd/preexec hook arrays rather than a DEBUG trap, and zsh's own
# `chpwd` for the directory report, so nothing here fights a prompt framework
# (powerlevel10k, starship, prezto) that installs its own hooks.

# =============================================================================
# Guards
# =============================================================================

# Only mark up interactive shells running under Winter, and only once: a
# re-source must not append a second copy of the hooks.
[[ -o interactive ]] || return 0

if [[ -z "${WINTER:-}" && "${TERM_PROGRAM:-}" != "winter" ]]; then
    return 0
fi

if [[ -n "${WINTER_SHELL_INTEGRATION:-}" ]]; then
    return 0
fi
typeset -g WINTER_SHELL_INTEGRATION=1

# =============================================================================
# Marks
# =============================================================================

# Percent-encode $PWD for the OSC 7 URI. Everything outside the unreserved set
# is escaped, so a directory containing a space, a quote, or a non-ASCII
# character survives the round trip.
#
# LC_ALL=C makes the index below walk *bytes* rather than characters. A URI
# escapes bytes, so a non-ASCII directory has to encode as its UTF-8 bytes
# (%C3%A9), not as its codepoint (%E9), which is not valid UTF-8 on its own and
# cannot be decoded back.
#
# `printf -v` assigns without a subshell. This runs once per byte of $PWD on
# every prompt, so a `$( )` here would add a fork per character to the latency
# of drawing the prompt.
__winter_encode_cwd() {
    local LC_ALL=C
        # Not named `path`: in zsh that is a special array tied to $PATH, so a
    # local of that name iterates PATH entries instead of the string's bytes.
    local cwd="$PWD" out="" index char hex
    for (( index = 1; index <= ${#cwd}; index++ )); do
        char="${cwd[index]}"
        case "$char" in
            [-_.~a-zA-Z0-9/])
                out+="$char"
                ;;
            *)
                printf -v hex '%%%02X' "'$char"
                out+="$hex"
                ;;
        esac
    done
    printf '%s' "$out"
}

# Report the working directory so Winter can show it and offer it to the
# "recent directories" palette.
__winter_report_cwd() {
    printf '\e]7;file://%s%s\e\\' "${HOST:-}" "$(__winter_encode_cwd)"
}

# Runs before each prompt: close the previous command with its exit status,
# then open the new prompt.
__winter_precmd() {
    # Not named `status`: in zsh that is a special parameter aliasing `$?`, and
    # declaring it local makes this hook fail silently, taking every mark it
    # emits with it.
    local exit_status=$?
    # Skip the D mark before the very first prompt of the session: there is no
    # preceding command, and reporting one would open an empty block.
    if [[ -n "${__winter_command_running:-}" ]]; then
        printf '\e]133;D;%s\e\\' "$exit_status"
        unset __winter_command_running
    fi
    printf '\e]133;A\e\\'
    __winter_report_cwd
}

# Runs after the user submits a command, before it executes.
__winter_preexec() {
    typeset -g __winter_command_running=1
    printf '\e]133;C\e\\'
}

# =============================================================================
# Installation
# =============================================================================

autoload -Uz add-zsh-hook
add-zsh-hook precmd __winter_precmd
add-zsh-hook preexec __winter_preexec

# The B mark closes the prompt and opens the typed command, so it goes at the
# very end of PS1. `%{ %}` marks it zero-width; without them zsh miscounts the
# prompt width and redraws long lines wrongly.
PS1="${PS1}%{$(printf '\e]133;B\e\\')%}"

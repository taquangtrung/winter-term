# winter.bash - OSC 133 shell integration for bash.
#
# Source this from ~/.bashrc:
#     [ -r /usr/share/winter-term/shell-integration/winter.bash ] && \
#         . /usr/share/winter-term/shell-integration/winter.bash
#
# It emits the marks Winter uses to cut the stream into command blocks:
#   OSC 133;A  before the prompt        OSC 133;C  before command output
#   OSC 133;B  at the end of the prompt OSC 133;D;<code> after the command
# plus OSC 7 so Winter tracks the working directory.
#
# Without these marks Winter still works, but the whole session is one rolling
# block: no per-command navigation, folding, or exit codes.
#
# Deliberately does not `set -euo pipefail`: this is sourced into an
# interactive shell, where those options would change the user's shell
# behavior and abort the session on the first failing command.
#
# The marks are ignored by terminals that do not understand them, so this is
# safe to source unconditionally. It no-ops outside Winter anyway, so other
# terminals' own integrations are left alone.

# =============================================================================
# Guards
# =============================================================================

# Only mark up interactive shells running under Winter, and only once: a
# re-source (a nested shell, a reloaded rc file) must not stack a second copy
# of the hooks onto PROMPT_COMMAND or PS1.
case "$-" in
    *i*) ;;
    *) return 0 ;;
esac

if [ -z "${WINTER:-}" ] && [ "${TERM_PROGRAM:-}" != "winter" ]; then
    return 0
fi

if [ -n "${WINTER_SHELL_INTEGRATION:-}" ]; then
    return 0
fi
WINTER_SHELL_INTEGRATION=1

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
    local cwd="$PWD" out="" index char hex
    for (( index = 0; index < ${#cwd}; index++ )); do
        char="${cwd:index:1}"
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
    printf '\033]7;file://%s%s\033\\' "${HOSTNAME:-}" "$(__winter_encode_cwd)"
}

# The B mark, in PS1-escape form. `\[ \]` marks it zero-width; without them
# bash miscounts the prompt width and redraws long lines wrongly.
__WINTER_PS1_MARK='\[\033]133;B\033\\\]'

# Append the B mark to PS1 unless it is already there.
#
# Re-applied on every prompt rather than once at source time: a prompt
# framework that rebuilds PS1 each cycle (conda's `(base)` prefix, powerline,
# starship) overwrites whatever was appended at startup, silently taking the
# mark with it and leaving every command's text outside its own block.
__winter_mark_ps1() {
    if [[ "$PS1" != *"$__WINTER_PS1_MARK"* ]]; then
        PS1="${PS1}${__WINTER_PS1_MARK}"
    fi
}

# Runs before each prompt: close the previous command with its exit status,
# then open the new prompt.
__winter_precmd() {
    local status=$?
    # Skip the D mark before the very first prompt of the session: there is no
    # preceding command, and reporting one would open an empty block.
    if [ -n "${__winter_command_running:-}" ]; then
        printf '\033]133;D;%s\033\\' "$status"
        unset __winter_command_running
    fi
    printf '\033]133;A\033\\'
    __winter_report_cwd
    return $status
}

# Runs before each command executes, via the DEBUG trap. The trap also fires
# for the commands inside PROMPT_COMMAND itself, so it marks output start only
# once per prompt and only for a command the user actually submitted.
__winter_preexec() {
    if [ -n "${__winter_command_running:-}" ]; then
        return 0
    fi
    # Every helper in this file, not an enumerated few: the DEBUG trap fires
    # for the commands inside PROMPT_COMMAND too, and one unlisted helper is
    # enough to emit C before the prompt has even been drawn, which puts the
    # output mark ahead of the prompt mark and scrambles the block.
    case "$BASH_COMMAND" in
        __winter_*) return 0 ;;
    esac
    __winter_command_running=1
    printf '\033]133;C\033\\'
}

# =============================================================================
# Installation
# =============================================================================

# Two hooks, at opposite ends of PROMPT_COMMAND. `__winter_precmd` goes first
# so it reads `$?` before any other hook can clobber it; `__winter_mark_ps1`
# goes last so it re-marks PS1 after any framework in between has rebuilt it.
# Neither replaces what the user already had.
if [[ "$(declare -p PROMPT_COMMAND 2>/dev/null)" == "declare -a"* ]]; then
    # bash 5.1+ allows PROMPT_COMMAND to be an array of commands.
    PROMPT_COMMAND=(__winter_precmd "${PROMPT_COMMAND[@]}" __winter_mark_ps1)
elif [ -n "${PROMPT_COMMAND:-}" ]; then
    PROMPT_COMMAND="__winter_precmd; ${PROMPT_COMMAND}; __winter_mark_ps1"
else
    PROMPT_COMMAND="__winter_precmd; __winter_mark_ps1"
fi

__winter_mark_ps1

# Chain onto any DEBUG trap already installed (bash-preexec, conda, atuin)
# rather than replacing it: `trap ... DEBUG` overwrites, and silently breaking
# another tool's preexec would be a worse bug than missing our own mark.
__winter_existing_debug_trap="$(trap -p DEBUG)"
if [ -n "$__winter_existing_debug_trap" ]; then
    # `trap -p` prints `trap -- 'body' DEBUG`; recover just the body.
    __winter_existing_debug_trap="${__winter_existing_debug_trap#trap -- \'}"
    __winter_existing_debug_trap="${__winter_existing_debug_trap%\' DEBUG}"
    trap "__winter_preexec; ${__winter_existing_debug_trap}" DEBUG
else
    trap '__winter_preexec' DEBUG
fi
unset __winter_existing_debug_trap

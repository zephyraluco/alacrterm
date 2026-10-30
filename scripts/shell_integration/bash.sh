# POSIX sh syntax; Bash hooks are enabled when running inside Bash.
# Source in an interactive shell. No startup files are modified here.
case $- in *i*) ;; *) return ;; esac
[ "${__alacrterm_integrated-}" != 1 ] || return
__alacrterm_integrated=1
__alacrterm_ready=0
__alacrterm_esc=$(printf '\033')
__alacrterm_bel=$(printf '\007')
__alacrterm_cr=$(printf '\r')
__alacrterm_nl='
'
__alacrterm_marker='\[\e]633;B\a\]'

__alacrterm_signature() {
    printf '%s|' "${PATH-}"
    alias
    if [ -n "${BASH_VERSION-}" ]; then builtin declare -F; fi
}
__alacrterm_clock() {
    __alacrterm_now=${SECONDS-}
    case $__alacrterm_now in
        ''|*[!0-9]*) __alacrterm_now=$(date +%s 2>/dev/null || :) ;;
    esac
    case $__alacrterm_now in ''|*[!0-9]*) __alacrterm_now=0 ;; esac
}
__alacrterm_command_names() {
    if [ -n "${BASH_VERSION-}" ]; then
        builtin compgen -c
    else
        # POSIX has no portable function/builtin enumeration interface.
        for __alacrterm_builtin in alias bg break cd command continue eval exec exit export false fc fg getopts hash jobs kill printf pwd read readonly return set shift test times trap true type ulimit umask unalias unset wait; do
            if command -v "$__alacrterm_builtin" >/dev/null 2>&1; then
                printf '%s\n' "$__alacrterm_builtin"
            fi
        done
        alias | sed 's/^alias //; s/=.*//'
        (
            IFS=:
            set -f
            for __alacrterm_dir in ${PATH-}; do
                [ -n "$__alacrterm_dir" ] || __alacrterm_dir=.
                set +f
                for __alacrterm_file in "$__alacrterm_dir"/*; do
                    if [ -f "$__alacrterm_file" ] && [ -x "$__alacrterm_file" ]; then
                        printf '%s\n' "${__alacrterm_file##*/}"
                    fi
                done
                set -f
            done
        )
    fi
}
alacrterm_refresh_completions() {
    # Canonical order also avoids retransmitting an unchanged set when PATH
    # order changes. Keep data separate from printf's format string.
    __alacrterm_new_inventory=$(__alacrterm_command_names | LC_ALL=C command -p sort -u | LC_ALL=C command -p awk '
        /^[a-zA-Z0-9_.:+-]+$/ { printf "\033]633;Command;%s\007", $0 }
    ')
    if [ "${__alacrterm_inventory+x}" != x ] ||
        [ "$__alacrterm_new_inventory" != "$__alacrterm_inventory" ]; then
        printf '\033]633;CommandReset\007%s' "$__alacrterm_new_inventory"
        __alacrterm_inventory=$__alacrterm_new_inventory
    fi
    if [ "$#" -ge 1 ]; then
        __alacrterm_inventory_signature=$1
    else
        __alacrterm_inventory_signature=$(__alacrterm_signature)
    fi
    if [ "$#" -ge 2 ]; then
        __alacrterm_last_refresh=$2
    else
        __alacrterm_clock
        __alacrterm_last_refresh=$__alacrterm_now
    fi
}
__alacrterm_before_prompt() {
    __alacrterm_status=$?
    __alacrterm_ready=0
    return "$__alacrterm_status"
}
__alacrterm_prompt() {
    __alacrterm_clock
    __alacrterm_current_signature=$(__alacrterm_signature)
    case ${__alacrterm_last_refresh-} in ''|*[!0-9]*) __alacrterm_last_refresh=0 ;; esac
    if [ "$__alacrterm_current_signature" != "${__alacrterm_inventory_signature-}" ] ||
        [ "$((__alacrterm_now - __alacrterm_last_refresh))" -ge 30 ]; then
        alacrterm_refresh_completions "$__alacrterm_current_signature" "$__alacrterm_now"
    fi
    __alacrterm_history_sync
    case ${PWD-} in
        *"$__alacrterm_esc"*|*"$__alacrterm_bel"*|*"$__alacrterm_cr"*|*"$__alacrterm_nl"*) printf '\033]633;AlacrtermCwd;\007' ;;
        *) printf '\033]633;AlacrtermCwd;%s\007' "${PWD-}" ;;
    esac
    printf '\033]633;D;%s\007\033]633;A\007' "${__alacrterm_status:-0}"
    if [ -n "${BASH_VERSION-}" ]; then
        case ${PS1-} in *"$__alacrterm_marker") ;; *) PS1="${PS1-}$__alacrterm_marker" ;; esac
    fi
    __alacrterm_ready=1
    return "${__alacrterm_status:-0}"
}
__alacrterm_preexec() {
    [ "$__alacrterm_ready" = 1 ] || return 0
    # Completion functions and bind -x callbacks run while the user is editing.
    [ "${COMP_LINE+x}${READLINE_LINE+x}" = '' ] || return 0
    case ${BASH_COMMAND-} in __alacrterm_before_prompt) return 0 ;; esac
    __alacrterm_ready=0
    printf '\033]633;C\007'
}
__alacrterm_run_prompt_commands() {
    # Constant Bash-only code, parsed only when a user supplied an array hook.
    eval 'for __alacrterm_user_prompt_command in "${__alacrterm_user_prompt_commands[@]}"; do
        eval "$__alacrterm_user_prompt_command"
    done'
}

# History is read from this shell, never from a guessed history file.
__alacrterm_history_collect() {
    # Bash excludes the current event from expansion in interactive functions.
    # A worker-only sentinel makes the last accepted event addressable.
    builtin history -s '__alacrterm_history_boundary__'
    __alacrterm_history_index=$1
    while [ "$__alacrterm_history_index" -le "$2" ]; do
        # history -p resolves the event without executing it, preserving literal
        # whitespace and multiline boundaries. Run only in this worker's copy.
        __alacrterm_history_command=$(builtin history -p "!$__alacrterm_history_index" 2>/dev/null) || __alacrterm_history_command=
        __alacrterm_history_emit "$__alacrterm_history_index" "$__alacrterm_history_command"
        __alacrterm_history_index=$((__alacrterm_history_index + 1))
    done
}
__alacrterm_history_sync() {
    [ -n "${BASH_VERSION-}" ] && [ "${ALACRTERM_HISTORY_ENABLED:-1}" != 0 ] || return 0
    __alacrterm_history_row=$(HISTTIMEFORMAT= builtin history 1)
    [ "$__alacrterm_history_row" != "$__alacrterm_history_anchor" ] || [ -z "$__alacrterm_history_last" ] || return 0
    __alacrterm_history_number=${__alacrterm_history_row#"${__alacrterm_history_row%%[![:space:]]*}"}
    __alacrterm_history_number=${__alacrterm_history_number%%[!0-9]*}
    __alacrterm_history_number=${__alacrterm_history_number:-0}
    __alacrterm_history_mode=append
    __alacrterm_history_first=$(HISTTIMEFORMAT= builtin history 1000)
    __alacrterm_history_first=${__alacrterm_history_first#"${__alacrterm_history_first%%[![:space:]]*}"}
    __alacrterm_history_first=${__alacrterm_history_first%%[!0-9]*}
    __alacrterm_history_first=${__alacrterm_history_first:-1}
    if [ -z "$__alacrterm_history_last" ] || [ "$__alacrterm_history_number" -le "$__alacrterm_history_last" ]; then
        __alacrterm_history_mode=snapshot
    elif [ "$__alacrterm_history_first" -le "$__alacrterm_history_last" ]; then
        __alacrterm_history_first=$((__alacrterm_history_last + 1))
    fi
    __alacrterm_history_last=$__alacrterm_history_number
    __alacrterm_history_anchor=$__alacrterm_history_row
    __alacrterm_history_start "$__alacrterm_history_mode" "$__alacrterm_history_first" "$__alacrterm_history_number"
}
__alacrterm_history_init

alacrterm_refresh_completions
printf '\033]633;AlacrtermShell;posix\007'
if [ -n "${BASH_VERSION-}" ]; then
    case $(builtin declare -p PROMPT_COMMAND 2>/dev/null) in
        'declare -a '*)
            eval '__alacrterm_user_prompt_commands=( "${PROMPT_COMMAND[@]}" )'
            # Older Bash only executes the first element of an array hook.
            # Normalize it while retaining all user commands and their order.
            unset PROMPT_COMMAND
            PROMPT_COMMAND='__alacrterm_before_prompt; __alacrterm_run_prompt_commands; __alacrterm_prompt'
            ;;
        *) PROMPT_COMMAND="__alacrterm_before_prompt${PROMPT_COMMAND:+; $PROMPT_COMMAND}; __alacrterm_prompt" ;;
    esac
    # Select a hook, never reject a Bash version. PS0 exists starting at 4.4;
    # newer Bash also hides inherited DEBUG traps while sourcing a file, so
    # use PS0 there rather than risking replacement of a user's debugger trap.
    __alacrterm_major=${BASH_VERSION%%.*}
    __alacrterm_minor=${BASH_VERSION#*.}
    __alacrterm_minor=${__alacrterm_minor%%.*}
    if [ "$__alacrterm_major" -gt 4 ] ||
        { [ "$__alacrterm_major" -eq 4 ] && [ "$__alacrterm_minor" -ge 4 ]; }; then
        PS0='\e]633;C\a'"${PS0-}"
    elif [ -z "$(trap -p DEBUG)" ]; then
        case $- in
            *T*) : ;; # Preserve function tracing; no safe portable preexec hook.
            *) trap '__alacrterm_preexec' DEBUG ;;
        esac
    fi
else
    # POSIX defines no preexec hook. Where PS1 command substitution is available,
    # report prompt/cwd without guessing execution events or replacing traps.
    PS1='$(__alacrterm_status=$?; __alacrterm_prompt)'"${PS1-}$(printf '\033]633;B\007')"
fi

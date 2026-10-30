# Shared by the embedded Bash/Zsh integrations. Never evaluate history text.
__alacrterm_history_init() {
    __alacrterm_history_session="${BASHPID:-$$}-${RANDOM:-0}-${RANDOM:-0}"
    __alacrterm_history_batch=0
    __alacrterm_history_pid=
    __alacrterm_history_last=
    __alacrterm_history_anchor=
    printf '\033]633;History;session;%s\007' "$__alacrterm_history_session"
}
__alacrterm_history_emit() {
    # Locale is C in the worker: length and encoding operate on UTF-8 bytes.
    [ "${#2}" -le 4096 ] || return 0
    case $2 in ''|*[[:cntrl:]]*) return 0 ;; esac
    case $2 in *[![:space:]]*) ;; *) return 0 ;; esac
    __alacrterm_history_text=$2
    __alacrterm_history_hex=
    while [ -n "$__alacrterm_history_text" ]; do
        __alacrterm_history_char=${__alacrterm_history_text%"${__alacrterm_history_text#?}"}
        __alacrterm_history_text=${__alacrterm_history_text#?}
        # printf's quoted character notation converts a literal byte to a number.
        printf -v __alacrterm_history_byte '%02x' "'$__alacrterm_history_char"
        __alacrterm_history_hex=$__alacrterm_history_hex$__alacrterm_history_byte
    done
    printf '\033]633;History;entry;%s;%s;%s;%s\007' "$__alacrterm_history_session" "$__alacrterm_history_batch" "$1" "$__alacrterm_history_hex"
    __alacrterm_history_count=$((__alacrterm_history_count + 1))
}
__alacrterm_history_start() {
    # One worker per shell. Defer newer changes until the next prompt if busy.
    __alacrterm_history_batch=$((__alacrterm_history_batch + 1))
    (
        export LC_ALL=C
        __alacrterm_history_count=0
        printf '\033]633;History;begin;%s;%s;%s\007' "$__alacrterm_history_session" "$__alacrterm_history_batch" "$1"
        __alacrterm_history_collect "$2" "$3"
        printf '\033]633;History;end;%s;%s;%s\007' "$__alacrterm_history_session" "$__alacrterm_history_batch" "$__alacrterm_history_count"
    ) </dev/null 2>/dev/null &
    __alacrterm_history_pid=$!
    # Suppress job notifications without changing the user's job-control options.
    disown "$__alacrterm_history_pid" 2>/dev/null || :
}

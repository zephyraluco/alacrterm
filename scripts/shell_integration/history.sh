# Shared by the embedded Bash/Zsh integrations. Never evaluate history text.
__alacrterm_history_init() {
    __alacrterm_history_session="${BASHPID:-$$}-${RANDOM:-0}-${RANDOM:-0}"
    __alacrterm_history_batch=0
    __alacrterm_history_pid=
    __alacrterm_history_last=
    __alacrterm_history_anchor=
    __alacrterm_history_announce
}
__alacrterm_history_announce() {
    printf '\033]633;History;session;%s\007' "$__alacrterm_history_session"
}
__alacrterm_history_emit() {
    # Locale is C in the worker: length and encoding operate on UTF-8 bytes.
    [ "${#2}" -le 4096 ] || return 0
    case $2 in ''|*[[:cntrl:]]*) return 0 ;; esac
    case $2 in *[![:space:]]*) ;; *) return 0 ;; esac
    # Encode in the background with POSIX byte tools. A shell loop per byte
    # makes large histories exceed the receiver's batch timeout.
    __alacrterm_history_hex=$(printf '%s' "$2" | command -p od -An -v -tx1 | command -p tr -d ' \n')
    printf '\033]633;History;entry;%s;%s;%s;%s\007' "$__alacrterm_history_session" "$__alacrterm_history_batch" "$1" "$__alacrterm_history_hex"
    __alacrterm_history_count=$((__alacrterm_history_count + 1))
}
__alacrterm_history_start() {
    # Each worker inherits the accepted history at this prompt. Serialize
    # transmission behind the preceding worker without blocking the prompt.
    __alacrterm_history_batch=$((__alacrterm_history_batch + 1))
    {
      __alacrterm_history_pid=$(
        {
        export LC_ALL=C
        if [ -n "$__alacrterm_history_pid" ]; then
            __alacrterm_history_wait=0
            while kill -0 "$__alacrterm_history_pid" 2>/dev/null; do
                # A closing shell may leave its completed worker as a zombie
                # briefly. Do not retain a queued worker indefinitely.
                if [ "$((__alacrterm_history_wait % 5))" -eq 0 ]; then
                    case $(command -p ps -p "$__alacrterm_history_pid" -o stat= 2>/dev/null) in (*Z*) break ;; esac
                fi
                [ "$__alacrterm_history_wait" -lt 1500 ] || exit 0
                __alacrterm_history_wait=$((__alacrterm_history_wait + 1))
                sleep .02
            done
        fi
        __alacrterm_history_count=0
        printf '\033]633;History;begin;%s;%s;%s\007' "$__alacrterm_history_session" "$__alacrterm_history_batch" "$1"
        __alacrterm_history_collect "$2" "$3"
        printf '\033]633;History;end;%s;%s;%s\007' "$__alacrterm_history_session" "$__alacrterm_history_batch" "$__alacrterm_history_count"
        } </dev/null >&3 3>&- 2>/dev/null &
      # Launch inside the substitution so the interactive parent keeps $! for
      # user jobs. FD 3 carries original output instead of the PID pipe.
      __alacrterm_history_child=$!
      disown "$__alacrterm_history_child" 2>/dev/null || :
      printf '%s' "$__alacrterm_history_child"
      )
    } 3>&1
}

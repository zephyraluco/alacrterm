# Source in an interactive Zsh. Existing hooks remain installed.
[[ -o interactive ]] || return
[[ ${__alacrterm_integrated-} == 1 ]] && return
__alacrterm_integrated=1
autoload -Uz add-zsh-hook
__alacrterm_precmd() {
    local alacrterm_status=$?
    local alacrterm_signature="$PATH|${(ok)aliases}|${(ok)functions}"
    if [[ $alacrterm_signature != "${__alacrterm_inventory_signature-}" ]] || (( SECONDS - ${__alacrterm_last_refresh:-0} >= 30 )); then
        alacrterm_refresh_completions
    fi
    __alacrterm_history_sync
    case "$PWD" in
        *$'\e'*|*$'\a'*|*$'\n'*|*$'\r'*) printf '\033]633;AlacrtermCwd;\007' ;;
        *) printf '\033]633;AlacrtermCwd;%s\007' "$PWD" ;;
    esac
    printf '\033]633;D;%s\007\033]633;A\007' "$alacrterm_status"
    local marker=$'%{\e]633;B\a%}'
    [[ $PROMPT == *$marker ]] || PROMPT+=$marker
    return "$alacrterm_status"
}
__alacrterm_preexec() { printf '\033]633;C\007'; }
add-zsh-hook precmd __alacrterm_precmd
add-zsh-hook preexec __alacrterm_preexec
alacrterm_refresh_completions() {
    rehash
    printf '\033]633;CommandReset\007'
    local alacrterm_name
    for alacrterm_name in ${(k)aliases} ${(k)functions} ${(k)builtins} ${(k)commands}; do
        case "$alacrterm_name" in ''|*[!a-zA-Z0-9_.:+-]*) continue ;; esac
        printf '\033]633;Command;%s\007' "$alacrterm_name"
    done
    __alacrterm_inventory_signature="$PATH|${(ok)aliases}|${(ok)functions}"
    __alacrterm_last_refresh=$SECONDS
}
__alacrterm_history_collect() {
    local index command
    for index in ${(on)${(k)history}}; do
        (( index >= $1 && index <= $2 )) || continue
        command=${history[$index]}
        __alacrterm_history_emit "$index" "$command"
    done
}
__alacrterm_history_sync() {
    [[ ${ALACRTERM_HISTORY_ENABLED:-1} != 0 ]] || return 0
    [[ -n $__alacrterm_history_pid ]] && kill -0 "$__alacrterm_history_pid" 2>/dev/null && return 0
    local -a keys
    keys=(${(On)${(k)history}})
    local last=${keys[1]:-0} first=${keys[1000]:-${keys[-1]:-1}} mode=append
    local anchor=${history[$last]-}
    [[ $last != "$__alacrterm_history_last" || $anchor != "$__alacrterm_history_anchor" ]] || return 0
    if [[ -z $__alacrterm_history_last ]] || (( last <= __alacrterm_history_last )); then
        mode=snapshot
    elif (( first <= __alacrterm_history_last )); then
        first=$((__alacrterm_history_last + 1))
    fi
    __alacrterm_history_last=$last
    __alacrterm_history_anchor=$anchor
    __alacrterm_history_start "$mode" "$first" "$last"
}
zmodload zsh/parameter
__alacrterm_history_init

alacrterm_refresh_completions
printf '\033]633;AlacrtermShell;posix\007'

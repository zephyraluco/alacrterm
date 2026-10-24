# Source in an interactive Zsh. Existing hooks remain installed.
[[ -o interactive ]] || return
[[ ${__alacrterm_integrated-} == 1 ]] && return
__alacrterm_integrated=1
autoload -Uz add-zsh-hook
__alacrterm_precmd() {
    local alacrterm_status=$?
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
}
alacrterm_refresh_completions
printf '\033]633;AlacrtermShell;posix\007'

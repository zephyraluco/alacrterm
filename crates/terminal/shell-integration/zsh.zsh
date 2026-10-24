# Source in an interactive Zsh. Existing hooks remain installed.
[[ -o interactive ]] || return
[[ ${__alacrterm_integrated-} == 1 ]] && return
__alacrterm_integrated=1
autoload -Uz add-zsh-hook
__alacrterm_precmd() {
    local alacrterm_status=$?
    printf '\033]633;D;%s\007\033]633;A\007' "$alacrterm_status"
    local marker=$'%{\e]633;B\a%}'
    [[ $PROMPT == *$marker ]] || PROMPT+=$marker
    return "$alacrterm_status"
}
__alacrterm_preexec() { printf '\033]633;C\007'; }
add-zsh-hook precmd __alacrterm_precmd
add-zsh-hook preexec __alacrterm_preexec

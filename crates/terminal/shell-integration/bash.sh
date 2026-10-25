# Source in an interactive Bash. No startup files are modified.
[[ $- == *i* ]] || return
(( BASH_VERSINFO[0] > 4 || (BASH_VERSINFO[0] == 4 && BASH_VERSINFO[1] >= 4) )) || return
[[ ${__alacrterm_integrated-} == 1 ]] && return
__alacrterm_integrated=1
__alacrterm_prompt() {
    local alacrterm_status=$?
    local alacrterm_signature="$PATH|$(alias -p)|$(declare -F)"
    if [[ $alacrterm_signature != "${__alacrterm_inventory_signature-}" ]] || (( SECONDS - ${__alacrterm_last_refresh:-0} >= 30 )); then
        alacrterm_refresh_completions
    fi
    case "$PWD" in
        *$'\e'*|*$'\a'*|*$'\n'*|*$'\r'*) printf '\033]633;AlacrtermCwd;\007' ;;
        *) printf '\033]633;AlacrtermCwd;%s\007' "$PWD" ;;
    esac
    printf '\033]633;D;%s\007\033]633;A\007' "$alacrterm_status"
    local marker='\[\e]633;B\a\]'
    case "$PS1" in *"$marker") ;; *) PS1+="$marker" ;; esac
    return "$alacrterm_status"
}
if [[ $(declare -p PROMPT_COMMAND 2>/dev/null) == 'declare -a '* ]]; then
    PROMPT_COMMAND+=(__alacrterm_prompt)
else
    PROMPT_COMMAND="${PROMPT_COMMAND:+$PROMPT_COMMAND; }__alacrterm_prompt"
fi
PS0='\e]633;C\a'"${PS0-}"
# The shell supplies its own PATH, aliases, builtins and functions; nothing is executed.
alacrterm_refresh_completions() {
    printf '\033]633;CommandReset\007'
    local alacrterm_name
    while IFS= read -r alacrterm_name; do
        case "$alacrterm_name" in ''|*[!a-zA-Z0-9_.:+-]*) continue ;; esac
        printf '\033]633;Command;%s\007' "$alacrterm_name"
    done < <(compgen -c)
    __alacrterm_inventory_signature="$PATH|$(alias -p)|$(declare -F)"
    __alacrterm_last_refresh=$SECONDS
}
alacrterm_refresh_completions
printf '\033]633;AlacrtermShell;posix\007'

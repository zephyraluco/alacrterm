# Source in an interactive Bash. No startup files are modified.
[[ $- == *i* ]] || return
(( BASH_VERSINFO[0] > 4 || (BASH_VERSINFO[0] == 4 && BASH_VERSINFO[1] >= 4) )) || return
[[ ${__alacrterm_integrated-} == 1 ]] && return
__alacrterm_integrated=1
__alacrterm_prompt() {
    local alacrterm_status=$?
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

# Sourced during normal startup. No command is entered into the interactive editor.
case $- in *i*) ;; *) return ;; esac
[ "${ALACRTERM_REMOTE_INTEGRATION:-${LC_ALACRTERM_INTEGRATION:-0}}" = 1 ] || return
if [ -n "${BASH_VERSION-}" ]; then
    [ ! -r "$HOME/.local/share/alacrterm/shell/bash.sh" ] || . "$HOME/.local/share/alacrterm/shell/bash.sh" || :
elif [ -n "${ZSH_VERSION-}" ]; then
    [ ! -r "$HOME/.local/share/alacrterm/shell/zsh.zsh" ] || . "$HOME/.local/share/alacrterm/shell/zsh.zsh" || :
fi

# Do not propagate activation through nested SSH clients forwarding LC_*.
unset ALACRTERM_REMOTE_INTEGRATION LC_ALACRTERM_INTEGRATION

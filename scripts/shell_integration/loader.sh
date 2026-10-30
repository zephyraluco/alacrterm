# Sourced during normal startup. No command is entered into the interactive editor.
case $- in *i*) ;; *) return ;; esac
case ${ALACRTERM_REMOTE_INTEGRATION:-${LC_ALACRTERM_INTEGRATION:-0}} in
    1) ALACRTERM_HISTORY_ENABLED=1 ;;
    1-no-history) ALACRTERM_HISTORY_ENABLED=0 ;;
    *) return ;;
esac
if [ -n "${BASH_VERSION-}" ]; then
    [ ! -r "$HOME/.local/share/alacrterm/shell/bash.sh" ] || . "$HOME/.local/share/alacrterm/shell/bash.sh" || :
elif [ -n "${ZSH_VERSION-}" ]; then
    [ ! -r "$HOME/.local/share/alacrterm/shell/zsh.zsh" ] || . "$HOME/.local/share/alacrterm/shell/zsh.zsh" || :
fi

# Do not propagate activation through nested SSH clients forwarding LC_*.
unset ALACRTERM_REMOTE_INTEGRATION LC_ALACRTERM_INTEGRATION

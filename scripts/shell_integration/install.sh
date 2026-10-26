# Runs in a separate exec channel, never as a command in the interactive shell.
set -eu
umask 077
kind=$1
bash_script=$2
zsh_script=$3
loader=$4
zshdir=${5:-${ZDOTDIR:-$HOME}}
dir=$HOME/.local/share/alacrterm/shell
begin='# >>> alacrterm shell integration >>>'
end='# <<< alacrterm shell integration <<<'
block="$begin
[ ! -r \"\$HOME/.local/share/alacrterm/shell/loader.sh\" ] || . \"\$HOME/.local/share/alacrterm/shell/loader.sh\"
$end"
mkdir -p "$dir"
mkdir "$dir/.install-lock" 2>/dev/null || exit 1
tmp=
cleanup() { [ -z "$tmp" ] || rm -f -- "$tmp"; rmdir "$dir/.install-lock"; }
trap cleanup 0
trap 'exit 1' 1 2 15

check_entry() {
    file=$1
    # Preserve user-managed symlinks rather than replacing their target implicitly.
    [ ! -L "$file" ] || return 1
    if [ -e "$file" ]; then
        [ -f "$file" ] && [ -w "$file" ] || return 1
    fi
    if [ -f "$dir/installed-$kind" ]; then
        # A deleted or edited managed entry requires explicit repair, not another append.
        [ -f "$file" ] || return 1
        [ "$(grep -Fxc -- "$begin" "$file" || :)" = 1 ] || return 1
        [ "$(grep -Fxc -- "$end" "$file" || :)" = 1 ] || return 1
        actual=$(sed -n '/^# >>> alacrterm shell integration >>>$/,/^# <<< alacrterm shell integration <<</p' "$file")
        [ "$actual" = "$block" ] || return 1
    elif [ -f "$file" ] && grep -Fq -- "$begin" "$file"; then
        actual=$(sed -n '/^# >>> alacrterm shell integration >>>$/,/^# <<< alacrterm shell integration <<</p' "$file")
        [ "$actual" = "$block" ] || return 1
    fi
}
append_entry() {
    file=$1
    if [ -f "$file" ] && grep -Fqx -- "$begin" "$file"; then return; fi
    tmp=$(mktemp "${file}.alacrterm.XXXXXXXX")
    if [ -f "$file" ]; then cp -p -- "$file" "$tmp"; fi
    printf '\n%s\n' "$block" >> "$tmp"
    mv -f -- "$tmp" "$file"
    tmp=
}
write_script() {
    file=$1
    content=$2
    [ ! -L "$file" ] || return 1
    tmp=$(mktemp "${file}.XXXXXXXX")
    printf '%s' "$content" > "$tmp"
    if [ -f "$file" ] && cmp -s -- "$tmp" "$file"; then
        rm -f -- "$tmp"
    else
        chmod 600 "$tmp"
        mv -f -- "$tmp" "$file"
    fi
    tmp=
}

if [ "$kind" = bash ]; then
    rc=$HOME/.bashrc
    profile=$HOME/.profile
    for candidate in "$HOME/.bash_profile" "$HOME/.bash_login" "$HOME/.profile"; do
        if [ -e "$candidate" ]; then profile=$candidate; break; fi
    done
    check_entry "$rc"
    check_entry "$profile"
else
    rc=$zshdir/.zshrc
    check_entry "$rc"
fi
for file in bash.sh zsh.zsh loader.sh installed-bash installed-zsh; do [ ! -L "$dir/$file" ] || exit 1; done
chmod 700 "$dir"
write_script "$dir/bash.sh" "$bash_script"
write_script "$dir/zsh.zsh" "$zsh_script"
write_script "$dir/loader.sh" "$loader"
append_entry "$rc"
if [ "$kind" = bash ]; then append_entry "$profile"; fi
# Write the manifest only after every entrypoint is installed successfully.
write_script "$dir/installed-$kind" '1'
printf '\036ALACRTERM_INSTALLED:1\037'

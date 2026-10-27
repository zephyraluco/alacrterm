# One exec: discover shells first, then compare the version/type manifest.
set -eu
umask 077
version=$1
bash_script=$2
zsh_script=$3
loader=$4
login_shell=$5
zshdir=$6
shells_file=$7
dir=$HOME/.local/share/alacrterm/shell
have_bash=0
have_zsh=0
zsh_path=
add_shell() {
    [ -f "$1" ] && [ -x "$1" ] || return 0
    case ${1##*/} in
        bash) have_bash=1 ;;
        zsh) have_zsh=1; [ -n "$zsh_path" ] || zsh_path=$1 ;;
    esac
}
if [ -r "$shells_file" ]; then
    while IFS= read -r path || [ -n "$path" ]; do
        path=${path%%#*}
        path=${path#"${path%%[![:space:]]*}"}
        path=${path%"${path##*[![:space:]]}"}
        case $path in /*) add_shell "$path" ;; esac
    done < "$shells_file"
else
    add_shell "$login_shell"
fi
if [ "$have_bash$have_zsh" = 00 ]; then
    printf '\036ALACRTERM_INSTALLED:unsupported\037'
    exit 0
fi

stored_version=
stored_bash=0
stored_zsh=0
invalid=0
if [ -f "$dir/version" ] && [ -r "$dir/version" ]; then
    {
        IFS= read -r stored_version || :
        while IFS= read -r kind || [ -n "$kind" ]; do
            case $kind in
                bash) stored_bash=1 ;;
                zsh) stored_zsh=1 ;;
                *) invalid=1 ;;
            esac
        done
    } < "$dir/version"
fi
# Fast path intentionally trusts the manifest: no entry/script checks or writes,
# no directory creation, no install lock, and no Zsh startup probe.
if [ "$invalid" = 0 ] && [ "$stored_version" = "$version" ] &&
    [ "$stored_bash$stored_zsh" = "$have_bash$have_zsh" ]; then
    printf '\036ALACRTERM_INSTALLED:1\037'
    exit 0
fi

# A non-default Zsh can set an unexported ZDOTDIR in .zshenv. Discover it inside
# this exec only when installation is needed. Preserve the login shell's value
# when the SSH command itself already ran through Zsh.
if [ "$have_zsh" = 1 ]; then
    case ${login_shell##*/} in
        zsh) ;;
        *)
            probe=$("$zsh_path" -c 'printf "\036ALACRTERM_ZDOTDIR:%s\037" "${ZDOTDIR:-$HOME}"')
            prefix=$(printf '\036ALACRTERM_ZDOTDIR:')
            suffix=$(printf '\037')
            case $probe in
                *"$prefix"*"$suffix"*) zshdir=${probe#*"$prefix"}; zshdir=${zshdir%%"$suffix"*} ;;
                *) exit 1 ;;
            esac
            ;;
    esac
    case $zshdir in /*) ;; *) exit 1 ;; esac
fi
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
    [ ! -L "$file" ] || return 1
    if [ -e "$file" ]; then
        [ -f "$file" ] && [ -w "$file" ] || return 1
        starts=$(grep -Fxc -- "$begin" "$file" || :)
        ends=$(grep -Fxc -- "$end" "$file" || :)
        if [ "$starts$ends" != 00 ]; then
            [ "$starts$ends" = 11 ] || return 1
            actual=$(sed -n '/^# >>> alacrterm shell integration >>>$/,/^# <<< alacrterm shell integration <<</p' "$file")
            [ "$actual" = "$block" ] || return 1
        fi
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

if [ "$have_bash" = 1 ]; then
    profile=$HOME/.profile
    for candidate in "$HOME/.bash_profile" "$HOME/.bash_login" "$HOME/.profile"; do
        if [ -e "$candidate" ]; then profile=$candidate; break; fi
    done
    check_entry "$HOME/.bashrc"
    check_entry "$profile"
fi
if [ "$have_zsh" = 1 ]; then check_entry "$zshdir/.zshrc"; fi
for file in bash.sh zsh.zsh loader.sh version; do
    [ ! -L "$dir/$file" ] || exit 1
    if [ -e "$dir/$file" ]; then [ -f "$dir/$file" ] || exit 1; fi
done
chmod 700 "$dir"
write_script "$dir/bash.sh" "$bash_script"
write_script "$dir/zsh.zsh" "$zsh_script"
write_script "$dir/loader.sh" "$loader"
if [ "$have_bash" = 1 ]; then
    append_entry "$HOME/.bashrc"
    append_entry "$profile"
fi
if [ "$have_zsh" = 1 ]; then
    mkdir -p "$zshdir"
    append_entry "$zshdir/.zshrc"
fi
# Legacy markers are superseded by the shared manifest. Keep foreign directories.
for file in installed-bash installed-zsh; do
    if [ -f "$dir/$file" ] && [ ! -L "$dir/$file" ]; then rm -f -- "$dir/$file"; fi
done
manifest=$version
if [ "$have_bash" = 1 ]; then manifest="$manifest
bash"; fi
if [ "$have_zsh" = 1 ]; then manifest="$manifest
zsh"; fi
# Commit only after every discovered supported shell has been installed.
write_script "$dir/version" "$manifest
"
printf '\036ALACRTERM_INSTALLED:1\037'

#!/bin/sh
# Read-only monitor on its own SSH exec channel; no terminal or prompt changes.
LC_ALL=C
export LC_ALL
test -r /proc/stat && test -r /proc/meminfo || exit 78
while :; do
    printf 'host '; hostname
    printf 'user '; id -un
    if command -v who >/dev/null 2>&1; then
        if sessions=$(who 2>/dev/null); then
            printf 'sessions '; printf '%s\n' "$sessions" | awk 'NF { count++ } END { print count+0 }'
        fi
    fi
    awk '/^cpu / { print; } /^cpu[0-9]+ / { cores++ } END { print "cores " cores }' /proc/stat
    sed 's/^/mem /' /proc/meminfo
    sed 's/^/net /' /proc/net/dev
    printf 'uptime '; cat /proc/uptime
    df -Pk / 2>/dev/null | awk 'NR == 2 { print "disk " $0 }'
    printf '__ALACRTERM_STATUS_END__\n'
    sleep 2 || exit
done

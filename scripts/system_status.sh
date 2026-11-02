#!/bin/sh
# Read-only monitor on its own SSH exec channel; no terminal or prompt changes.
LC_ALL=C
export LC_ALL
test -r /proc/stat && test -r /proc/meminfo || exit 78
status_dir=${status_dir:-${HOME:-/}}
cpu_topology=$(lscpu 2>/dev/null) || cpu_topology=
cpu_present=$(cat /sys/devices/system/cpu/present 2>/dev/null) || cpu_present=
while :; do
    printf 'host '; hostname
    printf 'user '; id -un
    if command -v who >/dev/null 2>&1; then
        if sessions=$(who -u 2>/dev/null); then
            printf 'sessions '; printf '%s\n' "$sessions" | awk 'NF { count++ } END { print count+0 }'
            printf '%s\n' "$sessions" | sed '/^$/d;s/^/login /'
        fi
    fi
    awk '/^cpu / { print; } /^cpu[0-9]+ / { threads++ } END { print "threads " threads }' /proc/stat
    awk -F: '
        /^physical id/ { socket=$2; gsub(/[[:space:]]/, "", socket) }
        /^core id/ { core=$2; gsub(/[[:space:]]/, "", core); ids[socket ":" core]=1 }
        /^$/ { socket="" }
        END { for (id in ids) count++; print "physical_cores " count+0 }
    ' /proc/cpuinfo 2>/dev/null
    printf '%s\n' "$cpu_topology" | awk -F: '
        /^[[:space:]]*CPU\(s\):/ { threads=$2+0 }
        /^[[:space:]]*Core\(s\) per socket:/ { cores=$2+0 }
        /^[[:space:]]*Socket\(s\):/ { sockets=$2+0 }
        END {
            if (threads>0) print "configured_threads " threads
            if (cores>0 && sockets>0) print "configured_cores " cores*sockets
        }
    '
    printf '%s\n' "$cpu_present" | awk -F, '{
        for(i=1;i<=NF;i++) {
            if ($i ~ /^[0-9]+-[0-9]+$/) { split($i,r,"-"); count+=r[2]-r[1]+1 }
            else if ($i ~ /^[0-9]+$/) count++
        }
    } END { if(count>0) print "configured_threads " count }'
    sed 's/^/mem /' /proc/meminfo
    sed 's/^/net /' /proc/net/dev
    printf 'uptime '; cat /proc/uptime
    df -Pk -- "$status_dir" 2>/dev/null | awk 'NR == 2 {
        mount=$6; for (i=7; i<=NF; i++) mount=mount " " $i
        printf "disk %s\t%s\t%s\t%s\t%s\t%s\n", $1, $2, $3, $4, $5, mount
    }'
    df -PkT 2>/dev/null | awk 'NR>1 {
        mount=$7; for (i=8; i<=NF; i++) mount=mount " " $i
        printf "filesystem %s\t%s\t%s\t%s\t%s\t%s\t%s\n", $1, $2, $3, $4, $5, $6, mount
    }'
    printf '__ALACRTERM_STATUS_END__\n'
    sleep 2 || exit
done

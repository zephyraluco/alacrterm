#!/bin/sh
# Read-only monitor on its own SSH exec channel; no terminal or prompt changes.
LC_ALL=C
export LC_ALL
trap 'exit 0' HUP INT TERM
test -r /proc/stat && test -r /proc/meminfo || exit 78
status_dir=${status_dir:-${HOME:-/}}
status_dir=$(cd "$status_dir" 2>/dev/null && pwd -P) || status_dir=/
monitor_host=$(hostname)
monitor_user=$(id -un)
# Read release metadata as data, never source a remote file as shell code.
monitor_os_id=
monitor_os_like=
monitor_release=/etc/os-release
test -r "$monitor_release" || monitor_release=/usr/lib/os-release
if test -r "$monitor_release"; then
    while IFS='=' read -r key value || test -n "$key"; do
        value=${value#\"}; value=${value%\"}
        value=${value#\'}; value=${value%\'}
        case $key in
            ID) monitor_os_id=$value ;;
            ID_LIKE) monitor_os_like=$value ;;
        esac
    done < "$monitor_release"
fi
monitor_logins=
monitor_partitions=
monitor_sample=0
while :; do
    printf 'os_kernel Linux\nos_id %s\nos_like %s\n' "$monitor_os_id" "$monitor_os_like"
    printf 'host %s\nuser %s\n' "$monitor_host" "$monitor_user"
    printf '%s\n' "$monitor_logins" "$monitor_partitions"
    # One process reads all fast counters. CPU topology is not displayed, so
    # avoid lscpu and repeated full /proc/cpuinfo scans on the remote host.
    awk '
        FILENAME == "/proc/stat" {
            if ($1 == "cpu") print
            else if ($1 ~ /^cpu[0-9]+$/) threads++
            next
        }
        FILENAME == "/proc/meminfo" {
            if ($1 ~ /^(MemTotal|MemAvailable|MemFree|Buffers|Cached|SReclaimable|Shmem):$/) print "mem " $0
            next
        }
        FILENAME == "/proc/net/dev" { if ($0 ~ /:/) print "net " $0; next }
        FILENAME == "/proc/uptime" { print "uptime " $0 }
        END { print "threads " threads }
    ' /proc/stat /proc/meminfo /proc/net/dev /proc/uptime
    printf '__ALACRTERM_STATUS_END__\n' || exit
    sleep 2 || exit
    # Send the first lightweight frame before doing slower metadata work.
    # Cached records are included in every frame; missing records are not zeros.
    if test $((monitor_sample % 5)) -eq 0; then
        monitor_logins=
        if command -v who >/dev/null 2>&1 && monitor_sessions=$(who -u 2>/dev/null); then
            monitor_logins=$(printf '%s\n' "$monitor_sessions" | awk 'NF { print "login " $0; count++ } END { print "sessions " count+0 }')
        fi
    fi
    if test $((monitor_sample % 15)) -eq 0; then
        # A single filesystem query supplies both the table and current mount.
        monitor_partitions=$(df -PkT 2>/dev/null | ALACRTERM_MONITOR_DIRECTORY="$status_dir" awk '
            NR > 1 {
                mount=$7; for (i=8; i<=NF; i++) mount=mount " " $i
                printf "filesystem %s\t%s\t%s\t%s\t%s\t%s\t%s\n", $1, $2, $3, $4, $5, $6, mount
                path=ENVIRON["ALACRTERM_MONITOR_DIRECTORY"]
                if (length(mount)>longest && (mount=="/" || path==mount || index(path,mount "/")==1)) {
                    longest=length(mount)
                    disk=$1 "\t" $3 "\t" $4 "\t" $5 "\t" $6 "\t" mount
                }
            }
            END { if (longest>0) print "disk " disk }
        ')
    fi
    monitor_sample=$((monitor_sample + 1))
done

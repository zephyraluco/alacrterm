#!/bin/sh
set -eu
runtime=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
export WAYX_XKB_DIR="$runtime/share/X11/xkb"
export LD_LIBRARY_PATH="$runtime/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"

export WAYX_XKB_BIN_DIR="$runtime/bin"
exec "$runtime/bin/wayx" serve --core "$runtime/lib/libwayx-xserver.so" "$@"

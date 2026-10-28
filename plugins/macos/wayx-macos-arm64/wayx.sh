#!/bin/sh
set -eu
runtime=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
export WAYX_XKB_DIR="$runtime/share/X11/xkb"

export WAYX_XKB_BIN_DIR="$runtime/bin"
exec "$runtime/bin/wayx" serve --core "$runtime/lib/libwayx-xserver.dylib" "$@"

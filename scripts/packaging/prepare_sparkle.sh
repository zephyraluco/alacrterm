#!/usr/bin/env bash
set -euo pipefail

python3 scripts/packaging/fetch_sparkle.py
echo "SPARKLE_FRAMEWORK_DIR=$GITHUB_WORKSPACE/target/sparkle" >> "$GITHUB_ENV"
echo "DYLD_FRAMEWORK_PATH=$GITHUB_WORKSPACE/target/sparkle${DYLD_FRAMEWORK_PATH:+:$DYLD_FRAMEWORK_PATH}" >> "$GITHUB_ENV"

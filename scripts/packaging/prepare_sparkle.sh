#!/usr/bin/env bash
set -euo pipefail

python3 scripts/packaging/fetch_sparkle.py
echo "SPARKLE_FRAMEWORK_DIR=$GITHUB_WORKSPACE/target/sparkle" >> "$GITHUB_ENV"

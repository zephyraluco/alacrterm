#!/usr/bin/env bash
set -euo pipefail

gh api --method POST "repos/$GH_REPO/releases/generate-notes" -f "tag_name=$RELEASE_TAG" --jq .body > "$RUNNER_TEMP/release-notes.md"
python3 scripts/packaging/release.py --tag "$RELEASE_TAG" --repository "$GH_REPO" --notes "$RUNNER_TEMP/release-notes.md"

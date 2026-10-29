#!/usr/bin/env bash
set -euo pipefail

gh release create "$RELEASE_TAG" dist/* --verify-tag --draft --notes-file "$RUNNER_TEMP/release-notes.md"
gh release edit "$RELEASE_TAG" --draft=false --latest

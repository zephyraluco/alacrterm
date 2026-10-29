#!/usr/bin/env bash
set -euo pipefail

# Only branch push runs validate the exact tagged commit (PR runs may test a merge).
deadline=$((SECONDS + 3600))
while true; do
  pending=false
  for workflow in test.yml package.yml; do
    result=$(gh run list --repo "$GH_REPO" --workflow "$workflow" \
      --commit "$RELEASE_SHA" --event push --limit 100 \
      --json status,conclusion,url \
      --jq '.[0] | if . == null then "missing" else [.status, .conclusion, .url] | join("|") end')
    case "$result" in
      completed\|success\|*) ;;
      completed\|*)
        echo "::error::$workflow did not succeed: $result"
        exit 1
        ;;
      *)
        echo "Waiting for $workflow at $RELEASE_SHA: $result"
        pending=true
        ;;
    esac
  done
  if [ "$pending" = false ]; then
    echo "test and package passed for $RELEASE_SHA"
    exit 0
  fi
  if (( SECONDS >= deadline )); then
    echo "::error::Timed out waiting for branch CI. Push the tagged commit to a branch and ensure test and package succeed."
    exit 1
  fi
  sleep 30
done

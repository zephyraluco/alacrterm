#!/usr/bin/env bash
set -euo pipefail

security delete-keychain "$RUNNER_TEMP/alacrterm-signing.keychain-db" || true
rm -f "$RUNNER_TEMP/certificate.p12" "$RUNNER_TEMP/notary.p8"

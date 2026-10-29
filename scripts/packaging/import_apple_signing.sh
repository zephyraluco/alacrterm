#!/usr/bin/env bash
set -euo pipefail

python3 scripts/packaging/prepare_apple_signing.py
SIGNING_PASSWORD=$(openssl rand -hex 32)
security create-keychain -p "$SIGNING_PASSWORD" "$RUNNER_TEMP/alacrterm-signing.keychain-db"
security set-keychain-settings -lut 21600 "$RUNNER_TEMP/alacrterm-signing.keychain-db"
security unlock-keychain -p "$SIGNING_PASSWORD" "$RUNNER_TEMP/alacrterm-signing.keychain-db"
security import "$RUNNER_TEMP/certificate.p12" -k "$RUNNER_TEMP/alacrterm-signing.keychain-db" -P "$APPLE_CERTIFICATE_PASSWORD" -T /usr/bin/codesign
security set-key-partition-list -S apple-tool:,apple:,codesign: -s -k "$SIGNING_PASSWORD" "$RUNNER_TEMP/alacrterm-signing.keychain-db"
security list-keychains -d user -s "$RUNNER_TEMP/alacrterm-signing.keychain-db" "$HOME/Library/Keychains/login.keychain-db"

#!/usr/bin/env bash
set -euo pipefail

cc -Wall -Wextra -Werror crates/updater/native/sparkle_state_test.c -o /tmp/sparkle-state-test
/tmp/sparkle-state-test

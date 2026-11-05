#!/usr/bin/env bash
set -euo pipefail

cc -Wall -Wextra -Werror crates/updater/native/sparkle_state_test.c -o /tmp/sparkle-state-test
/tmp/sparkle-state-test
cc -Wall -Wextra -Werror crates/alacrterm/native/exit_state_test.c -o /tmp/alacrterm-exit-state-test
/tmp/alacrterm-exit-state-test

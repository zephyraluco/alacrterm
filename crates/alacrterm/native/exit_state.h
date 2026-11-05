#ifndef ALACRTERM_EXIT_STATE_H
#define ALACRTERM_EXIT_STATE_H
#include <stdbool.h>

typedef struct {
    bool requested;
    bool permitted;
    bool awaiting_reply;
} AlacrtermExitState;

static inline bool alacrterm_exit_should_allow(AlacrtermExitState *state) {
    if (state->permitted) {
        state->permitted = false;
        return true;
    }
    if (!state->awaiting_reply) {
        state->requested = true;
        state->awaiting_reply = true;
    }
    return false;
}

// Cancel any earlier deferred quit before resuming Sparkle. Its next quit event
// is already approved by the completed cleanup operation.
static inline bool alacrterm_exit_prepare(AlacrtermExitState *state) {
    bool awaiting = state->awaiting_reply;
    *state = (AlacrtermExitState){ .permitted = true };
    return awaiting;
}
#endif

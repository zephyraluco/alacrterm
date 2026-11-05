#include "exit_state.h"
#include <assert.h>

int main(void) {
    AlacrtermExitState state = {0};
    // A normal quit still waits for cleanup, without issuing duplicate requests.
    assert(!alacrterm_exit_should_allow(&state));
    assert(state.requested && state.awaiting_reply);
    state.requested = false;
    assert(!alacrterm_exit_should_allow(&state));
    assert(!state.requested);

    // Resume an update after cleanup, even if an ordinary quit arrived during it.
    assert(alacrterm_exit_prepare(&state));
    assert(!state.requested && !state.awaiting_reply);
    assert(alacrterm_exit_should_allow(&state));
    assert(!state.permitted);
    assert(!alacrterm_exit_should_allow(&state));

    // The usual update path has no outstanding native termination reply.
    state = (AlacrtermExitState){0};
    assert(!alacrterm_exit_prepare(&state));
    assert(alacrterm_exit_should_allow(&state));
    return 0;
}

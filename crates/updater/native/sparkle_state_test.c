#include "sparkle_state.h"
#include <assert.h>
int main(void) {
    assert(alacrterm_choice_state(true, false) == AlacrtermIdle);
    assert(alacrterm_choice_state(true, true) == AlacrtermIdle);
    assert(alacrterm_choice_state(false, false) == AlacrtermIdle);
    assert(alacrterm_choice_state(false, true) == AlacrtermDeferred);
    assert(alacrterm_cycle_end_state(AlacrtermChecking, false) == AlacrtermIdle);
    assert(alacrterm_cycle_end_state(AlacrtermAvailable, false) == AlacrtermIdle);
    assert(alacrterm_cycle_end_state(AlacrtermDownloading, false) == AlacrtermIdle);
    assert(alacrterm_cycle_end_state(AlacrtermWaitingRestart, true) == AlacrtermWaitingRestart);
    assert(alacrterm_cycle_end_state(AlacrtermDeferred, false) == AlacrtermDeferred);
    assert(alacrterm_cycle_end_state(AlacrtermFailed, false) == AlacrtermFailed);
    return 0;
}

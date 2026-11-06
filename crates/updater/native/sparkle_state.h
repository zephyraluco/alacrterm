#ifndef ALACRTERM_SPARKLE_STATE_H
#define ALACRTERM_SPARKLE_STATE_H
#include <stdbool.h>
enum AlacrtermUpdateState {
    AlacrtermIdle = 0,
    AlacrtermChecking = 1,
    AlacrtermAvailable = 2,
    AlacrtermDownloading = 3,
    AlacrtermWaitingRestart = 4,
    AlacrtermFailed = 5,
    AlacrtermDeferred = 6,
    AlacrtermCurrent = 7,
    AlacrtermInstalling = 8,
};
// Shared by the Objective-C bridge and portable regression tests.
static inline int alacrterm_choice_state(bool skip, bool downloaded) {
    return !skip && downloaded ? AlacrtermDeferred : AlacrtermIdle;
}
static inline int alacrterm_cycle_end_state(int state, bool pending) {
    if (pending || state == AlacrtermDeferred || state == AlacrtermFailed || state == AlacrtermCurrent) { return state; }
    return AlacrtermIdle;
}
static inline int alacrterm_restart_failure_state(int state, bool pending) {
    return pending ? AlacrtermDeferred : state;
}
static inline bool alacrterm_defer_install(int state, bool *install_requested) {
    bool offered = state == AlacrtermAvailable || state == AlacrtermWaitingRestart || state == AlacrtermDeferred;
    if (!offered) { return false; }
    *install_requested = false;
    return true;
}
#endif

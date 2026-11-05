#import <AppKit/AppKit.h>
#import <objc/runtime.h>
#include <stdbool.h>
#include "exit_state.h"

// GPUI exposes applicationWillTerminate, which is too late to reject termination.
// Add the cancellable AppKit delegate entry point without replacing GPUI's delegate.
static bool installed;
static AlacrtermExitState state;
static NSApplicationTerminateReply (*originalShouldTerminate)(id, SEL, NSApplication *);
static NSApplicationTerminateReply shouldTerminate(id self, SEL selector, NSApplication *app) {
    if (alacrterm_exit_should_allow(&state)) {
        return originalShouldTerminate ? originalShouldTerminate(self, selector, app) : NSTerminateNow;
    }
    return NSTerminateLater;
}
bool alacrterm_exit_install(void) {
    NSCAssert([NSThread isMainThread], @"Exit coordination must run on the main thread");
    if (installed) { return true; }
    id delegate = NSApp.delegate;
    if (delegate == nil) { return false; }
    Class cls = object_getClass(delegate);
    SEL selector = @selector(applicationShouldTerminate:);
    Method method = class_getInstanceMethod(cls, selector);
    if (method != NULL) {
        originalShouldTerminate = (void *)method_getImplementation(method);
        class_replaceMethod(cls, selector, (IMP)shouldTerminate, method_getTypeEncoding(method));
    } else {
        if (!class_addMethod(cls, selector, (IMP)shouldTerminate, "Q@:@")) { return false; }
    }
    installed = true;
    return true;
}
bool alacrterm_exit_requested(void) { bool value = state.requested; state.requested = false; return value; }
void alacrterm_exit_permit(void) { state.permitted = true; }
void alacrterm_exit_prepare_restart(void) {
    if (alacrterm_exit_prepare(&state)) {
        [NSApp replyToApplicationShouldTerminate:NO];
    }
}
void alacrterm_exit_revoke(void) { state.permitted = false; }
void alacrterm_exit_allow(void) {
    if (state.awaiting_reply) {
        state.awaiting_reply = false;
        state.requested = false;
        [NSApp replyToApplicationShouldTerminate:YES];
        return;
    }
    state.permitted = true;
    [NSApp terminate:nil];
    // If a future GPUI delegate cancels termination, the permission must not leak.
    state.permitted = false;
}

void alacrterm_exit_cancel(void) {
    if (state.awaiting_reply) {
        state.awaiting_reply = false;
        state.requested = false;
        [NSApp replyToApplicationShouldTerminate:NO];
    }
}

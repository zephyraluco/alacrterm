#import <AppKit/AppKit.h>
#import <objc/runtime.h>
#include <stdbool.h>

// GPUI exposes applicationWillTerminate, which is too late to reject termination.
// Add the cancellable AppKit delegate entry point without replacing GPUI's delegate.
static bool installed;
static bool requested;
static bool permitted;
static bool awaitingReply;
static NSApplicationTerminateReply (*originalShouldTerminate)(id, SEL, NSApplication *);
static NSApplicationTerminateReply shouldTerminate(id self, SEL selector, NSApplication *app) {
    if (permitted) {
        permitted = false;
        return originalShouldTerminate ? originalShouldTerminate(self, selector, app) : NSTerminateNow;
    }
    if (!awaitingReply) { requested = true; awaitingReply = true; }
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
bool alacrterm_exit_requested(void) { bool value = requested; requested = false; return value; }
void alacrterm_exit_permit(void) { permitted = true; }
void alacrterm_exit_allow(void) {
    if (awaitingReply) {
        awaitingReply = false;
        [NSApp replyToApplicationShouldTerminate:YES];
        return;
    }
    permitted = true;
    [NSApp terminate:nil];
    // If a future GPUI delegate cancels termination, the permission must not leak.
    permitted = false;
}

void alacrterm_exit_cancel(void) {
    if (awaitingReply) {
        awaitingReply = false;
        [NSApp replyToApplicationShouldTerminate:NO];
    }
}

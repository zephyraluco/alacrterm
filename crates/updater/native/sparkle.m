#import <AppKit/AppKit.h>
#import <Sparkle/Sparkle.h>
#include <stdbool.h>
#include "sparkle_state.h"
#import <dispatch/dispatch.h>

static SPUUpdater *controller;
static NSString *releaseNotes = @"";
static void (^pendingChoice)(SPUUserUpdateChoice);
static void (^downloadCancellation)(void);
static bool installRequested;
static uint64_t downloadedBytes;
static uint64_t expectedBytes;
static double progress = -1;
static NSString *availableVersion = @"";
static NSString *statusText = @"尚未检查更新";
static void (^pendingInstallation)(void);
static bool restartRequested;
static bool changed;
static int updateState;

static void updateStatus(NSString *text, int state) {
    statusText = text;
    updateState = state;
    changed = true;
}

// Sparkle owns transport, validation and installation; GPUI owns all update UI.
@interface AlacrtermUserDriver : NSObject <SPUUserDriver>
@end
@implementation AlacrtermUserDriver
- (void)showUpdatePermissionRequest:(SPUUpdatePermissionRequest *)request reply:(void (^)(SUUpdatePermissionResponse *))reply {
    reply([[SUUpdatePermissionResponse alloc] initWithAutomaticUpdateChecks:controller.automaticallyChecksForUpdates sendSystemProfile:NO]);
}
- (void)showUserInitiatedUpdateCheckWithCancellation:(void (^)(void))cancellation {
    updateStatus(@"正在检查更新…", AlacrtermChecking);
}
- (void)showUpdateFoundWithAppcastItem:(SUAppcastItem *)item state:(SPUUserUpdateState *)state reply:(void (^)(SPUUserUpdateChoice))reply {
    availableVersion = item.displayVersionString;
    releaseNotes = item.itemDescription ?: @"";
    pendingChoice = [reply copy];
    progress = -1;
    updateStatus(@"有新版本发布", AlacrtermAvailable);
    if (installRequested) {
        void (^choice)(SPUUserUpdateChoice) = pendingChoice;
        pendingChoice = nil;
        updateStatus(@"正在准备下载更新…", AlacrtermDownloading);
        choice(SPUUserUpdateChoiceInstall);
    }
}
- (void)showUpdateReleaseNotesWithDownloadData:(SPUDownloadData *)data {
    if (data.data.length <= 256 * 1024) {
        releaseNotes = [[NSString alloc] initWithData:data.data encoding:NSUTF8StringEncoding] ?: @"";
        changed = true;
    }
}
- (void)showUpdateReleaseNotesFailedToDownloadWithError:(NSError *)error {
    // Notes are informational; their failure must not prevent a signed update.
    releaseNotes = @"更新说明暂时无法加载。";
    changed = true;
}
- (void)showUpdateNotFoundWithError:(NSError *)error acknowledgement:(void (^)(void))acknowledgement {
    installRequested = false;
    NSNumber *reason = error.userInfo[SPUNoUpdateFoundReasonKey];
    BOOL latest = reason != nil && (reason.integerValue == SPUNoUpdateFoundReasonOnLatestVersion || reason.integerValue == SPUNoUpdateFoundReasonOnNewerThanLatestVersion);
    updateStatus(latest ? @"已是最新版本，暂无更新" : error.localizedDescription, latest ? AlacrtermCurrent : AlacrtermFailed);
    acknowledgement();
}
- (void)showUpdaterError:(NSError *)error acknowledgement:(void (^)(void))acknowledgement {
    installRequested = false;
    updateStatus(error.localizedDescription, AlacrtermFailed);
    acknowledgement();
}
- (void)showDownloadInitiatedWithCancellation:(void (^)(void))cancellation {
    downloadCancellation = [cancellation copy];
    downloadedBytes = 0;
    expectedBytes = 0;
    progress = -1;
    updateStatus(@"正在下载更新…", AlacrtermDownloading);
}
- (void)showDownloadDidReceiveExpectedContentLength:(uint64_t)length {
    expectedBytes = length;
}
- (void)showDownloadDidReceiveDataOfLength:(uint64_t)length {
    downloadedBytes += length;
    progress = expectedBytes > 0 ? MIN(100.0, 100.0 * downloadedBytes / expectedBytes) : -1;
    changed = true;
}
- (void)showDownloadDidStartExtractingUpdate {
    downloadCancellation = nil;
    progress = -1;
    updateStatus(@"正在验证并解压更新…", AlacrtermDownloading);
}
- (void)showExtractionReceivedProgress:(double)value {
    progress = MAX(0.0, MIN(100.0, value * 100.0));
    changed = true;
}
- (void)showReadyToInstallAndRelaunch:(void (^)(SPUUserUpdateChoice))reply {
    progress = -1;
    if (installRequested) {
        updateStatus(@"正在准备安装并重启…", AlacrtermDownloading);
        reply(SPUUserUpdateChoiceInstall);
    } else {
        pendingChoice = [reply copy];
        updateStatus(@"更新已就绪", AlacrtermWaitingRestart);
    }
}
- (void)showInstallingUpdateWithApplicationTerminated:(BOOL)terminated retryTerminatingApplication:(void (^)(void))retry {
    progress = -1;
    updateStatus(@"正在安装并重启…", AlacrtermWaitingRestart);
}
- (void)showUpdateInstalledAndRelaunched:(BOOL)relaunched acknowledgement:(void (^)(void))acknowledgement {
    installRequested = false;
    updateStatus(@"更新已安装", AlacrtermCurrent);
    acknowledgement();
}
- (void)dismissUpdateInstallation {
    pendingChoice = nil;
    downloadCancellation = nil;
    installRequested = false;
}
@end
static AlacrtermUserDriver *userDriver;

@interface AlacrtermUpdateDelegate : NSObject <SPUUpdaterDelegate>
@end
@implementation AlacrtermUpdateDelegate
- (BOOL)updaterShouldPromptForPermissionToCheckForUpdates:(SPUUpdater *)updater {
    return NO;
}
- (BOOL)updater:(SPUUpdater *)updater mayPerformUpdateCheck:(SPUUpdateCheck)check error:(NSError *__autoreleasing *)error {
    if (pendingInstallation == nil) { updateStatus(@"正在检查更新…", 1); }
    return YES;
}
- (void)updater:(SPUUpdater *)updater didFindValidUpdate:(SUAppcastItem *)item {
    availableVersion = item.displayVersionString;
    updateStatus([NSString stringWithFormat:@"发现新版本 %@", item.displayVersionString], 2);
}
- (void)updaterDidNotFindUpdate:(SPUUpdater *)updater error:(NSError *)error {
    NSNumber *reason = error.userInfo[SPUNoUpdateFoundReasonKey];
    BOOL latest = reason != nil && (reason.integerValue == SPUNoUpdateFoundReasonOnLatestVersion || reason.integerValue == SPUNoUpdateFoundReasonOnNewerThanLatestVersion);
    updateStatus(latest ? @"已是最新版本，暂无更新" : error.localizedDescription, latest ? AlacrtermCurrent : AlacrtermFailed);
}
- (void)updater:(SPUUpdater *)updater willDownloadUpdate:(SUAppcastItem *)item withRequest:(NSMutableURLRequest *)request {
    updateStatus(@"Sparkle 正在下载更新", 3);
}
- (void)updater:(SPUUpdater *)updater didDownloadUpdate:(SUAppcastItem *)item {
    updateStatus(@"更新已下载，正在验证与准备安装", 3);
}
- (BOOL)updater:(SPUUpdater *)updater shouldPostponeRelaunchForUpdate:(SUAppcastItem *)item untilInvokingBlock:(void (^)(void))handler {
    pendingInstallation = [handler copy];
    restartRequested = true;
    updateStatus(@"更新已就绪，等待确认关闭终端与传输任务", 4);
    return YES;
}
- (void)updater:(SPUUpdater *)updater userDidMakeChoice:(SPUUserUpdateChoice)choice forUpdate:(SUAppcastItem *)item state:(SPUUserUpdateState *)state {
    if (choice == SPUUserUpdateChoiceInstall) { return; }
    int next = alacrterm_choice_state(choice == SPUUserUpdateChoiceSkip, state.stage != SPUUserUpdateStageNotDownloaded);
    if (next == AlacrtermDeferred) {
        updateStatus(@"更新已保留，将稍后安装", next);
        return;
    }
    pendingInstallation = nil;
    restartRequested = false;
    updateStatus(choice == SPUUserUpdateChoiceSkip ? @"已跳过此版本" : @"已暂缓此次更新", 0);
}
- (void)updater:(SPUUpdater *)updater didAbortWithError:(NSError *)error {
    pendingChoice = nil;
    downloadCancellation = nil;
    installRequested = false;
    pendingInstallation = nil;
    restartRequested = false;
    if (error.code == SUNoUpdateError) {
        if (updateState != AlacrtermCurrent && updateState != AlacrtermFailed) {
            updateStatus(error.localizedDescription, AlacrtermFailed);
        }
    } else if (error.code == SUInstallationCanceledError) {
        updateStatus(@"安装已取消", 0);
    } else {
        updateStatus(error.localizedDescription, 5);
    }
}
- (void)updater:(SPUUpdater *)updater didFinishUpdateCycleForUpdateCheck:(SPUUpdateCheck)check error:(NSError *)error {
    if (error != nil) {
        [self updater:updater didAbortWithError:error];
    } else {
        int next = alacrterm_cycle_end_state(updateState, pendingInstallation != nil);
        if (next != updateState) { updateStatus(@"更新检查已结束", next); }
    }
}
- (void)userDidCancelDownload:(SPUUpdater *)updater {
    pendingInstallation = nil;
    restartRequested = false;
    updateStatus(@"下载已取消", 0);
}

@end
static AlacrtermUpdateDelegate *delegate;

bool alacrterm_sparkle_start(bool automatic) {
    NSCAssert([NSThread isMainThread], @"Sparkle must run on the main thread");
    if (controller != nil) { return true; }
    if ([[NSBundle mainBundle] objectForInfoDictionaryKey:@"SUPublicEDKey"] == nil) {
        updateStatus(@"此构建未配置 Sparkle 更新公钥", 5);
        return false;
    }
    delegate = [AlacrtermUpdateDelegate new];
    userDriver = [AlacrtermUserDriver new];
    controller = [[SPUUpdater alloc] initWithHostBundle:[NSBundle mainBundle] applicationBundle:[NSBundle mainBundle] userDriver:userDriver delegate:delegate];
    controller.automaticallyChecksForUpdates = automatic;
    controller.automaticallyDownloadsUpdates = NO;
    NSError *error = nil;
    if (![controller startUpdater:&error]) {
        updateStatus(error.localizedDescription ?: @"Sparkle 初始化失败", 5);
        controller = nil;
        return false;
    }
    return true;
}
void alacrterm_sparkle_check(void) {
    if (pendingChoice != nil || pendingInstallation != nil) { changed = true; return; }
    [controller checkForUpdates];
}
void alacrterm_sparkle_automatic(bool enabled) {
    controller.automaticallyChecksForUpdates = enabled;
}
const char *alacrterm_sparkle_version(void) { return availableVersion.UTF8String; }
const char *alacrterm_sparkle_status(void) { return statusText.UTF8String; }
int alacrterm_sparkle_state(void) { return updateState; }
bool alacrterm_sparkle_changed(void) { bool value = changed; changed = false; return value; }
bool alacrterm_sparkle_restart_requested(void) { bool value = restartRequested; restartRequested = false; return value; }
bool alacrterm_sparkle_allow_restart(void) {
    if (pendingInstallation != nil) {
        void (^handler)(void) = pendingInstallation;
        pendingInstallation = nil;
        dispatch_async(dispatch_get_main_queue(), handler);
        return true;
    }
    return false;
}

const char *alacrterm_sparkle_notes(void) { return releaseNotes.UTF8String; }
double alacrterm_sparkle_progress(void) { return progress; }
void alacrterm_sparkle_install(void) {
    installRequested = true;
    if (pendingInstallation != nil) { restartRequested = true; return; }
    if (pendingChoice != nil) {
        void (^choice)(SPUUserUpdateChoice) = pendingChoice;
        pendingChoice = nil;
        progress = -1;
        updateStatus(@"正在准备更新…", AlacrtermDownloading);
        choice(SPUUserUpdateChoiceInstall);
    } else {
        [controller checkForUpdates];
    }
}
void alacrterm_sparkle_defer(void) {
    bool hadOffer = updateState == AlacrtermAvailable || updateState == AlacrtermWaitingRestart || updateState == AlacrtermDeferred;
    installRequested = false;
    if (pendingChoice != nil) {
        void (^choice)(SPUUserUpdateChoice) = pendingChoice;
        pendingChoice = nil;
        choice(SPUUserUpdateChoiceDismiss);
    }
    if (hadOffer && updateState != AlacrtermFailed) {
        updateStatus(@"更新已保留，可稍后安装", AlacrtermDeferred);
    }
}

#import <AppKit/AppKit.h>
#import <Sparkle/Sparkle.h>
#include <stdbool.h>

static SPUStandardUpdaterController *controller;
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

@interface AlacrtermUpdateDelegate : NSObject <SPUUpdaterDelegate>
@end
@implementation AlacrtermUpdateDelegate
- (BOOL)updaterShouldPromptForPermissionToCheckForUpdates:(SPUUpdater *)updater {
    return NO;
}
- (BOOL)updater:(SPUUpdater *)updater mayPerformUpdateCheck:(SPUUpdateCheck)check error:(NSError *__autoreleasing *)error {
    updateStatus(@"正在检查更新…", 1);
    return YES;
}
- (void)updater:(SPUUpdater *)updater didFindValidUpdate:(SUAppcastItem *)item {
    updateStatus([NSString stringWithFormat:@"发现新版本 %@", item.displayVersionString], 2);
}
- (void)updaterDidNotFindUpdate:(SPUUpdater *)updater error:(NSError *)error {
    updateStatus(@"已是最新版本", 0);
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
- (void)updater:(SPUUpdater *)updater didAbortWithError:(NSError *)error {
    pendingInstallation = nil;
    restartRequested = false;
    updateStatus(error.localizedDescription, 5);
}
- (void)userDidCancelDownload:(SPUUpdater *)updater {
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
    controller = [[SPUStandardUpdaterController alloc] initWithStartingUpdater:NO updaterDelegate:delegate userDriverDelegate:nil];
    controller.updater.automaticallyChecksForUpdates = automatic;
    controller.updater.automaticallyDownloadsUpdates = NO;
    NSError *error = nil;
    if (![controller.updater startUpdater:&error]) {
        updateStatus(error.localizedDescription ?: @"Sparkle 初始化失败", 5);
        controller = nil;
        return false;
    }
    return true;
}
void alacrterm_sparkle_check(void) {
    if (pendingInstallation != nil) { restartRequested = true; return; }
    [controller checkForUpdates:nil];
}
void alacrterm_sparkle_automatic(bool enabled) {
    controller.updater.automaticallyChecksForUpdates = enabled;
}
const char *alacrterm_sparkle_status(void) { return statusText.UTF8String; }
int alacrterm_sparkle_state(void) { return updateState; }
bool alacrterm_sparkle_changed(void) { bool value = changed; changed = false; return value; }
bool alacrterm_sparkle_restart_requested(void) { bool value = restartRequested; restartRequested = false; return value; }
void alacrterm_sparkle_allow_restart(void) {
    if (pendingInstallation != nil) {
        void (^handler)(void) = pendingInstallation;
        pendingInstallation = nil;
        handler();
    }
}

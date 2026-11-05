//! Non-interactive shutdown: cancel work, drain cleanup, then close/restart.
use crate::AppRoot;
use gpui::{App, Global, WeakEntity, Window};
use gpui_kit::component::{WindowExt as _, notification::Notification};

#[derive(Default)]
struct Coordinator {
    running: bool,
    sources: Vec<terminal::SshFs>,
}
impl Global for Coordinator {}

impl Coordinator {
    fn start_cleanup(&mut self, sources: Vec<terminal::SshFs>) -> Vec<terminal::SshFs> {
        for source in sources {
            if !self.sources.contains(&source) {
                self.sources.push(source);
            }
        }
        self.running = true;
        self.sources.clone()
    }

    fn finish_cleanup(&mut self, drained: bool) {
        self.running = false;
        // Timeout only ends this attempt. Direct editor uploads are not tracked
        // by the transfer manager, and their closed views cannot supply handles
        // on retry. Release them only after every source has actually drained.
        if drained {
            self.sources.clear();
        }
    }
}

#[cfg(any(target_os = "macos", test))]
impl Coordinator {
    fn take_request(&self, take: impl FnOnce() -> bool) -> bool {
        !self.running && take()
    }
}

#[cfg(any(target_os = "macos", test))]
pub(crate) fn take_native_request(cx: &App, take: impl FnOnce() -> bool) -> bool {
    // Reading either native flag consumes it. Leave it pending while another
    // exit operation owns cleanup; both watchers run on the main thread.
    match cx.try_global::<Coordinator>() {
        Some(state) => state.take_request(take),
        None => take(),
    }
}

type Continuation = Box<dyn FnOnce(&mut Window, &mut App) -> anyhow::Result<()>>;
type Completion = Box<dyn FnOnce(bool, &mut App)>;

pub(crate) fn request(
    root: WeakEntity<AppRoot>,
    window: &mut Window,
    cx: &mut App,
    title: &'static str,
    action: Continuation,
) {
    request_with_completion(root, window, cx, title, action, Box::new(|_, _| {}));
}

pub(crate) fn request_with_completion(
    root: WeakEntity<AppRoot>,
    window: &mut Window,
    cx: &mut App,
    _title: &'static str,
    action: Continuation,
    completed: Completion,
) {
    if !cx.has_global::<Coordinator>() {
        cx.set_global(Coordinator::default());
    }
    if cx.global::<Coordinator>().running {
        completed(false, cx);
        return;
    }
    let Ok((mut sources, preview)) = root.update(cx, |root, cx| {
        root.closing = true;
        let mut sources = root
            ._transfers
            .read(cx)
            .transfers()
            .iter()
            .map(|task| task.source.clone())
            .collect::<Vec<_>>();
        root._transfers
            .update(cx, |manager, cx| manager.cancel_all(cx));
        for tab in &root.tabs {
            sources.extend(tab.update(cx, |tab, cx| tab.shutdown(cx)));
        }
        (sources, root.editor_window.take())
    }) else {
        completed(false, cx);
        return;
    };
    if let Some(preview) = preview {
        let _ = preview.update(cx, |root, window, cx| {
            if let Ok(view) = root
                .view()
                .clone()
                .downcast::<crate::editor_window::EditorWindow>()
            {
                sources.extend(view.update(cx, |preview, _| preview.shutdown_sources()));
            }
            window.remove_window();
        });
    }
    for source in &sources {
        source.cancel_transfers();
    }
    let sources = cx.global_mut::<Coordinator>().start_cleanup(sources);
    log::info!("Exit cleanup started: {_title}");
    window
        .spawn(cx, async move |cx| {
            let started = std::time::Instant::now();
            loop {
                let ready = cx.update(|_, cx| {
                    for source in &sources {
                        source.cancel_transfers();
                    }
                    !sources.iter().any(|source| source.has_pending_transfers())
                        && root
                            .read_with(cx, |root, cx| root._transfers.read(cx).active_count() == 0)
                            .unwrap_or(true)
                });
                match ready {
                    Ok(true) => break,
                    Ok(false) => {
                        if started.elapsed() >= std::time::Duration::from_secs(30) {
                            let _ = cx.update(|window, cx| {
                                cx.global_mut::<Coordinator>().finish_cleanup(false);
                                let _ = root.update(cx, |root, _| root.closing = false);
                                completed(false, cx);
                                log::error!("Exit cleanup timed out: {_title}");
                                window.push_notification(
                                    Notification::error(
                                        "等待传输任务结束超时，已取消退出，请稍后重试。",
                                    ),
                                    cx,
                                );
                            });
                            return;
                        }
                        cx.background_executor()
                            .timer(std::time::Duration::from_millis(25))
                            .await
                    }
                    Err(_) => return,
                }
            }
            let _ = cx.update(|window, cx| {
                log::info!("Exit cleanup completed: {_title}");
                completed(true, cx);
                cx.global_mut::<Coordinator>().finish_cleanup(true);
                if let Err(error) = action(window, cx) {
                    let _ = root.update(cx, |root, _| root.closing = false);
                    window
                        .push_notification(Notification::error(format!("操作失败：{error:#}")), cx);
                }
            });
        })
        .detach();
}

impl AppRoot {
    pub fn should_close(root: &WeakEntity<Self>, window: &mut Window, cx: &mut App) -> bool {
        request(
            root.clone(),
            window,
            cx,
            "关闭窗口",
            Box::new(|window, _| {
                #[cfg(target_os = "macos")]
                native::permit();
                window.remove_window();
                Ok(())
            }),
        );
        false
    }
}

#[cfg(any(target_os = "macos", test))]
mod native {
    #[cfg(target_os = "macos")]
    unsafe extern "C" {
        fn alacrterm_exit_install() -> bool;
        fn alacrterm_exit_requested() -> bool;
        fn alacrterm_exit_allow();
        fn alacrterm_exit_permit();
        fn alacrterm_exit_cancel();
        fn alacrterm_exit_prepare_restart();
        fn alacrterm_exit_revoke();
    }
    pub(super) fn install() -> bool {
        #[cfg(target_os = "macos")]
        {
            unsafe { alacrterm_exit_install() }
        }
        #[cfg(not(target_os = "macos"))]
        {
            true
        }
    }
    pub(super) fn requested() -> bool {
        #[cfg(target_os = "macos")]
        {
            unsafe { alacrterm_exit_requested() }
        }
        #[cfg(not(target_os = "macos"))]
        {
            false
        }
    }
    pub(super) fn allow() {
        #[cfg(target_os = "macos")]
        unsafe {
            alacrterm_exit_allow();
        }
    }
    pub(super) fn cancel() {
        #[cfg(target_os = "macos")]
        unsafe {
            alacrterm_exit_cancel();
        }
    }
    pub(super) fn prepare_restart() {
        #[cfg(target_os = "macos")]
        unsafe {
            alacrterm_exit_prepare_restart();
        }
    }
    pub(super) fn revoke() {
        #[cfg(target_os = "macos")]
        unsafe {
            alacrterm_exit_revoke();
        }
    }
    #[cfg(target_os = "macos")]
    pub(super) fn permit() {
        unsafe {
            alacrterm_exit_permit();
        }
    }
}

#[cfg(any(target_os = "macos", test))]
pub(crate) fn allow_sparkle_restart() -> anyhow::Result<()> {
    // Cleanup is finished. Sparkle must own termination and relaunch, and its
    // upcoming Apple quit event must not start another GPUI cleanup cycle.
    native::prepare_restart();
    log::info!("Handing restart to Sparkle after exit cleanup");
    if !updater::macos::allow_restart() {
        native::revoke();
        anyhow::bail!("更新安装请求已失效，请重新检查更新");
    }
    Ok(())
}

#[cfg(any(target_os = "macos", test))]
pub(crate) fn revoke_sparkle_restart() {
    native::revoke();
}

#[cfg(any(target_os = "macos", test))]
pub fn attach_native(root: WeakEntity<AppRoot>, window: &mut Window, cx: &mut App) {
    assert!(native::install(), "无法安装 macOS 退出协调器");
    window
        .spawn(cx, async move |cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(50))
                    .await;
                if cx
                    .update(|window, cx| {
                        if take_native_request(cx, native::requested) {
                            request_with_completion(
                                root.clone(),
                                window,
                                cx,
                                "退出 Alacrterm",
                                Box::new(|_, _| {
                                    platform::shutdown_wayx();
                                    native::allow();
                                    Ok(())
                                }),
                                Box::new(|accepted, _| {
                                    if !accepted {
                                        native::cancel();
                                    }
                                }),
                            );
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_restart_stays_pending_until_cleanup_is_idle() {
        let pending = std::cell::Cell::new(true);
        let take = || pending.replace(false);
        let mut coordinator = Coordinator {
            running: true,
            ..Default::default()
        };
        assert!(!coordinator.take_request(take));
        assert!(
            pending.get(),
            "busy cleanup must preserve the restart request"
        );
        coordinator.running = false;
        assert!(coordinator.take_request(take));
        assert!(!coordinator.take_request(take), "deliver the request once");
    }
    #[test]
    fn native_exit_watch_uses_the_window_context_on_all_test_platforms() {
        let _: fn(WeakEntity<AppRoot>, &mut Window, &mut App) = attach_native;
    }
    #[test]
    fn retry_after_timeout_keeps_editor_sources_without_rediscovering_them() {
        let source = terminal::SshFs::new(ssh::SshParams::new("localhost", 22, "tester"));
        let mut coordinator = Coordinator::default();
        let first = coordinator.start_cleanup(vec![source.clone()]);
        coordinator.finish_cleanup(false);
        drop(first);
        // The editor and terminal have been removed by the first attempt. A
        // retry cannot discover their handles again, even for direct writebacks.
        let retry = coordinator.start_cleanup(Vec::new());
        assert_eq!(retry.len(), 1, "the previous source must still be awaited");
        assert_eq!(retry[0].session_id(), source.session_id());
        coordinator.finish_cleanup(false);
        let again = coordinator.start_cleanup(vec![source]);
        assert_eq!(again.len(), 1, "rediscovered handles should not accumulate");
        coordinator.finish_cleanup(true);
        assert!(coordinator.start_cleanup(Vec::new()).is_empty());
    }
}

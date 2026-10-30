//! Non-interactive shutdown: cancel work, drain cleanup, then close/restart.
use crate::AppRoot;
use gpui::{App, Global, WeakEntity, Window};
use gpui_kit::component::{WindowExt as _, notification::Notification};

#[derive(Default)]
struct Coordinator {
    running: bool,
}
impl Global for Coordinator {}

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
        (sources, root.preview_window.take())
    }) else {
        completed(false, cx);
        return;
    };
    if let Some(preview) = preview {
        let _ = preview.update(cx, |root, window, cx| {
            if let Ok(view) = root
                .view()
                .clone()
                .downcast::<crate::preview_window::PreviewWindow>()
            {
                sources.extend(view.update(cx, |preview, _| preview.shutdown_sources()));
            }
            window.remove_window();
        });
    }
    for source in &sources {
        source.cancel_transfers();
    }
    cx.global_mut::<Coordinator>().running = true;
    window
        .spawn(cx, async move |cx| {
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
                        cx.background_executor()
                            .timer(std::time::Duration::from_millis(25))
                            .await
                    }
                    Err(_) => return,
                }
            }
            let _ = cx.update(|window, cx| {
                completed(true, cx);
                cx.global_mut::<Coordinator>().running = false;
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
    pub(crate) fn should_close(root: &WeakEntity<Self>, window: &mut Window, cx: &mut App) -> bool {
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
    #[cfg(target_os = "macos")]
    pub(super) fn permit() {
        unsafe {
            alacrterm_exit_permit();
        }
    }
}

#[cfg(any(target_os = "macos", test))]
pub(crate) fn allow_sparkle_restart(_root: &WeakEntity<AppRoot>, _cx: &mut App) {
    updater::macos::allow_restart();
}

#[cfg(any(target_os = "macos", test))]
pub(crate) fn attach_native(root: WeakEntity<AppRoot>, window: &mut Window, cx: &mut App) {
    assert!(native::install(), "无法安装 macOS 退出协调器");
    window
        .spawn(cx, async move |cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(50))
                    .await;
                if cx
                    .update(|window, cx| {
                        if native::requested() {
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
    fn native_exit_watch_uses_the_window_context_on_all_test_platforms() {
        let _: fn(WeakEntity<AppRoot>, &mut Window, &mut App) = attach_native;
    }
}

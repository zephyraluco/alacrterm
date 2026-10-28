//! A single confirmation boundary for window closure and update installation.
//! Callers must not terminate the app or release a Sparkle restart outside this boundary.
use crate::AppRoot;
use crate::close_after_transfers;
use gpui::{App, Context, Global, ParentElement as _, Styled as _, WeakEntity, Window, div, px};
use gpui_kit::component::{WindowExt as _, notification::Notification};
use gpui_kit::component::{
    button::{Button, ButtonVariants as _},
    dialog::DialogFooter,
};

#[derive(Default)]
struct Coordinator {
    prompting: bool,
    #[cfg(any(target_os = "macos", test))]
    native_approval: Option<(Snapshot, std::time::Instant)>,
}
impl Global for Coordinator {}

type Continuation = Box<dyn FnOnce(&mut App) -> anyhow::Result<()>>;

#[derive(Clone, Debug, PartialEq, Eq)]
struct Snapshot {
    transfers: usize,
    dirty: bool,
    connections: Vec<gpui::EntityId>,
}
impl Snapshot {
    fn blocked(&self) -> bool {
        self.transfers > 0 || self.dirty
    }
    fn can_continue(&self, next: &Self) -> bool {
        !next.blocked() && self.connections == next.connections
    }
}
fn snapshot(root: &WeakEntity<AppRoot>, cx: &mut App) -> Option<Snapshot> {
    let (transfers, preview, connections) = root
        .read_with(cx, |root, cx| {
            (
                root._transfers.read(cx).active_count(),
                root.preview_window,
                root.tabs
                    .iter()
                    .flat_map(|tab| tab.read(cx).connected_views(cx))
                    .collect(),
            )
        })
        .ok()?;
    Some(Snapshot {
        transfers,
        dirty: AppRoot::preview_has_pending_changes(preview, cx),
        connections,
    })
}

pub(crate) fn request(
    root: WeakEntity<AppRoot>,
    window: &mut Window,
    cx: &mut App,
    title: &'static str,
    action: Continuation,
) {
    request_with_completion(root, window, cx, title, action, Box::new(|_| {}));
}

pub(crate) fn request_with_completion(
    root: WeakEntity<AppRoot>,
    window: &mut Window,
    cx: &mut App,
    title: &'static str,
    action: Continuation,
    completed: Box<dyn FnOnce(&mut App)>,
) {
    let Some(before) = snapshot(&root, cx) else {
        completed(cx);
        return;
    };
    if before.blocked() {
        completed(cx);
        window.push_notification(
            Notification::warning("请先完成文件传输，并保存预览中的改动后重试"),
            cx,
        );
        return;
    }
    if !cx.has_global::<Coordinator>() {
        cx.set_global(Coordinator::default());
    }
    if cx.global::<Coordinator>().prompting {
        completed(cx);
        return;
    }
    cx.global_mut::<Coordinator>().prompting = true;
    let prompt = window.prompt(
        gpui::PromptLevel::Warning,
        title,
        Some(&format!(
            "将关闭 {} 个终端连接。\nX11 会话也会中断。\n本地进程无法恢复。是否继续？",
            before.connections.len()
        )),
        &["继续", "取消"],
        cx,
    );
    window
        .spawn(cx, async move |cx| {
            let accepted = prompt.await.ok() == Some(0);
            gpui::AsyncApp::update(cx, |cx| {
                cx.global_mut::<Coordinator>().prompting = false;
                completed(cx);
            });
            let result = cx.update(|window, cx| {
                if !accepted {
                    return;
                }
                if !snapshot(&root, cx).is_some_and(|after| before.can_continue(&after)) {
                    window.push_notification(
                        Notification::warning("连接或任务状态已改变，请确认后重试"),
                        cx,
                    );
                    return;
                }
                if let Err(error) = action(cx) {
                    window
                        .push_notification(Notification::error(format!("操作失败：{error:#}")), cx);
                }
            });
            if let Err(error) = result {
                log::debug!("退出确认窗口已关闭：{error}");
            }
        })
        .detach();
}

/// Keep existing transfer wait/cancel behavior, but route the actual close through the same gate.
impl AppRoot {
    fn close_after_pending_transfers(
        &mut self,
        cancel: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if cancel {
            self._transfers
                .update(cx, |manager, cx| manager.cancel_all(cx));
        }
        if self._transfers.read(cx).active_count() == 0 {
            self.finish_main_close(window, cx);
        } else {
            self.close_when_transfers_finish = true;
        }
    }

    pub(crate) fn finish_main_close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.close_when_transfers_finish = false;
        if Self::preview_has_pending_changes(self.preview_window, cx) {
            window.push_notification(
                Notification::warning("预览文件仍在保存或有未保存的改动,请先在预览窗口处理"),
                cx,
            );
            return;
        }
        let root = cx.weak_entity();
        window.defer(cx, move |window, cx| {
            let Ok((active, preview)) = root.read_with(cx, |root, cx| {
                (root._transfers.read(cx).active_count(), root.preview_window)
            }) else {
                return;
            };
            if active == 0 && !Self::preview_has_pending_changes(preview, cx) {
                let handle = window.window_handle();
                request(
                    root,
                    window,
                    cx,
                    "关闭窗口",
                    Box::new(move |cx| {
                        handle.update(cx, |_, window, _| {
                            #[cfg(target_os = "macos")]
                            native::permit();
                            window.remove_window();
                        })?;
                        Ok(())
                    }),
                );
            }
        });
    }

    pub(crate) fn should_close(root: &WeakEntity<Self>, window: &mut Window, cx: &mut App) -> bool {
        let Ok((active, preview)) = root.read_with(cx, |root, cx| {
            (root._transfers.read(cx).active_count(), root.preview_window)
        }) else {
            return true;
        };
        if Self::preview_has_pending_changes(preview, cx) {
            window.push_notification(
                Notification::warning("预览文件仍在保存或有未保存的改动,请先在预览窗口处理"),
                cx,
            );
            return false;
        }
        if close_after_transfers(active, true) {
            let handle = window.window_handle();
            request(
                root.clone(),
                window,
                cx,
                "关闭窗口",
                Box::new(move |cx| {
                    handle.update(cx, |_, window, _| {
                        #[cfg(target_os = "macos")]
                        native::permit();
                        window.remove_window();
                    })?;
                    Ok(())
                }),
            );
            return false;
        }
        if !window.has_active_dialog(cx) {
            let returning = root.clone();
            let dismissed = root.clone();
            let waiting = root.clone();
            let cancelling = root.clone();
            window.open_dialog(cx, move |dialog, _window, _cx| {
                let returning = returning.clone();
                let dismissed = dismissed.clone();
                let waiting = waiting.clone();
                let cancelling = cancelling.clone();
                dialog
                    .title("传输尚未完成")
                    .w(px(420.))
                    .child(
                        div()
                            .text_sm()
                            .child(format!("仍有 {active} 项传输在运行。")),
                    )
                    .footer(
                        DialogFooter::new()
                            .child(Button::new("transfer-exit-back").label("返回").on_click(
                                move |_, window, cx| {
                                    window.close_dialog(cx);
                                    let _ = returning.update(cx, |root, _| {
                                        root.close_when_transfers_finish = false
                                    });
                                },
                            ))
                            .child(
                                Button::new("transfer-exit-cancel")
                                    .label("取消传输并退出")
                                    .on_click(move |_, window, cx| {
                                        window.close_dialog(cx);
                                        let _ = cancelling.update(cx, |root, cx| {
                                            root.close_after_pending_transfers(true, window, cx);
                                        });
                                    }),
                            )
                            .child(
                                Button::new("transfer-exit-wait")
                                    .label("等待完成")
                                    .primary()
                                    .on_click(move |_, window, cx| {
                                        window.close_dialog(cx);
                                        let _ = waiting.update(cx, |root, cx| {
                                            root.close_after_pending_transfers(false, window, cx);
                                        });
                                    }),
                            ),
                    )
                    .on_cancel(move |_, _, cx| {
                        let _ = dismissed
                            .update(cx, |root, _| root.close_when_transfers_finish = false);
                        true
                    })
            });
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_exit_watch_uses_the_window_context_on_all_test_platforms() {
        let _: fn(WeakEntity<AppRoot>, &mut Window, &mut App) = attach_native;
    }
    #[test]
    fn confirmation_is_invalidated_by_new_transfers_or_unsaved_changes() {
        let before = Snapshot {
            transfers: 0,
            dirty: false,
            connections: vec![],
        };
        assert!(before.can_continue(&before));
        let connected = Snapshot {
            connections: vec![1_u64.into()],
            ..before.clone()
        };
        assert!(!before.can_continue(&connected));
        assert!(!connected.can_continue(&Snapshot {
            connections: vec![2_u64.into()],
            ..before.clone()
        }));
        assert!(!before.can_continue(&Snapshot {
            transfers: 1,
            ..before.clone()
        }));
        assert!(!before.can_continue(&Snapshot {
            dirty: true,
            ..before.clone()
        }));
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
    #[cfg(target_os = "macos")]
    pub(super) fn permit() {
        unsafe {
            alacrterm_exit_permit();
        }
    }
}

#[cfg(any(target_os = "macos", test))]
pub(crate) fn allow_sparkle_restart(root: &WeakEntity<AppRoot>, cx: &mut App) {
    // Sparkle may terminate asynchronously after its postpone callback returns.
    // Recheck this snapshot at the actual native quit request; do not ask twice.
    let approved = snapshot(root, cx).map(|state| (state, std::time::Instant::now()));
    cx.global_mut::<Coordinator>().native_approval = approved;
    updater::macos::allow_restart();
}

#[cfg(any(target_os = "macos", test))]
pub(crate) fn attach_native(root: WeakEntity<AppRoot>, window: &mut Window, cx: &mut App) {
    if !cx.has_global::<Coordinator>() {
        cx.set_global(Coordinator::default());
    }
    assert!(native::install(), "无法安装 macOS 退出协调器");
    window
        .spawn(cx, async move |cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(50))
                    .await;
                if cx
                    .update(|window, cx| {
                        if !native::requested() {
                            return;
                        }
                        let approved = cx.global_mut::<Coordinator>().native_approval.take();
                        if approved.is_some_and(|(before, at)| {
                            at.elapsed() < std::time::Duration::from_secs(10)
                                && snapshot(&root, cx)
                                    .is_some_and(|after| before.can_continue(&after))
                        }) {
                            native::allow();
                        } else {
                            request(
                                root.clone(),
                                window,
                                cx,
                                "退出 Alacrterm",
                                Box::new(|_| {
                                    platform::shutdown_wayx();
                                    native::allow();
                                    Ok(())
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

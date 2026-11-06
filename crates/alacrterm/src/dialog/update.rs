//! Version confirmation and download/install progress without opening Settings.
use crate::{
    AppRoot,
    updates::{self, Phase, UpdateOffer},
};
use gpui::StatefulInteractiveElement as _;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    App, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, Styled as _, Subscription, WeakEntity, Window, div, px,
};
use gpui_kit::component::{
    ActiveTheme as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    progress::Progress,
    v_flex,
};

#[derive(Default)]
struct InstallIntent {
    active: bool,
    owns_download: bool,
}
impl InstallIntent {
    fn begin(&mut self, owns_download: bool) {
        self.active = true;
        self.owns_download = owns_download;
    }
    fn cancel(&mut self) -> bool {
        self.active = false;
        std::mem::take(&mut self.owns_download)
    }
    fn prepared(&mut self, success: bool) -> bool {
        let requested = std::mem::take(&mut self.active);
        self.owns_download = false;
        success && requested
    }
}

struct UpdateDialog {
    root: WeakEntity<AppRoot>,
    updates: Entity<updates::Updates>,
    intent: InstallIntent,
    _subscription: Subscription,
}

pub(crate) fn check(root: WeakEntity<AppRoot>, window: &mut Window, cx: &mut App) {
    if window.has_active_dialog(cx) {
        return;
    }
    let model = updates::model(cx);
    if !model.read(cx).phase.has_update() {
        model.update(cx, |update, cx| update.check(cx));
    }
    open(root, window, cx);
}

pub(crate) fn open(root: WeakEntity<AppRoot>, window: &mut Window, cx: &mut App) {
    if window.has_active_dialog(cx) {
        return;
    }
    let view = cx.new(|cx| {
        let model = updates::model(cx);
        let subscription =
            cx.observe_in(&model, window, |this: &mut UpdateDialog, _, window, cx| {
                this.advance(window, cx);
                cx.notify();
            });
        UpdateDialog {
            root,
            updates: model,
            intent: InstallIntent::default(),
            _subscription: subscription,
        }
    });
    window.open_dialog(cx, move |dialog, _, _| {
        let cancelling = view.clone();
        let updating = view.clone();
        dialog
            .title("软件更新")
            .w(px(420.))
            .close_button(false)
            .overlay_closable(false)
            .child(view.clone())
            .on_ok(move |_, window, cx| {
                updating.update(cx, |this, cx| {
                    if !this.updates.read(cx).phase.has_update() && !this.updates.read(cx).busy() {
                        return true;
                    }
                    this.begin(window, cx);
                    false
                })
            })
            .on_cancel(move |_, _, cx| {
                cancelling.update(cx, |this, cx| {
                    if this.intent.active {
                        return false;
                    }
                    this.cancel(cx);
                    true
                })
            })
    });
}

impl UpdateDialog {
    fn cancel(&mut self, cx: &mut Context<Self>) {
        #[cfg(any(target_os = "macos", test))]
        if matches!(self.updates.read(cx).phase, Phase::Native(_)) {
            updater::macos::defer();
        }
        let owned = self.intent.cancel();
        if owned && matches!(self.updates.read(cx).phase, Phase::Downloading { .. }) {
            self.updates
                .update(cx, |update, cx| update.cancel_download(cx));
        }
        self.intent.owns_download = false;
    }

    fn begin(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.intent.active || self.updates.read(cx).busy() {
            return;
        }
        match self.updates.read(cx).phase.clone() {
            Phase::Available(UpdateOffer::Signed(_)) => {
                self.intent.begin(true);
                self.updates.update(cx, |update, cx| update.download(cx));
            }
            Phase::Ready { .. } => {
                self.intent.begin(false);
                self.advance(window, cx);
            }
            Phase::Available(UpdateOffer::Manual(_)) => {
                cx.open_url(updater::DOWNLOAD_PAGE);
                window.close_dialog(cx);
            }
            #[cfg(any(target_os = "macos", test))]
            Phase::Native(_) => {
                self.intent.begin(false);
                updater::macos::install();
            }
            _ => {}
        }
        cx.notify();
    }

    fn advance(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.intent.active {
            return;
        }
        match self.updates.read(cx).phase.clone() {
            Phase::Ready { .. } => {
                self.intent.owns_download = false;
                let dialog = cx.weak_entity();
                updates::install_package(
                    self.root.clone(),
                    window,
                    cx,
                    move |success, window, cx| {
                        let accepted = dialog
                            .update(cx, |dialog, _| dialog.intent.prepared(success))
                            .unwrap_or(false);
                        if accepted {
                            window.close_dialog(cx);
                        }
                        accepted
                    },
                );
            }
            Phase::Available(_) | Phase::Failed(_) | Phase::Disabled => {
                self.intent.active = false;
                self.intent.owns_download = false;
            }
            #[cfg(any(target_os = "macos", test))]
            Phase::Native(
                updater::macos::State::Failed
                | updater::macos::State::Current
                | updater::macos::State::Idle
                | updater::macos::State::Deferred,
            ) => {
                self.intent.active = false;
            }
            _ => {}
        }
    }
}

impl Render for UpdateDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let update = self.updates.read(cx);
        let available = update.phase.has_update();
        let busy = update.busy() || self.intent.active;
        let current = update.is_current();
        let notes = update.release_notes();
        v_flex()
            .gap_3()
            .child(div().text_lg().child(if current {
                "已是最新版本，暂无更新"
            } else if available && !busy {
                "有新版本发布"
            } else {
                "软件更新"
            }))
            .child(
                h_flex()
                    .justify_between()
                    .child("当前版本")
                    .child(env!("CARGO_PKG_VERSION")),
            )
            .when(available, |view| {
                view.child(
                    h_flex()
                        .justify_between()
                        .child("可升级版本")
                        .child(update.available_version().unwrap_or_default()),
                )
            })
            .when(available && !notes.is_empty(), |view| {
                view.child(
                    v_flex().gap_2().child("本次更新").child(
                        div()
                            .id("update-release-notes")
                            .max_h(px(240.))
                            .overflow_y_scroll()
                            .text_sm()
                            .child(notes),
                    ),
                )
            })
            .when(busy, |view| {
                view.child(
                    Progress::new("update-progress")
                        .loading(update.progress().is_none())
                        .value(update.progress().unwrap_or_default()),
                )
                .child(div().text_sm().child(update.status()))
            })
            .when(!busy && !current && !available, |view| {
                view.child(update.status())
            })
            .when(!update.notice.is_empty() && !current, |view| {
                view.child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(update.notice.clone()),
                )
            })
            .child(
                h_flex()
                    .w_full()
                    .justify_between()
                    .gap_2()
                    .when(available && !busy, |row| {
                        row.child(
                            Button::new("postpone-software-update")
                                .label("稍后更新")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.cancel(cx);
                                    window.close_dialog(cx);
                                })),
                        )
                        .child(
                            Button::new("install-software-update")
                                .label("更新并重启")
                                .primary()
                                .on_click(
                                    cx.listener(|this, _, window, cx| this.begin(window, cx)),
                                ),
                        )
                    })
                    .when(!available && !busy, |row| {
                        row.child(
                            Button::new("confirm-software-update")
                                .label("确定")
                                .primary()
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        )
                    }),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::InstallIntent;
    #[test]
    fn cancelled_or_failed_preparation_cannot_continue_installing() {
        let mut intent = InstallIntent::default();
        intent.begin(true);
        assert!(intent.cancel());
        assert!(!intent.prepared(true));
        intent.begin(false);
        assert!(!intent.prepared(false));
        assert!(!intent.prepared(true));
        intent.begin(false);
        assert!(intent.prepared(true));
        assert!(!intent.prepared(true));
    }
    #[test]
    fn cancellation_only_owns_downloads_started_by_this_dialog() {
        let mut intent = InstallIntent::default();
        assert!(!intent.cancel());
        intent.begin(false);
        assert!(!intent.cancel());
        intent.begin(true);
        assert!(intent.cancel());
        assert!(!intent.cancel());
    }
}

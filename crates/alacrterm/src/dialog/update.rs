//! Version confirmation and download/install progress without opening Settings.
use crate::{
    AppRoot,
    updates::{self, Phase, UpdateOffer},
};
use gpui::prelude::FluentBuilder as _;
use gpui::{
    App, AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, Styled as _,
    Subscription, WeakEntity, Window, div, px,
};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    h_flex, v_flex,
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

pub(crate) fn open(root: WeakEntity<AppRoot>, window: &mut Window, cx: &mut App) {
    if window.has_active_dialog(cx) || !updates::model(cx).read(cx).phase.has_update() {
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
                updating.update(cx, |this, cx| this.begin(window, cx));
                false
            })
            .on_cancel(move |_, _, cx| {
                cancelling.update(cx, |this, cx| this.cancel(cx));
                true
            })
    });
}

impl UpdateDialog {
    fn cancel(&mut self, cx: &mut Context<Self>) {
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
                // Sparkle owns its native download/installation UI and restart handoff.
                self.updates.update(cx, |update, cx| update.check(cx));
                window.close_dialog(cx);
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
            _ => {}
        }
    }
}

impl Render for UpdateDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let update = self.updates.read(cx);
        let target = update
            .available_version()
            .unwrap_or_else(|| "暂不可用".into());
        let busy = update.busy() || self.intent.active;
        let manual = matches!(update.phase, Phase::Available(UpdateOffer::Manual(_)));
        let notice = update.notice.clone();
        v_flex()
            .gap_3()
            .child(
                h_flex()
                    .justify_between()
                    .child("当前版本")
                    .child(env!("CARGO_PKG_VERSION")),
            )
            .child(h_flex().justify_between().child("可升级版本").child(target))
            .when(busy, |view| {
                view.child(div().text_sm().child(update.status()))
            })
            .when(manual, |view| {
                view.child(
                    div()
                        .text_sm()
                        .child("此版本需要手动安装，立刻更新将打开官方下载页。"),
                )
            })
            .when(!notice.is_empty(), |view| {
                view.child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(notice),
                )
            })
            .child(
                h_flex()
                    .w_full()
                    .justify_end()
                    .gap_2()
                    .child(
                        Button::new("cancel-software-update")
                            .label("取消")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.cancel(cx);
                                window.close_dialog(cx);
                            })),
                    )
                    .child(
                        Button::new("install-software-update")
                            .label("立刻更新")
                            .primary()
                            .disabled(busy || !update.phase.has_update())
                            .on_click(cx.listener(|this, _, window, cx| this.begin(window, cx))),
                    ),
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

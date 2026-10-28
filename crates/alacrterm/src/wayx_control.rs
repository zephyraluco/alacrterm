//! Application-level WayX control: icon-only status and explicit confirmation.
use crate::{AppRoot, assets::IconName};
use gpui::{
    App, Context, InteractiveElement as _, IntoElement, MouseButton, ParentElement as _,
    Styled as _, Window, div, px,
};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, WindowExt as _,
    button::{Button, ButtonVariants as _},
    dialog::{DialogAction, DialogClose, DialogFooter},
};
use platform::WayxStatus;

impl AppRoot {
    pub(crate) fn wayx_button(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let (running, stopped, danger) = {
            let theme = cx.theme();
            (
                theme.secondary_foreground,
                theme.muted_foreground,
                theme.danger,
            )
        };
        let (icon, label) = match self.wayx_status {
            WayxStatus::Stopped => (Icon::new(IconName::X11).text_color(stopped), "启动 WayX"),
            WayxStatus::Starting => (Icon::new(IconName::LoaderCircle), "WayX 正在启动"),
            WayxStatus::Running => (Icon::new(IconName::X11).text_color(running), "停止 WayX"),
            WayxStatus::Stopping => (Icon::new(IconName::Loader), "WayX 正在停止"),
            WayxStatus::Failed => (
                Icon::new(IconName::TriangleAlert).text_color(danger),
                "重试启动 WayX",
            ),
        };
        // Occlusion excludes the ancestor's Drag hitbox from native Windows
        // hit testing. Stopping propagation also prevents title-bar movement
        // when an ordinary mouse event reaches the control on Linux/macOS.
        div()
            .id("wayx-control-area")
            .occlude()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                Button::new("wayx-control")
                    .icon(icon)
                    .ghost()
                    .accessibility_label(label)
                    .disabled(matches!(
                        self.wayx_status,
                        WayxStatus::Starting | WayxStatus::Stopping
                    ))
                    .on_click(cx.listener(|_, _, window, cx| Self::confirm_wayx(window, cx))),
            )
    }

    fn confirm_wayx(window: &mut Window, cx: &mut App) {
        let status = platform::wayx_status();
        if matches!(status, WayxStatus::Starting | WayxStatus::Stopping) {
            return;
        }
        let stopping = status == WayxStatus::Running;
        window.open_dialog(cx, move |dialog, _, _| {
            dialog
                .title(if stopping {
                    "停止 WayX？"
                } else {
                    "启动 WayX？"
                })
                .w(px(400.))
                .child(div().text_sm().child(if stopping {
                    "停止后，远端图形窗口将关闭。SSH 终端和 SFTP 连接不受影响。"
                } else {
                    "启动本地图形显示服务，供 SSH 会话运行远端图形应用。"
                }))
                .footer(
                    DialogFooter::new()
                        .child(DialogClose::new().child(Button::new("wayx-cancel").label("取消")))
                        .child(
                            DialogAction::new().child(
                                Button::new("wayx-confirm")
                                    .label(if stopping { "停止" } else { "启动" })
                                    .primary(),
                            ),
                        ),
                )
                .on_ok(move |_, _, _| {
                    // Recheck state in the service; a stale dialog cannot create duplicates.
                    if stopping {
                        platform::stop_wayx();
                    } else {
                        platform::start_wayx();
                    }
                    true
                })
        });
    }
}

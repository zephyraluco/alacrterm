//! 「会话信息」视图:当前会话的只读信息(名称 / 连接 / 进程 / 状态)。
//!
//! 内容就是一个 [`SidebarMenu`](gpui_kit::component::sidebar::SidebarMenu),
//! 会话为空(标签页全部关闭)时四项都显示 `--`,避免面板看上去是空的。
//! 「当前会话」指 dock 里那块活动面板(`AppRoot::terminals[AppRoot::active]`),
//! 与会话列表(记录,见 [`super::sessions`])无关。

use gpui::{App, SharedString};
use gpui_kit::component::sidebar::{SidebarMenu, SidebarMenuItem};

use crate::AppRoot;
use crate::assets::IconName;
impl AppRoot {
    /// 「会话信息」视图的内容：当前会话的只读信息（名称 / 连接 / 进程 / 状态）。
    ///
    /// 无会话（标签页全部关闭）时四项都显示 `--`，避免面板看上去是空的。
    ///
    /// 只读 [`App`]：调用方是「会话信息」标签所在的那条侧边栏实体，它只持有根视图的
    /// 弱引用（见 `sidebar_panel::Sidebar` 的字段说明），拿不到 `&mut Context<AppRoot>`。
    pub(super) fn render_session_info_menu(&self, cx: &App) -> SidebarMenu {
        let (name, target, pid, state) = match self.terminals.get(self.active) {
            Some(session) => {
                let view = session.view.read(cx);
                let pid = view
                    .pid(cx)
                    .map(|pid| pid.to_string())
                    .unwrap_or_else(|| "--".to_string());
                let state = if view.has_exited() {
                    "已断开"
                } else {
                    "运行中"
                };
                (
                    session.title(cx),
                    session.target(cx).label(),
                    pid,
                    state.to_string(),
                )
            }
            None => (
                SharedString::from("--"),
                "--".to_string(),
                "--".to_string(),
                "无会话".to_string(),
            ),
        };

        SidebarMenu::new().child(
            SidebarMenuItem::new("当前会话")
                .icon(IconName::SquareTerminal)
                .default_open(true)
                .click_to_toggle(true)
                .children([
                    SidebarMenuItem::new(format!("名称：{name}")).disable(true),
                    SidebarMenuItem::new(format!("连接：{target}")).disable(true),
                    SidebarMenuItem::new(format!("进程：{pid}")).disable(true),
                    SidebarMenuItem::new(format!("状态：{state}")).disable(true),
                ]),
        )
    }
}

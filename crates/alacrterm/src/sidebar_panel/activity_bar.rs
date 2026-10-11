//! 最左侧的 **VS Code 式活动栏**:一列图标(每个视图一枚)+ 底部一枚「设置」。
//!
//! 外观全交给 gpui-kit 的 **`Sidebar` 组件**(`gpui_kit::component::sidebar`),这里用它的
//! **图标折叠态**:`SidebarCollapsible::Icon`(组件的默认值)+ `collapsed(true)`,宽度取
//! 48px(= 组件内部的 `COLLAPSED_WIDTH`)。这个形态下组件自己会把内容区收成 `p_2`、
//! 让 `SidebarMenuItem` **只摆图标并居中**,还自带**悬停高亮 / `active` 底色 /
//! 鼠标悬停时的右侧 tooltip**(见 `sidebar/menu.rs` 的 `collapsed_tooltip`)——
//! 所以这里不再手绘图标按钮,只是把「每个视图一枚 `SidebarMenuItem`」摆进一个常驻的窄
//! `Sidebar`,底部「设置」放进组件的 `footer` 槽(贴底)。
//!
//! 它**常驻**(根视图把它摆在分栏组**之外**的最左一列,折叠侧边栏时它还在,否则就没有
//! 恢复入口了)。点击语义是本应用自己的(见 [`Sidebar::toggle_view`]):点一枚 = 让
//! **左侧边栏**显示这个视图(本侧没有就从右侧搬过来);再点当前**正在显示**的那枚 =
//! 折叠侧边栏 —— 照 VS Code,折叠不另设开关。高亮的即左侧边栏当前显示的视图;
//! 侧边栏折叠时一枚都不亮(照 VS Code)。

use gpui::{
    AnyElement, Context, Entity, IntoElement, ParentElement as _, Pixels, Styled as _, div, px,
};
use gpui_kit::component::{
    ActiveTheme as _, Icon, Sizable as _,
    button::{Button, ButtonVariants as _},
    sidebar::{Sidebar as SidebarWidget, SidebarMenu, SidebarMenuItem},
};

use super::{Sidebar, SidebarView};
use crate::AppRoot;
use crate::assets::IconName;

/// 活动栏宽度(VS Code 是 48px;与 gpui-kit `Sidebar` 的图标折叠宽度一致)。
pub(super) const ACTIVITY_BAR_WIDTH: Pixels = px(48.);
/// 活动项图标的尺寸:24px 见方(与 VS Code 的活动项图标一致)。
const ACTIVITY_ICON_SIZE: Pixels = px(24.);
/// 底部「设置」按钮的尺寸:32px 见方 ⇒ 图标 24px(`Button` 会把图标设成按钮尺寸 × 0.75)。
const ACTIVITY_SETTINGS_SIZE: Pixels = px(32.);

/// 活动栏(根视图把它摆在主体最左一列)。
///
/// 状态都在 `read` 的短借用里取完再建元素 —— `cx.listener` 还要接着用 `cx`。
pub(crate) fn activity_bar(sidebar: &Entity<Sidebar>, cx: &mut Context<AppRoot>) -> AnyElement {
    let (files_enabled, active) = {
        let sidebar = sidebar.read(cx);
        // 折叠时一枚都不亮:亮着的那枚是「当前**显示**的视图」。
        let active = if sidebar.visible {
            sidebar.active_view()
        } else {
            None
        };
        (sidebar.files_enabled, active)
    };
    let items: Vec<SidebarMenuItem> = SidebarView::ALL
        .iter()
        .copied()
        .filter(|view| view.visible_with(files_enabled))
        .map(|view| activity_item(sidebar, view, active == Some(view)))
        .collect();

    SidebarWidget::new("activity-bar")
        .w(ACTIVITY_BAR_WIDTH)
        // 图标折叠态(`collapsible` 默认就是 `SidebarCollapsible::Icon`,这里只钉住折叠)。
        .collapsed(true)
        // 与标题栏同色 ⇒ 视觉上接着标题栏那一层(VS Code 的标题栏与活动栏是同一个色号)。
        .bg(cx.theme().title_bar)
        .child(SidebarMenu::new().children(items))
        .footer(settings_item(cx))
        .into_any_element()
}

/// 一枚活动项:一个视图的图标(文字只在折叠态的鼠标提示里出现)。
fn activity_item(sidebar: &Entity<Sidebar>, view: SidebarView, active: bool) -> SidebarMenuItem {
    let sidebar = sidebar.downgrade();
    SidebarMenuItem::new(view.label())
        .icon(Icon::new(view.icon()).with_size(ACTIVITY_ICON_SIZE))
        .active(active)
        // 图标态没有文字,给无障碍留一句完整说明。
        .accessibility_label(view.tooltip())
        .on_click(move |_, _, cx| {
            let _ = sidebar.update(cx, |sidebar, cx| sidebar.toggle_view(view, cx));
        })
}

/// 活动栏底部的「设置」入口(原先是标题栏左端的文字按钮,按 VS Code 挪到这儿)。
///
/// ⚠️ 这里用 `Button`、不是 `SidebarMenuItem`:后者只实现 `SidebarItem`(没有 `IntoElement`),
/// 而 `footer` 槽收的是普通元素;`SidebarFooter` 还会再套一层 `p_2`(48px 栏里就只剩 16px),
/// 所以自己摆一行、把按钮居中(按钮图标 24px = 与菜单项的图标同尺寸)。
fn settings_item(cx: &mut Context<AppRoot>) -> AnyElement {
    div()
        .w_full()
        .flex()
        .justify_center()
        .child(
            Button::new("activity-settings")
                .ghost()
                .with_size(ACTIVITY_SETTINGS_SIZE)
                .icon(IconName::Settings)
                .tooltip("设置")
                .on_click(cx.listener(|this, _, window, cx| this.open_settings_window(window, cx))),
        )
        .into_any_element()
}

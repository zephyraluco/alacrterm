//! 独立窗口形式的「设置」界面。
//!
//! 由活动栏底部的设置图标触发（[`AppRoot::open_settings_window`]）。
//!
//! 这里刻意**不使用对话框**：设置内容较多，独立窗口可以自由调整大小、独立摆放，
//! 也不会遮住终端内容。窗口句柄记录在 [`AppRoot`] 上，重复点击设置图标只会激活
//! 已打开的窗口，而不会开出多个；窗口被用户关闭后再点击会重新打开。
//!
//! 主题是全局状态（`Theme::global`），因此设置窗口与主窗口共享同一套配色，
//! 切换深浅色后需要 `cx.refresh_windows()` 让所有窗口（含本窗口）重绘。

use gpui::{
    App, AppContext as _, Bounds, Context, IntoElement, ParentElement as _, Render, Styled as _,
    Window, WindowBounds, WindowOptions, div, px, size,
};
use gpui_kit::component::{
    ActiveTheme as _, Icon, Root, Theme, ThemeMode, TitleBar, h_flex,
    setting::{SettingField, SettingGroup, SettingItem, SettingPage, Settings},
    v_flex,
};

use crate::AppRoot;
use crate::assets::IconName;

/// 设置窗口的初始尺寸（宽, 高）。
const WINDOW_SIZE: (f32, f32) = (760., 520.);
/// 设置窗口的最小尺寸（宽, 高）：`Settings` 内部是左右分栏，过窄会挤在一起。
const MIN_WINDOW_SIZE: (f32, f32) = (560., 400.);

/// 设置窗口的根视图（内容即 `Settings` 组件）。
///
/// 无自有状态：`Settings` 把搜索框等状态存在窗口的 keyed state 里。
struct SettingsWindow;

impl Render for SettingsWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .size_full()
            .bg(cx.theme().background)
            // —— 顶部：自绘标题栏 ——
            // 不用系统标题栏：Windows 下它的颜色跟随系统「浅色/深色」设置，
            // 应用是暗色主题时会出现一条白条。这里用 gpui-kit 的 TitleBar 自绘，
            // 配色取自当前主题，与主窗口保持一致（拖拽 / 双击最大化 / 窗口按钮由它处理）。
            .child(
                TitleBar::new().child(
                    h_flex()
                        .w_full()
                        .px(px(8.))
                        .gap(px(8.))
                        .items_center()
                        .child(
                            div()
                                .flex_1()
                                .overflow_hidden()
                                .text_size(px(13.))
                                .text_color(cx.theme().secondary_foreground)
                                .child("设置"),
                        ),
                ),
            )
            .child(
                // Settings 渲染为 h_resizable（左右分栏），需要父级给出确定高度；
                // flex_1 让它在窗口内撑满，min_h_0 允许收缩。
                div()
                    .flex_1()
                    .min_h_0()
                    .p_4()
                    .child(
                        Settings::new("app-settings").page(
                            SettingPage::new("外观")
                                .icon(Icon::new(IconName::Moon))
                                .default_open(true)
                                .group(
                                    SettingGroup::new().item(
                                        SettingItem::new(
                                            "深色主题",
                                            SettingField::switch(
                                                // 以全局 Theme 为唯一状态来源。
                                                |cx: &App| cx.theme().mode.is_dark(),
                                                |val: bool, cx: &mut App| {
                                                    let mode = if val {
                                                        ThemeMode::Dark
                                                    } else {
                                                        ThemeMode::Light
                                                    };
                                                    // Theme::change 传 None 时不会自动刷新窗口，
                                                    // 需手动刷新所有窗口（含本设置窗口）。
                                                    Theme::change(mode, None, cx);
                                                    cx.refresh_windows();
                                                },
                                            ),
                                        )
                                        .description(
                                            "切换深色 / 浅色界面主题，与终端背景保持一致。",
                                        ),
                                    ),
                                ),
                        ),
                    ),
            )
            // 与主窗口保持一致：gpui-kit 0.6 的 Root 不会自动渲染 Dialog 层，
            // 需在渲染树中显式挂载（当前设置项没用到弹窗，但可避免以后加字段时踩坑）。
            .children(Root::render_dialog_layer(window, cx))
    }
}

impl AppRoot {
    /// 活动栏底部设置图标：打开设置窗口；若已打开则激活它。
    pub(crate) fn open_settings_window(&mut self, cx: &mut Context<Self>) {
        // 已打开过：直接激活原窗口。窗口被关闭后 `update` 会返回 Err，
        // 于是继续往下走新建流程。
        if let Some(handle) = self.settings_window {
            if handle
                .update(cx, |_, window, _| window.activate_window())
                .is_ok()
            {
                return;
            }
        }

        let bounds = Bounds::centered(None, size(px(WINDOW_SIZE.0), px(WINDOW_SIZE.1)), cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            window_min_size: Some(size(px(MIN_WINDOW_SIZE.0), px(MIN_WINDOW_SIZE.1))),
            // 隐藏系统标题栏，改由视图内的自绘 `TitleBar` 负责
            // （同时设置 app_owns_titlebar_drag，拖拽/双击最大化都由它处理）。
            ..TitleBar::window_options()
        };

        // 与主窗口一致，外层包一层 gpui-kit Root（弹窗 / 通知 / 焦点恢复的宿主）。
        match cx.open_window(options, |window, cx| {
            let view = cx.new(|_| SettingsWindow);
            cx.new(|cx| Root::new(view, window, cx))
        }) {
            Ok(handle) => self.settings_window = Some(handle),
            Err(err) => log::error!("failed to open settings window: {err:#}"),
        }
    }
}

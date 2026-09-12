//! 独立窗口形式的「设置」界面。
//!
//! 由标题栏右侧的「设置」文字按钮触发（[`AppRoot::open_settings_window`]）；
//! 欢迎页的那一行「打开设置」也走同一入口。
//!
//! 它是一个**独立顶层窗口**（[`WindowKind::Normal`]）：任务栏里有自己的条目、
//! 不受主窗口置顶约束、不模态（打开期间主窗口照常可用），可以像另一个程序那样
//! 单独最小化 / 切换。
//!
//! 代价是：系统不再替我们把它与主窗口绑定。原来的 `WindowKind::Dialog` 是
//! 「主窗口的从属（模态）子窗口」（owner + `EnableWindow(parent, false)`），
//! 不占任务栏、始终压在主窗口之上、**随主窗口一起关闭**——独立之后就都没有了。
//! 因此**「主程序退出时一并关闭」由窗口自己负责**：视图创建时记下主窗口句柄，
//! 用 `App::on_window_closed` 盯住它，主窗口一关就 `App::quit()`（见
//! [`SettingsWindow::new`]）。不这么做的话，设置窗口会变成唯一活着的窗口，
//! `QuitMode::LastWindowClosed` 会留着进程不放：主窗口关掉了、任务栏里还挂着一个空壳。
//!
//! 之所以仍然用独立窗口而不是应用内对话框：设置内容较多，独立窗口可以自由
//! 调整大小、不与终端挤在同一条渲染树里，也不会遮住终端内容。
//!
//! 窗口句柄记录在 [`AppRoot`] 上，重复点击设置入口只会 `activate_window`，
//! 不会开出多个；窗口被用户关闭后再点击会重新开一个。
//!
//! 主题是全局状态（`Theme::global`），因此设置窗口与主窗口共享同一套配色，
//! 切换深浅色后需要 `cx.refresh_windows()` 让所有窗口（含本窗口）重绘。

use gpui::{
    AnyWindowHandle, App, AppContext as _, Bounds, Context, IntoElement, ParentElement as _,
    Render, Styled as _, Subscription, Window, WindowBounds, WindowKind, WindowOptions, div, px,
    size,
};
use gpui_kit::component::{
    ActiveTheme as _, Icon, Root, ThemeMode, TitleBar, h_flex,
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
/// `Settings` 把搜索框等状态存在窗口的 keyed state 里，所以视图几乎没有自有状态；
/// 唯一的字段是「主窗口已关闭」的观察句柄——`Subscription` 是 RAII 的，被丢掉即
/// 解除订阅，所以必须由视图持有（见 [`SettingsWindow::new`]）。
struct SettingsWindow {
    _main_window_closed: Subscription,
}

impl SettingsWindow {
    /// `main_window` = 主窗口句柄（由 [`AppRoot::open_settings_window`] 传入）。
    ///
    /// 设置窗口是独立顶层窗口，系统不会因 owner 消失而连带关闭它，所以这里自己
    /// 盯住主窗口：主窗口一关就 `App::quit()`，让主程序连同设置窗口一起退出。
    /// 这比反过来“在主窗口关闭时去关设置窗口”简单：不用跨窗口取句柄。
    fn new(main_window: AnyWindowHandle, cx: &mut Context<Self>) -> Self {
        let main_window_id = main_window.window_id();
        Self {
            _main_window_closed: cx.on_window_closed(move |cx, closed| {
                if closed == main_window_id {
                    cx.quit();
                }
            }),
        }
    }
}

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
                                                    // 统一入口：应用主题并重新压下
                                                    // 分栏拖拽条线条的透明覆盖（见
                                                    // `crate::change_theme` 的说明）。
                                                    // 传 None 时不会自动刷新窗口，
                                                    // 需手动刷新所有窗口（含本设置窗口）。
                                                    crate::change_theme(mode, cx);
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
    /// 标题栏右侧的「设置」文字按钮（以及欢迎页那一行）：打开设置窗口；
    /// 若已打开则激活它。
    ///
    /// `window` 是主窗口：设置窗口要靠它的句柄跟随主程序退出
    /// （见 [`SettingsWindow::new`]）。
    pub(crate) fn open_settings_window(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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

        let main_window = window.window_handle();
        let bounds = Bounds::centered(None, size(px(WINDOW_SIZE.0), px(WINDOW_SIZE.1)), cx);
        let mut options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            window_min_size: Some(size(px(MIN_WINDOW_SIZE.0), px(MIN_WINDOW_SIZE.1))),
            // 独立顶层窗口：任务栏里有自己的条目、不模态、不随主窗口置顶。
            // ⚠️ 别改回 `WindowKind::Dialog`：那是「主窗口的从属（模态）子窗口」，
            // Windows 后端会取当前活动窗口当 owner 并 `EnableWindow(parent, false)`
            // 锁住主窗口（见模块文档里的取舍说明）。
            kind: WindowKind::Normal,
            // 隐藏系统标题栏，改由视图内的自绘 `TitleBar` 负责
            // （同时设置 app_owns_titlebar_drag，拖拽/双击最大化都由它处理）。
            ..TitleBar::window_options()
        };
        // 独立窗口会在任务栏里露出条目，得有个名字；
        // 自绘标题栏本身不显示系统标题，这里只影响任务栏 / Alt-Tab。
        if let Some(titlebar) = options.titlebar.as_mut() {
            titlebar.title = Some("设置".into());
        }

        // 与主窗口一致，外层包一层 gpui-kit Root（弹窗 / 通知 / 焦点恢复的宿主）。
        match cx.open_window(options, |window, cx| {
            let view = cx.new(|cx| SettingsWindow::new(main_window, cx));
            cx.new(|cx| Root::new(view, window, cx))
        }) {
            Ok(handle) => self.settings_window = Some(handle),
            Err(err) => log::error!("failed to open settings window: {err:#}"),
        }
    }
}

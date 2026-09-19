//! 独立窗口形式的「设置」界面：两页（主题 / 终端），由标题栏的「设置」按钮或欢迎页
//! 同一入口打开（[`AppRoot::open_settings_window`]）。
//!
//! 它是**独立顶层窗口**（[`WindowKind::Normal`]）：任务栏里有自己的条目、不模态；
//! 「主程序退出时一并关闭」由窗口自己负责（视图用 `App::on_window_closed` 盯住主窗口，
//! 见 [`SettingsWindow::new`]）。
//!
//! 窗口句柄记在 [`AppRoot`] 上，重复点击只会激活已有窗口。主题是全局状态
//! （`Theme::global`），切换深浅色后要 `cx.refresh_windows()` 让两个窗口都重绘。

use gpui::{
    AnyWindowHandle, App, AppContext as _, Bounds, Context, FontWeight, IntoElement,
    ParentElement as _, Render, SharedString, Styled as _, Subscription, WeakEntity, Window,
    WindowBounds, WindowKind, WindowOptions, div, px, size,
};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Root, Sizable as _, ThemeMode, TitleBar, h_flex,
    setting::{
        AnySettingField, NumberFieldOptions, SettingField, SettingGroup, SettingItem, SettingPage,
        Settings,
    },
    switch::Switch,
    v_flex,
};
use terminal_view::RenderSettings;

use crate::AppRoot;
use crate::assets::IconName;
use crate::config;

/// 设置窗口的初始尺寸（宽, 高）。
const WINDOW_SIZE: (f32, f32) = (760., 520.);
/// 设置窗口的最小尺寸（宽, 高）：`Settings` 内部是左右分栏，过窄会挤在一起。
const MIN_WINDOW_SIZE: (f32, f32) = (560., 400.);

/// 设置窗口的根视图（内容即 `Settings` 组件）。
///
/// 除「主窗口已关闭」的观察句柄外，只持主窗口根视图的弱引用：终端渲染参数存在每个
/// `TerminalView` 里，要即时生效得经 [`AppRoot::apply_render_settings`] 转发。
struct SettingsWindow {
    app_root: WeakEntity<AppRoot>,
    _main_window_closed: Subscription,
}

impl SettingsWindow {
    /// `main_window` = 主窗口句柄（由 [`AppRoot::open_settings_window`] 传入）。
    ///
    /// 盯住主窗口：它一关就 `App::quit()`，让设置窗口跟着主程序一起退出。
    fn new(
        main_window: AnyWindowHandle,
        app_root: WeakEntity<AppRoot>,
        cx: &mut Context<Self>,
    ) -> Self {
        let main_window_id = main_window.window_id();
        Self {
            app_root,
            _main_window_closed: cx.on_window_closed(move |cx, closed| {
                if closed == main_window_id {
                    cx.quit();
                }
            }),
        }
    }

    /// 「主题」页：深浅模式开关 + 深 / 浅两个槽位各用哪套配色。
    fn theme_page(&self, cx: &App) -> SettingPage {
        SettingPage::new("主题")
            .icon(Icon::new(IconName::Moon))
            .default_open(true)
            .group(
                SettingGroup::new()
                    .item(
                        SettingItem::new("深色主题", theme_mode_switch())
                            .description("切换深色 / 浅色界面主题，与终端背景保持一致。"),
                    )
                    .item(
                        SettingItem::new("深色模式配色", theme_dropdown(cx, ThemeMode::Dark))
                            .description("深色模式下用哪套主题；默认「跟随 gpui-kit」用内置配色。"),
                    )
                    .item(
                        SettingItem::new("浅色模式配色", theme_dropdown(cx, ThemeMode::Light))
                            .description("同上，作用于浅色模式。主题文件在 themes/ 下，改完即刻热重载。"),
                    ),
            )
    }

    /// 「终端」页：字体与光标相关的渲染参数（只列界面能表达的项；ANSI 配色仍写在
    /// `config/terminal.json` 里，本页保存时不动它们）。
    fn terminal_page(&self) -> SettingPage {
        SettingPage::new("终端")
            .icon(Icon::new(IconName::SquareTerminal))
            .default_open(true)
            .group(
                SettingGroup::new()
                    .item(render_item(
                        "字体",
                        "终端字体族（系统里已安装的等宽字体）。",
                        {
                            let root = self.app_root.clone();
                            SettingField::input(
                                |cx: &App| config::settings(cx).render.font_family.clone(),
                                move |value: SharedString, cx: &mut App| {
                                    update_render(&root, cx, |render| render.font_family = value);
                                },
                            )
                        },
                    ))
                    .item(render_item(
                        "字号",
                        "终端字体大小（逻辑像素），6 ~ 48。",
                        {
                            let root = self.app_root.clone();
                            SettingField::number_input(
                                NumberFieldOptions {
                                    min: 6.0,
                                    max: 48.0,
                                    step: 1.0,
                                },
                                |cx: &App| {
                                    config::as_number(config::settings(cx).render.font_size.as_f32())
                                },
                                move |value: f64, cx: &mut App| {
                                    update_render(&root, cx, |render| render.font_size = px(value as f32));
                                },
                            )
                        },
                    ))
                    .item(render_item(
                        "字重",
                        "100 ~ 900，常用 400（常规）/ 700（粗体）。",
                        {
                            let root = self.app_root.clone();
                            SettingField::number_input(
                                NumberFieldOptions {
                                    min: 100.0,
                                    max: 900.0,
                                    step: 100.0,
                                },
                                |cx: &App| {
                                    config::as_number(config::settings(cx).render.font_weight.0)
                                },
                                move |value: f64, cx: &mut App| {
                                    update_render(&root, cx, |render| {
                                        render.font_weight = FontWeight(value as f32)
                                    });
                                },
                            )
                        },
                    ))
                    .item(render_item(
                        "行高倍数",
                        "行高 = 字号 × 该倍数，0.8 ~ 2.5（小于 1.0 会重叠）。",
                        {
                            let root = self.app_root.clone();
                            SettingField::number_input(
                                NumberFieldOptions {
                                    min: 0.8,
                                    max: 2.5,
                                    step: 0.05,
                                },
                                |cx: &App| {
                                    config::as_number(config::settings(cx).render.line_height_multiplier)
                                },
                                move |value: f64, cx: &mut App| {
                                    update_render(&root, cx, |render| {
                                        render.line_height_multiplier = value as f32
                                    });
                                },
                            )
                        },
                    ))
                    .item(render_item(
                        "最小对比度",
                        "APCA 最小对比度 Lc（0 = 关闭），0 ~ 106；过低的前景色会被抬亮。",
                        {
                            let root = self.app_root.clone();
                            SettingField::number_input(
                                NumberFieldOptions {
                                    min: 0.0,
                                    max: 106.0,
                                    step: 5.0,
                                },
                                |cx: &App| {
                                    config::as_number(config::settings(cx).render.minimum_contrast)
                                },
                                move |value: f64, cx: &mut App| {
                                    update_render(&root, cx, |render| {
                                        render.minimum_contrast = value as f32
                                    });
                                },
                            )
                        },
                    ))
                    .item(render_item(
                        "光标形状",
                        "终端应用没有用 DECSCUSR 指定形状时用这个。",
                        {
                            let root = self.app_root.clone();
                            SettingField::dropdown(
                                cursor_shape_options(),
                                |cx: &App| {
                                    config::cursor_shape_name(config::settings(cx).render.cursor_shape)
                                        .into()
                                },
                                move |value: SharedString, cx: &mut App| {
                                    let Some(shape) = config::parse_cursor_shape(&value) else {
                                        return;
                                    };
                                    update_render(&root, cx, |render| render.cursor_shape = shape);
                                },
                            )
                        },
                    ))
                    .item(render_item(
                        "光标闪烁",
                        "关闭后光标常亮（终端应用自己控制光标时不受影响）。",
                        {
                            let root = self.app_root.clone();
                            SettingField::switch(
                                |cx: &App| config::settings(cx).render.cursor_blinks,
                                move |value: bool, cx: &mut App| {
                                    update_render(&root, cx, |render| render.cursor_blinks = value);
                                },
                            )
                        },
                    )),
            )
    }
}

/// 「深色主题」开关。
///
/// ⚠️ 不用 `SettingField::switch`：它的弹簧动画会让另一个窗口在动画期间不重绘。
/// 这里把 element id 绑上当前模式（换模式 = 换新 id ⇒ 弹簧状态新建即已就位），
/// 两个窗口同一帧一起变色；外观与其它开关一致（同一个 `Switch` 组件）。
fn theme_mode_switch() -> SettingField<SharedString> {
    SettingField::render(move |options, _window, cx| {
        let dark = cx.theme().mode.is_dark();
        Switch::new(("theme-mode-switch", usize::from(dark)))
            .checked(dark)
            .disabled(options.is_disabled())
            .with_size(options.size())
            .on_click(move |next: &bool, _window, cx| {
                let mode = if *next {
                    ThemeMode::Dark
                } else {
                    ThemeMode::Light
                };
                // 统一入口（见 `config::change_theme`）；`None` 窗口参数不会自动刷新，
                // 需手动刷新所有窗口。
                config::change_theme(mode, cx);
                cx.refresh_windows();
            })
            .into_any_element()
    })
}

/// 主题下拉框：值 = 注册表里的主题名，空串 = 跟随 gpui-kit 内置主题。
/// 候选项每帧重算，所以 `themes/` 里增删文件后打开本页就能看到最新列表。
fn theme_dropdown(cx: &App, mode: ThemeMode) -> SettingField<SharedString> {
    SettingField::scrollable_dropdown(
        config::theme_options(cx, mode),
        move |cx: &App| {
            let settings = config::settings(cx);
            let name = if mode.is_dark() {
                &settings.dark_theme
            } else {
                &settings.light_theme
            };
            name.clone().unwrap_or_default()
        },
        move |value: SharedString, cx: &mut App| {
            let name = (!value.is_empty()).then_some(value);
            config::set_theme(name, mode, cx);
        },
    )
}

/// 终端渲染参数项：统一包一层 description（`SettingItem` 是链式构造的）。
fn render_item(
    title: &'static str,
    description: &'static str,
    field: impl AnySettingField + 'static,
) -> SettingItem {
    SettingItem::new(title, field).description(description)
}

/// 光标形状下拉框的候选项（值 = 配置文件里的写法）。
fn cursor_shape_options() -> Vec<(SharedString, SharedString)> {
    [
        ("Block", "方块"),
        ("Underline", "下划线"),
        ("Bar", "竖线"),
        ("HollowBlock", "空心方块"),
        ("Hidden", "隐藏"),
    ]
    .into_iter()
    .map(|(value, label)| (value.into(), format!("{label}（{value}）").into()))
    .collect()
}

/// 改一项终端渲染参数：更新全局 → 落盘 → 推给存活会话
/// （最后一步经 [`AppRoot::apply_render_settings`] 转发）。
fn update_render(
    root: &WeakEntity<AppRoot>,
    cx: &mut App,
    change: impl FnOnce(&mut RenderSettings),
) {
    let mut settings = config::settings(cx).render.clone();
    change(&mut settings);
    config::settings_mut(cx).render = settings.clone();
    config::save_render_settings(&settings);
    if let Some(root) = root.upgrade() {
        root.update(cx, |root, cx| root.apply_render_settings(settings, cx));
    }
}

impl Render for SettingsWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .size_full()
            .bg(cx.theme().background)
            // —— 顶部：自绘标题栏（配色跟主题走；拖拽 / 窗口按钮由 `TitleBar` 处理）——
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
                // `Settings` 是左右分栏，需要父级给出确定高度；不加内边距（要铺满窗口）。
                div()
                    .flex_1()
                    .min_h_0()
                    .child(
                        Settings::new("app-settings")
                            .page(self.theme_page(cx))
                            .page(self.terminal_page()),
                    ),
            )
            // gpui-kit 的 `Root` 不会自动渲染 Dialog 层，需在渲染树里显式挂载。
            .children(Root::render_dialog_layer(window, cx))
    }
}

impl AppRoot {
    /// 打开设置窗口；已打开则激活它。`window` 是主窗口（设置窗口靠它的句柄跟随退出）。
    pub(crate) fn open_settings_window(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // 已打开过就激活原窗口；`update` 返回 Err 说明窗口已关闭，继续往下新建。
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
            // ⚠️ 别改回 `WindowKind::Dialog`（那会锁住主窗口、且随 owner 关闭）。
            kind: WindowKind::Normal,
            // 隐藏系统标题栏，改由视图内的自绘 `TitleBar` 负责。
            ..TitleBar::window_options()
        };
        // 独立窗口会在任务栏 / Alt-Tab 里露出条目，得有个名字。
        if let Some(titlebar) = options.titlebar.as_mut() {
            titlebar.title = Some("设置".into());
        }

        // 外层包一层 gpui-kit `Root`（弹窗 / 通知 / 焦点恢复的宿主）。
        let app_root = cx.entity().downgrade();
        match cx.open_window(options, |window, cx| {
            let view = cx.new(|cx| SettingsWindow::new(main_window, app_root, cx));
            cx.new(|cx| Root::new(view, window, cx))
        }) {
            Ok(handle) => self.settings_window = Some(handle),
            Err(err) => log::error!("failed to open settings window: {err:#}"),
        }
    }
}

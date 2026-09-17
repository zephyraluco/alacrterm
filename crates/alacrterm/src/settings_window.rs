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
/// `Settings` 把搜索框等状态存在窗口的 keyed state 里，所以视图几乎没有自有状态；
/// 除了「主窗口已关闭」的观察句柄，只多一个主窗口根视图的弱引用：
/// **主题是全局状态**（`Theme::global`，改完 `refresh_windows` 两个窗口一起重绘），
/// 但**终端渲染参数存在每个 `TerminalView` 里**，要即时生效只能借
/// `AppRoot::apply_render_settings` 去更新它们。
struct SettingsWindow {
    app_root: WeakEntity<AppRoot>,
    _main_window_closed: Subscription,
}

impl SettingsWindow {
    /// `main_window` = 主窗口句柄（由 [`AppRoot::open_settings_window`] 传入）。
    ///
    /// 设置窗口是独立顶层窗口，系统不会因 owner 消失而连带关闭它，所以这里自己
    /// 盯住主窗口：主窗口一关就 `App::quit()`，让主程序连同设置窗口一起退出。
    /// 这比反过来“在主窗口关闭时去关设置窗口”简单：不用跨窗口取句柄。
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

    /// 「终端」页：字体与光标相关的渲染参数。
    ///
    /// 只列「界面能表达」的项；27 个 ANSI 颜色与 3 个 accent 色仍由
    /// `config/terminal.json` 手写——本页保存时**不会**动它们（见
    /// `config::save_render_settings`）。
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
/// ⚠️ 不用 `SettingField::switch`：它的把手是弹簧动画，而 gpui-pre-windows 在某个窗口
/// 连续重绘期间会**丢掉**另一个窗口的绘制请求 ⇒ 动画的那 250ms 里主窗口完全不重绘
/// （观感就是「主界面慢半拍」）。这里把 element id 绑上当前模式：换模式 = 换一把新 id
/// = 弹簧状态新建即已就位，因此没有动画、两个窗口同一帧一起变色。外观与其它开关完全
/// 一致（同一个 `Switch` 组件）。机制与实测见 `docs/terminal-architecture.md`。
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
                // 统一入口：应用主题并重新压下分栏拖拽条线条的透明覆盖
                // （见 `crate::change_theme`）；`None` 窗口参数不会自动刷新，
                // 需手动刷新所有窗口（含本设置窗口）。
                crate::change_theme(mode, cx);
                cx.refresh_windows();
            })
            .into_any_element()
    })
}

/// 主题下拉框：值 = 注册表里的主题名，空串 = 跟随 gpui-kit 内置主题。
///
/// 候选项每帧重算（`theme_options` 读主题库），所以 `themes/` 里增删文件后
/// 打开本页就能看到最新的列表。
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

/// 光标形状下拉框的候选项：值用的就是配置文件里的写法（与 `config` 里的
/// `cursor_shape_name` / `parse_cursor_shape` 对偶）。
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

/// 改一项终端渲染参数：更新全局 → 落盘 → 推给存活会话（三步缺一不可）。
///
/// 主题是全局状态，改完 `refresh_windows` 两个窗口就都对了；终端参数却存在每个
/// `TerminalView` 里，所以要借主窗口根视图（[`AppRoot::apply_render_settings`]）转发。
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
                // 不加内边距：导航栏与内容区各自有自己的留白，外面再包一圈会让整个
                // 组件与窗口边缘隔出一道缝（要的是铺满）。
                div()
                    .flex_1()
                    .min_h_0()
                    .child(
                        Settings::new("app-settings")
                            .page(self.theme_page(cx))
                            .page(self.terminal_page()),
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

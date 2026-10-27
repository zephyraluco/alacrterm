//! 独立窗口形式的「设置」界面：两页（主题 / 终端），入口 [`AppRoot::open_settings_window`]。

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

/// 初始尺寸（宽, 高）。
const WINDOW_SIZE: (f32, f32) = (760., 520.);
/// 最小尺寸（宽, 高）。
const MIN_WINDOW_SIZE: (f32, f32) = (560., 400.);

struct SettingsWindow {
    app_root: WeakEntity<AppRoot>,
    _main_window_closed: Subscription,
}

impl SettingsWindow {
    /// 主窗口关闭即退出。
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

    /// 「主题」页。
    fn theme_page(&self, cx: &App) -> SettingPage {
        SettingPage::new("主题")
            .icon(Icon::new(IconName::Moon))
            .default_open(true)
            .group(
                SettingGroup::new()
                    .item(
                        SettingItem::new("深色主题", theme_mode_switch())
                            .description("切换深色 / 浅色界面主题；终端配色在「终端」页单独选择。"),
                    )
                    .item(
                        SettingItem::new("深色模式配色", theme_dropdown(cx, ThemeMode::Dark))
                            .description("深色模式下用哪套主题；默认「跟随 gpui-kit」用内置配色。"),
                    )
                    .item(
                        SettingItem::new("浅色模式配色", theme_dropdown(cx, ThemeMode::Light))
                            .description("同上，作用于浅色模式。主题文件在 themes/global/ 下，改完即刻热重载。"),
                    ),
            )
    }

    /// 「终端」页。
    fn terminal_page(&self, cx: &App) -> SettingPage {
        SettingPage::new("终端")
            .icon(Icon::new(IconName::SquareTerminal))
            .default_open(true)
            .group(
                SettingGroup::new()
                    .item(render_item(
                        "终端配色",
                        "立即应用到所有终端并保存。默认／自定义使用 terminal.json 中的颜色。",
                        terminal_color_dropdown(self.app_root.clone(), cx),
                    ))
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
            ).group(suggestion_group(self.app_root.clone()))
    }
}

fn suggestion_group(root: WeakEntity<AppRoot>) -> SettingGroup {
    type Read = fn(&terminal_view::SuggestionSettings) -> bool;
    type Write = fn(&mut terminal_view::SuggestionSettings, bool);
    let fields: &[(&str, &str, Read, Write)] = &[
        ("命令建议", "显示终端补全候选列表。", |s| s.enabled, |s,v| s.enabled=v),
        ("自动显示建议", "关闭后使用 Ctrl+Space 手动查询。", |s| s.automatic, |s,v| s.automatic=v),
        ("历史候选", "仅保存当前分屏内存中的历史；关闭时清除。", |s| s.history, |s,v| s.history=v),
        ("Shell 命令候选", "使用当前 shell 的命令、别名和函数。", |s| s.commands, |s,v| s.commands=v),
        ("路径候选", "补全当前会话的文件和目录。", |s| s.paths, |s,v| s.paths=v),
        ("子命令和选项", "显示内置命令规格与说明。", |s| s.specifications, |s,v| s.specifications=v),
        ("项目参数", "从当前项目提供脚本和分支等候选。", |s| s.dynamic, |s,v| s.dynamic=v),
        ("模糊匹配", "命令名允许子序列匹配，前缀匹配优先。", |s| s.fuzzy, |s,v| s.fuzzy=v),
        ("接受后补空格", "文件和命令接受后追加空格；目录和历史除外。", |s| s.insert_trailing_space, |s,v| s.insert_trailing_space=v),
        ("SSH 自动集成", "为 Bash/Zsh 安装或复用启动入口，仅对 Alacrterm 会话启用。", |s| s.remote_integration, |s,v| s.remote_integration=v),
    ];
    let mut group = SettingGroup::new();
    for &(title, description, read, write) in fields {
        let root = root.clone();
        group = group.item(render_item(title, description, SettingField::switch(
            move |cx: &App| read(&config::settings(cx).render.suggestions),
            move |value, cx: &mut App| update_render(&root, cx, |render| write(&mut render.suggestions, value)),
        )));
    }
    group
}

/// 「深色主题」开关。
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
                config::change_theme(mode, cx);
                cx.refresh_windows();
            })
            .into_any_element()
    })
}

/// 主题下拉框：空串 = 跟随内置主题。
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

fn terminal_color_dropdown(root: WeakEntity<AppRoot>, cx: &App) -> SettingField<SharedString> {
    let mut options = vec![(SharedString::default(), "默认／自定义".into())];
    let mut entries: Vec<_> = config::ColorSchemeRegistry::global(cx).entries().iter()
        .map(|(id, scheme)| (SharedString::from(id.clone()), SharedString::from(scheme.name.clone())))
        .collect();
    entries.sort_by(|a, b| a.1.to_lowercase().cmp(&b.1.to_lowercase()).then(a.0.cmp(&b.0)));
    options.extend(entries);
    SettingField::scrollable_dropdown(
        options,
        |cx: &App| config::settings(cx).color_scheme.clone().unwrap_or_default(),
        move |value: SharedString, cx: &mut App| {
            let id = (!value.is_empty()).then_some(value);
            let palette = config::palette_settings(id.as_deref(), cx);
            config::select_color_scheme(id, cx);
            update_render(&root, cx, |render| {
                render.colors = palette.colors;
                render.selection_color = palette.selection_color;
                render.search_match_color = palette.search_match_color;
                render.link_color = palette.link_color;
            });
            cx.refresh_windows();
        },
    )
}

/// 渲染参数项：包一层 description。
fn render_item(
    title: &'static str,
    description: &'static str,
    field: impl AnySettingField + 'static,
) -> SettingItem {
    SettingItem::new(title, field).description(description)
}

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

/// 改一项渲染参数：更新全局 → 落盘 → 推给会话。
///
/// 设置窗口与字号快捷键（`alacrterm::IncreaseFontSize` 等）共用这条路径。
pub(crate) fn update_render(
    root: &WeakEntity<AppRoot>,
    cx: &mut App,
    change: impl FnOnce(&mut RenderSettings),
) {
    let mut settings = config::settings(cx).render.clone();
    change(&mut settings);
    if let Err(error) = config::save_render_settings(&settings, config::settings(cx).color_scheme.as_deref()) {
        eprintln!("[config] 保存终端设置失败，保持原设置：{error:#}");
        return;
    }
    config::settings_mut(cx).render = settings.clone();
    if let Some(root) = root.upgrade() {
        root.update(cx, |root, cx| root.apply_render_settings(settings, cx));
    }
}

impl Render for SettingsWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .size_full()
            .bg(cx.theme().background)
            // 顶部：自绘标题栏。
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
                div()
                    .flex_1()
                    .min_h_0()
                    .child(
                        Settings::new("app-settings")
                            .page(self.theme_page(cx))
                            .page(self.terminal_page(cx)),
                    ),
            )
            // 覆盖层由 gpui-kit `Root` 渲染。
    }
}

impl AppRoot {
    /// 打开或激活设置窗口。
    pub(crate) fn open_settings_window(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
            // 独立顶层窗口、不模态。
            kind: WindowKind::Normal,
            ..TitleBar::window_options()
        };
        // 任务栏 / Alt-Tab 里的窗口名。
        if let Some(titlebar) = options.titlebar.as_mut() {
            titlebar.title = Some("设置".into());
        }

        // 外层包一层 gpui-kit `Root`。
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

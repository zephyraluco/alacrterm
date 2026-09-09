//! 基于 gpui-kit (gpui-component 0.6) 外壳 + alacrterm 终端核心的终端应用。
//!
//! 架构（方案 A，移植自 https://github.com/zephyraluco/alacrterm）：
//! - `crates/terminal`      —— 终端仿真核心（alacritty_terminal 0.26 + PTY + 事件循环）
//! - `crates/terminal_view` —— 终端视图（自定义 gpui Element 逐 cell 渲染）
//! - `crates/util`          —— Shell 探测 / 路径工具（来自 Zed）
//! - 本文件                 —— gpui-kit 应用外壳：
//!   自绘标题栏（TitleBar）+ 活动栏（直到底部）+ 侧边栏（Sidebar，多终端会话切换）+ 终端，
//!   底部为两段式状态栏（左段属于侧边栏、右段属于终端），
//!   活动栏底部固定设置图标，点击弹出设置对话框（Dialog）。

mod assets;

use assets::IconName;
use gpui::{
    App, AppContext as _, Bounds, Context, Entity, InteractiveElement, IntoElement,
    ParentElement as _, Render, ScrollHandle, ScrollWheelEvent, SharedString, Styled as _, Window,
    WindowBounds, WindowOptions, div, point, px, size,
};
use gpui_kit::{
    QuitMode,
    component::{
        ActiveTheme as _, Icon, Root, Selectable as _, Sizable as _, Theme, ThemeMode, TitleBar,
        WindowExt as _,
        button::{Button, ButtonVariants as _},
        h_flex,
        setting::{SettingField, SettingGroup, SettingItem, SettingPage, Settings},
        status_bar::StatusBar, tab::{Tab, TabBar}, v_flex,
        sidebar::{Sidebar, SidebarGroup, SidebarMenu, SidebarMenuItem, SidebarFooter},
    },
    prelude::FluentBuilder as _,
};
use terminal_view::TerminalView;
use util::shell::Shell;

fn main() {
    gpui_kit::application()
        // 注册自有资产源（alacrterm assets.rs 方式）：本 crate 的 assets/icons 目录
        // 经 rust-embed 嵌入，`crate::assets::IconName` 由 icon_named! 宏扫描生成，
        // 可自由增删图标文件。
        .with_assets(assets::Assets)
        .with_quit_mode(QuitMode::LastWindowClosed)
        .run(|cx: &mut App| {
            gpui_kit::init(cx);
            // 终端为深色背景，应用主题跟随使用暗色。
            Theme::change(ThemeMode::Dark, None, cx);

            let bounds = Bounds::centered(None, size(px(1100.), px(700.)), cx);
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    // TitleBar::window_options()：隐藏系统标题栏（appears_transparent）,
                    // 由 gpui-kit TitleBar 自行处理拖拽 / 双击最大化 / 窗口控制按钮。
                    ..TitleBar::window_options()
                },
                |window, cx| {
                    let root = cx.new(|cx| AppRoot::new(window, cx));
                    cx.new(|cx| Root::new(root, window, cx))
                },
            )
            .expect("failed to open window");
        });
}

/// 侧边栏视图（对应活动栏图标）。
#[derive(Clone, Copy, PartialEq, Eq)]
enum SidebarView {
    /// 终端会话列表。
    Sessions,
    /// 关于。
    About,
}

/// 应用根视图：gpui-kit 自绘标题栏 + 活动栏 + 侧边栏 + 终端 + 底部状态栏。
struct AppRoot {
    /// 所有终端会话（保持运行，切换仅切换显示）。
    terminals: Vec<Entity<TerminalView>>,
    /// 当前显示的终端下标。
    active: usize,
    /// 侧边栏是否可见（点击活动栏当前视图图标可隐藏/显示）。
    sidebar_visible: bool,
    /// 侧边栏当前视图（由活动栏图标切换）。
    sidebar_view: SidebarView,
    /// 状态栏左侧展示的 shell 程序名（如 pwsh.exe）。
    shell_name: SharedString,
    /// 标签栏滚动句柄：跟踪 tabs 横向滚动，键盘切换时把选中标签滚入可视区。
    tab_scroll_handle: ScrollHandle,
}

impl AppRoot {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let shell_program = Shell::System.program();
        let shell_name = std::path::Path::new(&shell_program)
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or(shell_program);

        let mut this = Self {
            terminals: Vec::new(),
            active: 0,
            sidebar_visible: true,
            sidebar_view: SidebarView::Sessions,
            shell_name: shell_name.into(),
            tab_scroll_handle: ScrollHandle::new(),
        };
        this.spawn_terminal(window, cx);
        this
    }

    /// 活动栏图标点击：切换视图；再次点击当前视图图标则隐藏/显示侧边栏。
    fn set_sidebar_view(&mut self, view: SidebarView, cx: &mut Context<Self>) {
        if self.sidebar_view == view {
            self.sidebar_visible = !self.sidebar_visible;
        } else {
            self.sidebar_view = view;
            self.sidebar_visible = true;
        }
        cx.notify();
    }

    /// 活动栏底部设置图标点击：弹出设置对话框（不改变侧边栏的显隐状态）。
    ///
    /// 设置界面参考官方组件：https://gpui-kit.com/zh-CN/component/settings/
    fn open_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        window.open_dialog(cx, |dialog, _, _| {
            dialog
                .title("设置")
                .w(px(720.))
                .child(
                    // Settings 渲染为 h_resizable，需要外部给定高度（参考官方 story 的 h(420)）。
                    div().h(px(440.)).child(
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
        });
    }

    /// 新建一个终端会话（PTY 在后台启动），并订阅其事件用于刷新界面。
    fn spawn_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let view = cx.new(|cx| TerminalView::new(None, Shell::System, window, cx));
        cx.observe(&view, |_, _, cx| cx.notify()).detach();
        self.terminals.push(view);
        self.set_active_tab(self.terminals.len() - 1, cx);
    }

    /// 激活指定的终端会话标签，并把标签栏滑动到该标签。
    ///
    /// `ScrollHandle::scroll_to_item` 在下一帧 prepaint 时生效：仅做最小滚动，
    /// 让选中的标签进入可视范围（标签已在视野内时不动）。
    /// 所有激活路径（标签点击 / 侧边栏会话项 / 新建会话）都应经由本方法。
    fn set_active_tab(&mut self, index: usize, cx: &mut Context<Self>) {
        self.active = index.min(self.terminals.len().saturating_sub(1));
        self.tab_scroll_handle.scroll_to_item(self.active);
        cx.notify();
    }

    /// 关闭一个终端会话（标签页 × 按钮，参考官方 Dynamic Tabs / Closeable Tabs 示例）。
    /// 至少保留一个会话；实体移除后 `Terminal` 的 Drop 会关闭 PTY 并终止子进程。
    fn close_terminal(&mut self, index: usize, cx: &mut Context<Self>) {
        if self.terminals.len() <= 1 {
            return;
        }
        self.terminals.remove(index);
        // 选中下标调整逻辑与官方 close_tab 示例一致。
        if self.active >= index && self.active > 0 {
            self.active -= 1;
        }
        if self.active >= self.terminals.len() {
            self.active = self.terminals.len() - 1;
        }
        self.tab_scroll_handle.scroll_to_item(self.active);
        cx.notify();
    }
}

impl Render for AppRoot {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // 侧边栏菜单项：每个终端会话一项，点击切换。
        let this = cx.entity().downgrade();
        let items = self.terminals.iter().enumerate().map(|(ix, terminal)| {
            let this = this.clone();
            SidebarMenuItem::new(terminal.read(cx).title())
                .icon(IconName::SquareTerminal)
                .active(ix == self.active)
                .on_click(move |_, _, cx| {
                    let _ = this.update(cx, |this, cx| this.set_active_tab(ix, cx));
                })
        });

        // 侧边栏内容：随活动栏选中的视图切换。
        let sidebar_content = match self.sidebar_view {
            SidebarView::Sessions => {
                SidebarGroup::new("会话").child(SidebarMenu::new().children(items))
            }
            SidebarView::About => SidebarGroup::new("关于").child(
                SidebarMenu::new()
                    .child(SidebarMenuItem::new("test-rs 终端").disable(true))
                    .child(SidebarMenuItem::new("gpui-kit 0.6 · gpui-pre 0.3").disable(true))
                    .child(
                        SidebarMenuItem::new("alacritty_terminal 0.26 · ConPTY").disable(true),
                    ),
            ),
        };

        // 活动栏：侧边栏左侧的图标列。上部分为视图切换图标（终端会话 / 关于），
        // 弹性占位后，设置图标固定在最下方，点击弹出设置对话框。
        let activity_bar = v_flex()
            .w(px(44.))
            .h_full()
            .flex_shrink_0()
            .items_center()
            .py_2()
            .gap_1()
            .bg(cx.theme().tokens.sidebar)
            .border_r_1()
            .border_color(cx.theme().sidebar_border)
            .child(
                Button::new("view-sessions")
                    .ghost()
                    .icon(IconName::SquareTerminal)
                    .selected(self.sidebar_visible && self.sidebar_view == SidebarView::Sessions)
                    .tooltip("终端会话")
                    .on_click(
                        cx.listener(|this, _, _, cx| this.set_sidebar_view(SidebarView::Sessions, cx)),
                    ),
            )
            .child(
                Button::new("view-about")
                    .ghost()
                    .icon(IconName::Info)
                    .selected(self.sidebar_visible && self.sidebar_view == SidebarView::About)
                    .tooltip("关于")
                    .on_click(
                        cx.listener(|this, _, _, cx| this.set_sidebar_view(SidebarView::About, cx)),
                    ),
            )
            // 弹性占位：把设置图标推到活动栏最下方。
            .child(div().flex_1())
            .child(
                Button::new("view-settings")
                    .ghost()
                    .icon(IconName::Settings)
                    .tooltip("设置")
                    .on_click(cx.listener(|this, _, window, cx| this.open_settings(window, cx))),
            );

        // 侧边栏（结构参考官方文档：https://gpui-kit.com/zh-CN/component/sidebar/）
        // 点击活动栏当前视图图标可整体隐藏/显示。
        let sidebar = Sidebar::new("terminal-sidebar")
            .w(px(220.))
            // .header(
            //     SidebarHeader::new().child(
            //         h_flex()
            //             .gap_2()
            //             .child(Icon::new(IconName::SquareTerminal))
            //             .child("终端"),
            //     ),
            // )
            .child(sidebar_content)
            .footer(
                SidebarFooter::new().child(
                    Button::new("new-terminal")
                        .ghost()
                        .icon(IconName::Plus)
                        .label("新建终端")
                        .tooltip("新建终端")
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.spawn_terminal(window, cx);
                            cx.notify();
                        })),
                ),
            );

        // 两段式状态栏：左段属于侧边栏（与侧边栏同宽对齐），右段属于终端。
        // 两段使用同一状态栏配色（此前左段用 sidebar 背景色，明显深于终端段），
        // 仅以左段右侧的竖线区分；侧边栏隐藏时左段随之消失，终端段占满整行。
        let sidebar_status = StatusBar::new()
            .left(
                h_flex()
                    .items_center()
                    .gap_1()
                    .child(Icon::new(IconName::SquareTerminal).small())
                    .child(self.shell_name.clone()),
            )
            .w(px(220.))
            .flex_shrink_0()
            .border_r_1();

        let terminal_status = StatusBar::new()
            .child(self.terminals[self.active].read(cx).title())
            .right("ConPTY · alacritty_terminal 0.26")
            .flex_1()
            .min_w_0();

        // 终端会话标签页（参考官方 Tabs 示例的「Dynamic Tabs」）：
        // 每个标签由图标前缀 + 标题 + × 关闭后缀组成（prefix / suffix content），
        // 新建会话入口在侧边栏 footer。
        // w_full：与官方示例一致，显式占满父宽——否则标签栏按内容收缩，
        // 标签一多会撑出可视区并把右侧菜单键推走；约束住后内部自动
        // 横向裁剪/滚动（overflow_x_scroll），菜单键固定在右端兜底选择。
        let tab_bar = TabBar::new("terminal-tabs")
            .w_full()
            .menu(true)
            // 关联滚动句柄：激活会话时 scroll_to_item 精确滑动到选中标签。
            .track_scroll(&self.tab_scroll_handle)
            .selected_index(self.active)
            .on_click(cx.listener(|this, index: &usize, _, cx| {
                // 点击标签内 × 关闭会话后，事件仍会冒泡到此处且下标可能已失效，需钳制。
                this.set_active_tab(*index, cx);
            }))
            .children(
                self.terminals.iter().enumerate().map(|(ix, terminal)| {
                    Tab::new()
                        .px_2()
                        .prefix(Icon::new(IconName::SquareTerminal))
                        .label(terminal.read(cx).title())
                        .suffix(
                            Button::new(format!("close-tab-{ix}"))
                                .ghost()
                                .xsmall()
                                .icon(IconName::Close)
                                .tooltip("关闭会话")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.close_terminal(ix, cx);
                                })),
                        )
                }),
            );
        // 滚轮滚动标签栏：TabBar 内部 lock_scroll_axis 禁用了「垂直滚轮→横向」的
        // 自动映射，因此在外层把滚轮增量手动写入标签栏的 ScrollHandle（横向偏移）；
        // 触摸板横向滚动仍由标签栏内部处理，不会重复。
        let tab_scroll_handle = self.tab_scroll_handle.clone();
        let tab_bar_area = div()
            .id("tab-bar-area")
            .w_full()
            .on_scroll_wheel(move |event: &ScrollWheelEvent, window, _cx| {
                let dy = event.delta.pixel_delta(window.line_height()).y;
                if dy == px(0.) {
                    return;
                }
                let max = tab_scroll_handle.max_offset().x;
                // 滚轮向下 = 查看右侧标签（offset 向负方向增长）。
                let next = (tab_scroll_handle.offset().x - dy).clamp(-max, px(0.));
                tab_scroll_handle.set_offset(point(next, px(0.)));
                window.refresh();
            })
            .child(tab_bar);

        v_flex()
            .id("app-root")
            .size_full()
            .bg(cx.theme().background)
            // —— 顶部：自绘标题栏（图标 / 标题 / 窗口控制）——
            .child(
                TitleBar::new().child(
                    h_flex()
                        .w_full()
                        .px(px(8.))
                        .gap(px(8.))
                        .items_center()
                        // .child(Icon::new(IconName::SquareTerminal).small())
                        .child(
                            div()
                                .flex_1()
                                .overflow_hidden()
                                .text_size(px(13.))
                                .text_color(cx.theme().secondary_foreground)
                                .child("Alacrterm"),
                        ),
                ),
            )
            // —— 中部：活动栏（h_full 直到底部，不设状态栏）+ 右侧区域 ——
            // 右侧区域为「侧边栏 + 终端」行与其下方的两段式状态栏：
            // 左段状态栏与侧边栏同宽对齐（属于侧边栏），右段属于终端。
            .child(
                h_flex()
                    .flex_1()
                    .overflow_hidden()
                    .child(activity_bar)
                    .child(
                        // h_flex 默认交叉轴居中，满高列必须显式 h_full，
                        // 否则该列只取内容高度并垂直居中，布局整体塌陷。
                        v_flex()
                            .h_full()
                            .flex_1()
                            .overflow_hidden()
                            // 侧边栏（可隐藏）+ 终端卡片
                            .child(
                                h_flex()
                                    .flex_1()
                                    .overflow_hidden()
                                    .when(self.sidebar_visible, |row| row.child(sidebar))
                                    .child(
                                        // 终端 pane：上方标签页 + 下方终端卡片
                                        // overflow_hidden：pane 无 overflow 时，taffy 的自动
                                        // 最小尺寸 = 内容宽（含所有标签的总宽），标签一多 pane
                                        // 会被撑出可视区并把 TabBar(w_full) 与菜单键一起推走；
                                        // 设为 hidden 后最小尺寸归零，宽度完全由行分配。
                                        v_flex()
                                            .flex_1()
                                            .h_full()
                                            .overflow_hidden()
                                            .p_2()
                                            .gap_2()
                                            .child(tab_bar_area)
                                            .child(
                                                div()
                                                    .flex_1()
                                                    .border_1()
                                                    .border_color(cx.theme().border)
                                                    .rounded_md()
                                                    .overflow_hidden()
                                                    .child(self.terminals[self.active].clone()),
                                            ),
                                    ),
                            )
                            // —— 底部：两段式状态栏 ——
                            .child(
                                h_flex()
                                    .w_full()
                                    .when(self.sidebar_visible, |row| row.child(sidebar_status))
                                    .child(terminal_status),
                            ),
                    ),
            )
            // —— 覆盖层：gpui-kit 0.6 的 Root 不会自动渲染 Dialog 层，
            //    必须在渲染树中显式挂载（参考官方 ai_recipes 示例），否则弹窗不显示。——
            .children(Root::render_dialog_layer(window, cx))
    }
}

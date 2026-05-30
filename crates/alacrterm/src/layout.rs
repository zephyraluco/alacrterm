use crate::terminal_view::TerminalView;
use gpui::*;
use gpui_component::{
    ActiveTheme, Icon, IconName, Selectable, Sizable, TitleBar, h_flex, v_flex,
    button::{Button, ButtonVariants},
    resizable::{h_resizable, resizable_panel},
    tab::{Tab, TabBar},
};

/// 活动栏视图类型（对应左侧边栏不同功能）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivityView {
    Terminal,
    Sessions,
    Tools,
    Settings,
}

pub struct TerminalApp {
    pub terminals: Vec<Entity<TerminalView>>,
    pub active_tab: usize,
    pub active_activity: ActivityView,
    pub left_sidebar_visible: bool,
    pub right_sidebar_visible: bool,
}

impl TerminalApp {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let terminal_view = cx.new(|cx| TerminalView::new(cx));
        Self {
            terminals: vec![terminal_view],
            active_tab: 0,
            active_activity: ActivityView::Terminal,
            left_sidebar_visible: true,
            right_sidebar_visible: false,
        }
    }
}

impl Render for TerminalApp {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let active_tab = self.active_tab;
        let active_activity = self.active_activity;
        let left_sidebar_visible = self.left_sidebar_visible;
        let right_sidebar_visible = self.right_sidebar_visible;
        let tab_count = self.terminals.len();
        let terminal = self.terminals[active_tab].clone();

        v_flex()
            .size_full()
            // ── 标题栏（菜单栏）──────────────────────────────────────────────
            .child(
                TitleBar::new().child(
                    h_flex()
                        .gap_1()
                        .child(Button::new("menu-terminal").ghost().label("终端").small())
                        .child(Button::new("menu-session").ghost().label("会话").small())
                        .child(Button::new("menu-tools").ghost().label("工具").small())
                        .child(Button::new("menu-settings").ghost().label("设置").small()),
                ),
            )
            // ── 主区域（活动栏 + 三列可缩放区域）───────────────────────────
            .child(
                h_flex()
                    .flex_grow()
                    .min_h_0()
                    .overflow_hidden()
                    // ── 活动栏（固定 48px，竖向图标条）──────────────────────
                    .child(
                        v_flex()
                            .w(px(48.0))
                            .h_full()
                            .flex_shrink_0()
                            .border_r_1()
                            .border_color(cx.theme().sidebar_border)
                            .bg(cx.theme().sidebar)
                            .pt_2()
                            .pb_2()
                            .gap_1()
                            .items_center()
                            // 终端
                            .child(
                                Button::new("act-terminal")
                                    .ghost()
                                    .icon(Icon::new(IconName::SquareTerminal))
                                    .tooltip("终端")
                                    .selected(active_activity == ActivityView::Terminal)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        if this.active_activity == ActivityView::Terminal {
                                            this.left_sidebar_visible =
                                                !this.left_sidebar_visible;
                                        } else {
                                            this.active_activity = ActivityView::Terminal;
                                            this.left_sidebar_visible = true;
                                        }
                                        cx.notify();
                                    })),
                            )
                            // 会话
                            .child(
                                Button::new("act-sessions")
                                    .ghost()
                                    .icon(Icon::new(IconName::BookOpen))
                                    .tooltip("会话")
                                    .selected(active_activity == ActivityView::Sessions)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        if this.active_activity == ActivityView::Sessions {
                                            this.left_sidebar_visible =
                                                !this.left_sidebar_visible;
                                        } else {
                                            this.active_activity = ActivityView::Sessions;
                                            this.left_sidebar_visible = true;
                                        }
                                        cx.notify();
                                    })),
                            )
                            // 工具
                            .child(
                                Button::new("act-tools")
                                    .ghost()
                                    .icon(Icon::new(IconName::Settings2))
                                    .tooltip("工具")
                                    .selected(active_activity == ActivityView::Tools)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        if this.active_activity == ActivityView::Tools {
                                            this.left_sidebar_visible =
                                                !this.left_sidebar_visible;
                                        } else {
                                            this.active_activity = ActivityView::Tools;
                                            this.left_sidebar_visible = true;
                                        }
                                        cx.notify();
                                    })),
                            )
                            // 弹性间隔 → 设置推到底部
                            .child(div().flex_grow())
                            // 设置
                            .child(
                                Button::new("act-settings")
                                    .ghost()
                                    .icon(Icon::new(IconName::Settings))
                                    .tooltip("设置")
                                    .selected(active_activity == ActivityView::Settings)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        if this.active_activity == ActivityView::Settings {
                                            this.left_sidebar_visible =
                                                !this.left_sidebar_visible;
                                        } else {
                                            this.active_activity = ActivityView::Settings;
                                            this.left_sidebar_visible = true;
                                        }
                                        cx.notify();
                                    })),
                            ),
                    )
                    // ── 三列可拖拽缩放区域 ────────────────────────────────────
                    .child(
                        // 包裹 div：占满活动栏右侧剩余空间，给 h_resizable 提供 size 上下文
                        div()
                            .flex_grow()
                            .min_w_0()
                            .h_full()
                            .overflow_hidden()
                            .child(
                                h_resizable("main-panels")
                                    // ── 左侧边栏面板（可拖拽调宽，可隐藏）────
                                    .child(
                                        resizable_panel()
                                            .size(px(200.0))
                                            .size_range(px(100.0)..px(500.0))
                                            .flex_none()
                                            .visible(left_sidebar_visible)
                                            .child(
                                                v_flex()
                                                    .size_full()
                                                    .border_r_1()
                                                    .border_color(
                                                        cx.theme().sidebar_border,
                                                    )
                                                    .bg(cx.theme().sidebar)
                                                    // 标题行
                                                    .child(
                                                        h_flex()
                                                            .w_full()
                                                            .px_3()
                                                            .h(px(36.0))
                                                            .flex_shrink_0()
                                                            .items_center()
                                                            .border_b_1()
                                                            .border_color(
                                                                cx.theme().sidebar_border,
                                                            )
                                                            .child(
                                                                div()
                                                                    .text_sm()
                                                                    .font_weight(
                                                                        FontWeight::SEMIBOLD,
                                                                    )
                                                                    .text_color(
                                                                        cx.theme()
                                                                            .sidebar_foreground,
                                                                    )
                                                                    .child(
                                                                        match active_activity {
                                                                            ActivityView::Terminal => "终端",
                                                                            ActivityView::Sessions => "会话",
                                                                            ActivityView::Tools => "工具",
                                                                            ActivityView::Settings => "设置",
                                                                        },
                                                                    ),
                                                            ),
                                                    )
                                                    // 内容区
                                                    .child(
                                                        div()
                                                            .flex_grow()
                                                            .p_2()
                                                            .text_sm()
                                                            .text_color(
                                                                cx.theme().muted_foreground,
                                                            )
                                                            .child(match active_activity {
                                                                ActivityView::Terminal => {
                                                                    "终端会话列表"
                                                                }
                                                                ActivityView::Sessions => {
                                                                    "历史会话"
                                                                }
                                                                ActivityView::Tools => "工具列表",
                                                                ActivityView::Settings => "设置项",
                                                            }),
                                                    ),
                                            ),
                                    )
                                    // ── 中间终端面板（自动伸缩，含标签栏）────
                                    .child(
                                        resizable_panel()
                                            .size_range(px(200.0)..Pixels::MAX)
                                            .child(
                                                v_flex()
                                                    .size_full()
                                                    .overflow_hidden()
                                                    // 标签页栏
                                                    .child(
                                                        h_flex()
                                                            .w_full()
                                                            .h(px(36.0))
                                                            .flex_shrink_0()
                                                            .border_b_1()
                                                            .border_color(cx.theme().border)
                                                            .bg(cx.theme().tab_bar)
                                                            .items_center()
                                                            // 左侧边栏切换
                                                            .child(
                                                                Button::new(
                                                                    "toggle-left-sidebar",
                                                                )
                                                                .ghost()
                                                                .icon(Icon::new(
                                                                    if left_sidebar_visible {
                                                                        IconName::PanelLeftClose
                                                                    } else {
                                                                        IconName::PanelLeftOpen
                                                                    },
                                                                ))
                                                                .small()
                                                                .tooltip(
                                                                    if left_sidebar_visible {
                                                                        "隐藏左侧边栏"
                                                                    } else {
                                                                        "显示左侧边栏"
                                                                    },
                                                                )
                                                                .on_click(cx.listener(
                                                                    |this, _, _, cx| {
                                                                        this.left_sidebar_visible =
                                                                            !this
                                                                                .left_sidebar_visible;
                                                                        cx.notify();
                                                                    },
                                                                )),
                                                            )
                                                            // 终端标签列表
                                                            .child(
                                                                TabBar::new("terminal-tabs")
                                                                    .selected_index(active_tab)
                                                                    .children(
                                                                        (0..tab_count).map(|i| {
                                                                            Tab::new()
                                                                                .label(format!(
                                                                                    "终端 {}",
                                                                                    i + 1
                                                                                ))
                                                                                .selected(
                                                                                    i == active_tab,
                                                                                )
                                                                                .on_click(
                                                                                    cx.listener(
                                                                                        move |this,
                                                                                              _,
                                                                                              _,
                                                                                              cx| {
                                                                                            this.active_tab = i;
                                                                                            cx.notify();
                                                                                        },
                                                                                    ),
                                                                                )
                                                                        }),
                                                                    ),
                                                            )
                                                            // 弹性间隔
                                                            .child(div().flex_grow())
                                                            // 右侧边栏切换
                                                            .child(
                                                                Button::new(
                                                                    "toggle-right-sidebar",
                                                                )
                                                                .ghost()
                                                                .icon(Icon::new(
                                                                    if right_sidebar_visible {
                                                                        IconName::PanelRightClose
                                                                    } else {
                                                                        IconName::PanelRightOpen
                                                                    },
                                                                ))
                                                                .small()
                                                                .tooltip(
                                                                    if right_sidebar_visible {
                                                                        "隐藏右侧面板"
                                                                    } else {
                                                                        "显示右侧面板"
                                                                    },
                                                                )
                                                                .on_click(cx.listener(
                                                                    |this, _, _, cx| {
                                                                        this.right_sidebar_visible =
                                                                            !this
                                                                                .right_sidebar_visible;
                                                                        cx.notify();
                                                                    },
                                                                )),
                                                            ),
                                                    )
                                                    // 终端显示区
                                                    .child(
                                                        div()
                                                            .flex_grow()
                                                            .min_h_0()
                                                            .overflow_hidden()
                                                            .child(terminal),
                                                    ),
                                            ),
                                    )
                                    // ── 右侧边栏面板（可拖拽调宽，暂时留空）──
                                    .child(
                                        resizable_panel()
                                            .size(px(200.0))
                                            .size_range(px(100.0)..px(500.0))
                                            .flex_none()
                                            .visible(right_sidebar_visible)
                                            .child(
                                                v_flex()
                                                    .size_full()
                                                    .border_l_1()
                                                    .border_color(
                                                        cx.theme().sidebar_border,
                                                    )
                                                    .bg(cx.theme().sidebar)
                                                    // 标题行
                                                    .child(
                                                        h_flex()
                                                            .w_full()
                                                            .px_3()
                                                            .h(px(36.0))
                                                            .flex_shrink_0()
                                                            .items_center()
                                                            .justify_between()
                                                            .border_b_1()
                                                            .border_color(
                                                                cx.theme().sidebar_border,
                                                            )
                                                            .child(
                                                                div()
                                                                    .text_sm()
                                                                    .font_weight(
                                                                        FontWeight::SEMIBOLD,
                                                                    )
                                                                    .text_color(
                                                                        cx.theme()
                                                                            .sidebar_foreground,
                                                                    )
                                                                    .child("面板"),
                                                            )
                                                            .child(
                                                                Button::new(
                                                                    "close-right-sidebar",
                                                                )
                                                                .ghost()
                                                                .icon(Icon::new(
                                                                    IconName::PanelRightClose,
                                                                ))
                                                                .small()
                                                                .tooltip("隐藏右侧面板")
                                                                .on_click(cx.listener(
                                                                    |this, _, _, cx| {
                                                                        this.right_sidebar_visible =
                                                                            false;
                                                                        cx.notify();
                                                                    },
                                                                )),
                                                            ),
                                                    ),
                                            ),
                                    ),
                            ),
                    ),
            )
    }
}

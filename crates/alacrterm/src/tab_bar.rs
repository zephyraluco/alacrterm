//! 自绘的标签栏（dock 的 `TabGroupRenderer`）：定宽标签 + 图标 + 悬停/选中才出现的 `×`。
//!
//! 两层换法：`TerminalDockSkin` 把区级外观全委托给内层 `DockSkin`，只把
//! `tab_group_renderer()` 换成 `TerminalTabBar`；后者只覆写 `render_tab_bar` /
//! `render_active_panel` / `render_drop_indicator`，其余走 trait 默认值。
//!
//! 三条要点（为什么这么做、坑在哪，见 `docs/terminal-architecture.md` §3.2）：
//! 1. 渲染器不是实体（方法是 `&self`）⇒ 悬停状态放 `Cell`，改完要
//!    [`TabBarState::repaint`]，而且**只能通知根视图**；
//! 2. `TabGroupContext` 是只读快照，选中 / 拖放 / 关闭全得回调它；
//! 3. 关会话走 [`AppRoot::close_panel_id`]（dock 的 `close_panel` 拒绝关最后一块面板）。

use std::{cell::Cell, rc::Rc, sync::Arc};

use gpui::{
    AnyElement, AnyView, App, AppContext as _, Axis, Context, CursorStyle, InteractiveElement as _,
    IntoElement, MouseButton, ParentElement as _, Pixels, Render, ScrollHandle, SharedString,
    Stateful, StatefulInteractiveElement as _, StyleRefinement, Styled as _, WeakEntity, Window,
    div, prelude::FluentBuilder as _, px, svg,
};
use gpui_kit::component::{
    ActiveTheme as _, IconNamed as _, Sizable as _, h_flex,
    button::ButtonVariants as _,
    dock::{
        BasePanelView, DockArea, DockAreaRenderer, DockContext, DockSkin, DragPanel, DropIndicator,
        NodeId, PanelHandle, PanelState, TabGroupContext, TabGroupRenderer,
    },
};

use crate::AppRoot;
use crate::assets::IconName;

/// 标签栏高度。
const TAB_BAR_HEIGHT: Pixels = px(32.);
/// 标签定宽：不随标题长短变化。
const TAB_WIDTH: Pixels = px(200.);
const TAB_PADDING_X: Pixels = px(8.);
const TAB_GAP: Pixels = px(6.);
const TAB_ICON_SIZE: Pixels = px(12.);
/// 关闭按钮的固定槽位：不显示时也占位，否则悬停时标题会抖。
const TAB_CLOSE_SLOT: Pixels = px(16.);
const TAB_CLOSE_ICON: Pixels = px(10.);
/// 拖动预览尺寸（与皮肤一致）。
const DRAG_PREVIEW_SIZE: gpui::Size<gpui::Pixels> = gpui::size(px(96.), px(30.));

/// dock 区渲染器：区级外观全委托给 gpui-kit 的 [`DockSkin`]，只换标签栏。
pub(crate) struct TerminalDockSkin {
    inner: Rc<DockSkin>,
    tabs: Rc<TabBarState>,
}

impl TerminalDockSkin {
    /// `root`：悬停重绘与关会话都要经过根视图（见模块文档）。
    pub(crate) fn new(cx: &mut Context<DockArea>, root: WeakEntity<AppRoot>) -> Rc<Self> {
        let inner = DockSkin::new(cx);
        Rc::new(Self {
            inner,
            tabs: Rc::new(TabBarState::new(root)),
        })
    }
}

/// 区级外观全部转给内层皮肤（它知道分栏底色、dock 边框这些细节）。
impl DockAreaRenderer for TerminalDockSkin {
    fn frame(&self, window: &mut Window, cx: &mut App) -> Stateful<gpui::Div> {
        self.inner.frame(window, cx)
    }

    fn center_frame(&self, window: &mut Window, cx: &mut App) -> Stateful<gpui::Div> {
        self.inner.center_frame(window, cx)
    }

    fn split_frame(&self, node: NodeId, axis: Axis, window: &mut Window, cx: &mut App) -> Stateful<gpui::Div> {
        self.inner.split_frame(node, axis, window, cx)
    }

    fn render_dock(
        &self,
        dock: &DockContext,
        content: AnyElement,
        window: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        self.inner.render_dock(dock, content, window, cx)
    }

    fn build_placeholder(
        &self,
        state: &PanelState,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Arc<dyn BasePanelView>> {
        self.inner.build_placeholder(state, window, cx)
    }

    fn tab_group_renderer(&self) -> Rc<dyn TabGroupRenderer> {
        Rc::new(TerminalTabBar {
            state: self.tabs.clone(),
        })
    }
}

/// 标签栏的跨帧状态：渲染器不是实体，这些只能是 [`Cell`]。
struct TabBarState {
    /// 标签横向滚动（标签多到超出时）。
    scroll: ScrollHandle,
    /// 上一帧显示的活动标签，用来「活动标签变了就滚进可视区」。
    last_active: Cell<Option<usize>>,
    /// 鼠标悬停的标签下标。
    hovered: Cell<Option<usize>>,
    /// 应用根视图：悬停重绘与关会话都要经过它。
    root: WeakEntity<AppRoot>,
}

impl TabBarState {
    fn new(root: WeakEntity<AppRoot>) -> Self {
        Self {
            scroll: ScrollHandle::default(),
            last_active: Cell::new(None),
            hovered: Cell::new(None),
            root,
        }
    }

    /// 改完状态后要重绘一次——渲染器自己不是实体，没人会替我们重绘。
    ///
    /// ⚠️ 通知的是**根视图**而不是 dock 区：实测 `area.notify()` 不会让标签组重绘
    /// （标签组是区的子实体，区重绘不等于子实体重绘），而根视图那条路是验证过的
    /// ——终端 OSC 标题一变，标签文字就跟着变（见 `spawn_session` 里的 `observe`）。
    fn repaint(&self, cx: &mut App) {
        _ = self.root.update(cx, |_, cx| cx.notify());
    }

    /// 记录悬停标签；变了才重绘（`on_hover` 每帧都可能回调）。
    fn set_hovered(&self, ix: Option<usize>, cx: &mut App) {
        if self.hovered.replace(ix) != ix {
            self.repaint(cx);
        }
    }
}

/// 自绘标签栏本体。
struct TerminalTabBar {
    state: Rc<TabBarState>,
}

impl TabGroupRenderer for TerminalTabBar {
    fn render_tab_bar(
        &self,
        group: &TabGroupContext,
        window: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        let active_ix = group.active_ix();
        let visible: Vec<usize> = group
            .panels()
            .iter()
            .enumerate()
            .filter(|(_, panel)| panel.visible(cx))
            .map(|(ix, _)| ix)
            .collect();

        // 活动标签变化时把它滚进可视区（仅做最小滚动，和皮肤的做法一致）。
        if self.state.last_active.replace(Some(active_ix)) != Some(active_ix)
            && let Some(visible_ix) = visible.iter().position(|ix| *ix == active_ix)
        {
            self.state.scroll.scroll_to_item(visible_ix);
        }

        let tabs: Vec<AnyElement> = visible
            .iter()
            .map(|ix| self.render_tab(group, *ix, active_ix, window, cx))
            .collect();

        // 右端固定区：当前面板的工具栏按钮（我们只提供 `+` 新建终端）。
        let toolbar = group
            .active_panel()
            .and_then(|panel| PanelHandle::of(panel))
            .and_then(|handle| handle.toolbar_buttons(window, cx));

        let (bar_bg, border) = (cx.theme().tab_bar, cx.theme().border);

        h_flex()
            .w_full()
            .h(TAB_BAR_HEIGHT)
            .bg(bar_bg)
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .overflow_hidden()
                    // 栏底那条线画在标签**下面**：选中标签的不透明底色会盖掉它，
                    // 于是「选中标签与终端连成一体」。
                    .child(
                        div()
                            .absolute()
                            .bottom_0()
                            .left_0()
                            .w_full()
                            .h(px(1.))
                            .bg(border),
                    )
                    .child(
                        h_flex()
                            .id("terminal-tab-strip")
                            .w_full()
                            .h_full()
                            .overflow_x_scroll()
                            .track_scroll(&self.state.scroll)
                            .children(tabs),
                    ),
            )
            .child(
                h_flex()
                    .flex_none()
                    .h_full()
                    .px_1()
                    .gap_1()
                    .border_l_1()
                    .border_color(border)
                    .when_some(toolbar, |this, buttons| {
                        this.children(buttons.into_iter().map(|b| b.xsmall().ghost().tab_stop(false)))
                    }),
            )
            .into_any_element()
    }

    /// 内容区：照抄皮肤的写法（缓存 + 绝对定位铺满，终端流畅靠它）。
    fn render_active_panel(
        &self,
        panel: AnyView,
        _: &TabGroupContext,
        _: &mut Window,
        _: &mut App,
    ) -> AnyElement {
        div()
            .id("tab-content")
            .overflow_y_scroll()
            .overflow_x_hidden()
            .flex_1()
            .child(panel.cached(StyleRefinement::default().absolute().size_full()))
            .into_any_element()
    }

    /// 拖拽落点的预览块（`TabGroup` 已算好矩形，照画即可）。
    fn render_drop_indicator(
        &self,
        indicator: DropIndicator,
        _: &mut Window,
        cx: &mut App,
    ) -> Option<AnyElement> {
        let to = indicator.to();
        Some(
            div()
                .absolute()
                .left(to.origin().x)
                .top(to.origin().y)
                .w(to.size().width)
                .h(to.size().height)
                .bg(cx.theme().tokens.drop_target)
                .into_any_element(),
        )
    }
}

impl TerminalTabBar {
    fn render_tab(
        &self,
        group: &TabGroupContext,
        ix: usize,
        active_ix: usize,
        _window: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        // 颜色先取成局部值：下面要在闭包里可变借用 `cx`。
        let tab_active = cx.theme().tab_active;
        let tab_active_fg = cx.theme().tab_active_foreground;
        let tab_fg = cx.theme().tab_foreground;
        let muted = cx.theme().muted;
        let muted_fg = cx.theme().muted_foreground;
        let border = cx.theme().border;
        let drag_border = cx.theme().drag_border;

        let panel = &group.panels()[ix];
        let selected = ix == active_ix;
        let hovered = self.state.hovered.get() == Some(ix);
        // 关闭按钮：选中常驻，其余悬停才出现。
        let show_close = selected || hovered;
        let panel_id = panel.panel_id(cx);
        let title: SharedString = PanelHandle::of(panel)
            .and_then(|handle| handle.tab_name(cx))
            .unwrap_or_else(|| panel.panel_name(cx).into());

        // element id 里带组节点，避免分屏后两块面板的同一下标撞车。
        let tab_id = (group.node().as_u64() as usize) << 8 | ix;
        // 相邻标签的竖线（每条边界一条线，且选中标签两侧都有线）。
        let draw_left = ix > 0 && ix <= active_ix;
        let draw_right = ix >= active_ix;

        let drag = if group.is_draggable() {
            group.drag_panel(ix, cx)
        } else {
            None
        };

        let close_slot = {
            let slot = h_flex().flex_none().justify_center().size(TAB_CLOSE_SLOT);
            if !show_close {
                slot.into_any_element()
            } else {
                let root = self.state.root.clone();
                slot.id(("terminal-tab-close", tab_id))
                    .rounded(px(3.))
                    .cursor(CursorStyle::PointingHand)
                    .hover(|style| style.bg(muted))
                    .on_click(move |_, window, cx| {
                        // 别再冒泡到标签的「选中」处理。
                        cx.stop_propagation();
                        let _ = root.update(cx, |root, cx| {
                            root.close_panel_id(panel_id, window, cx)
                        });
                    })
                    .child(
                        svg()
                            .path(IconName::Close.path())
                            .w(TAB_CLOSE_ICON)
                            .h(TAB_CLOSE_ICON)
                            .text_color(muted_fg),
                    )
                    .into_any_element()
            }
        };

        h_flex()
            .id(("terminal-tab", tab_id))
            .flex_none()
            .w(TAB_WIDTH)
            .h_full()
            .relative()
            .gap(TAB_GAP)
            .px(TAB_PADDING_X)
            .border_color(border)
            .when(draw_left, |this| this.border_l_1())
            .when(draw_right, |this| this.border_r_1())
            .when(selected, |this| this.bg(tab_active).text_color(tab_active_fg))
            .when(!selected, |this| {
                this.text_color(tab_fg).hover(|style| style.bg(muted))
            })
            .cursor(CursorStyle::PointingHand)
            // 悬停要自己记账（gpui-pre 没有 `visible_on_hover`）。
            .on_hover({
                let state = self.state.clone();
                move |hovered: &bool, _, cx| state.set_hovered(hovered.then_some(ix), cx)
            })
            .on_click({
                let group = group.clone();
                move |_, window, cx| group.select_tab(ix, window, cx)
            })
            // 中键关闭。
            .on_mouse_up(MouseButton::Middle, {
                let root = self.state.root.clone();
                move |_, window, cx| {
                    let _ = root.update(cx, |root, cx| root.close_panel_id(panel_id, window, cx));
                }
            })
            // 拖动换位 / 拖出去分屏：只提供载荷与落点，状态机在 dock 里。
            .when_some(drag, |this, drag| {
                this.on_drag(drag, {
                    let title = title.clone();
                    move |drag, offset, _, cx| {
                        drag.set_drag_offset(offset);
                        drag.set_preview_size(DRAG_PREVIEW_SIZE);
                        cx.new(|_| TabDragPreview {
                            title: title.clone(),
                        })
                    }
                })
            })
            .when(group.is_droppable(), {
                // 落点上高亮左边框，提示「会插到这个标签前面」。
                let on_drop_panel = {
                    let group = group.clone();
                    move |drag: &DragPanel, window: &mut Window, cx: &mut App| {
                        group.drop_panel(drag.clone(), Some(ix), true, window, cx)
                    }
                };
                move |this| {
                    this.drag_over::<DragPanel>(move |this, _, _, _| {
                        this.border_l_2().border_color(drag_border)
                    })
                    .on_drop(on_drop_panel)
                }
            })
            .child(
                svg()
                    .path(IconName::SquareTerminal.path())
                    .flex_none()
                    .w(TAB_ICON_SIZE)
                    .h(TAB_ICON_SIZE)
                    .text_color(if selected { tab_active_fg } else { muted_fg }),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .child(title),
            )
            .child(close_slot)
            .into_any_element()
    }
}

/// 拖标签时跟着鼠标的小卡片。
struct TabDragPreview {
    title: SharedString,
}

impl Render for TabDragPreview {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        h_flex()
            .gap_2()
            .px_3()
            .py_1()
            .rounded(px(6.))
            .bg(cx.theme().tab_active)
            .text_color(cx.theme().tab_active_foreground)
            .child(
                svg()
                    .path(IconName::SquareTerminal.path())
                    .w(TAB_ICON_SIZE)
                    .h(TAB_ICON_SIZE),
            )
            .child(self.title.clone())
    }
}

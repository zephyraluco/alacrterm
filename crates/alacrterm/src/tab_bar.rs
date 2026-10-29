//! 自绘标签栏（dock 的 `TabGroupRenderer`）：定宽标签 + 图标 + 关闭按钮。
//! 渲染器不是实体：跨帧状态放 `Cell`，改完回调根视图重绘。

use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    rc::Rc,
    sync::Arc,
};

use gpui::{
    AnyElement, AnyView, App, AppContext as _, Axis, Context, CursorStyle, FocusHandle,
    InteractiveElement as _, IntoElement, MouseButton, ParentElement as _, Pixels, Render,
    ScrollHandle, SharedString, Stateful, StatefulInteractiveElement as _, StyleRefinement,
    Styled as _, WeakEntity, Window, WindowControlArea, div, prelude::FluentBuilder as _, px, svg,
};
use gpui_kit::{
    base::ResizeHandleContext,
    component::{
        ActiveTheme as _, IconNamed as _, Sizable as _, TITLE_BAR_HEIGHT,
        button::{Button, ButtonVariants as _},
        dock::{
            BasePanelView, DockArea, DockAreaRenderer, DockContext, DockSkin, DragPanel,
            DropIndicator, NodeId, PanelHandle, PanelState, TabGroupContext, TabGroupRenderer,
        },
        h_flex,
    },
};

use crate::AppRoot;
use crate::assets::IconName;
use crate::terminal_panel::TabPane;

const TAB_BAR_HEIGHT: Pixels = TITLE_BAR_HEIGHT;
const TAB_WIDTH: Pixels = px(200.);
const TAB_PADDING_X: Pixels = px(8.);
const TAB_GAP: Pixels = px(6.);
const TAB_ICON_SIZE: Pixels = px(12.);
/// 关闭按钮槽位（隐藏时占位）。
const TAB_CLOSE_SLOT: Pixels = px(16.);
const TAB_CLOSE_ICON: Pixels = px(10.);
const DRAG_PREVIEW_SIZE: gpui::Size<gpui::Pixels> = gpui::size(px(96.), px(30.));

/// dock 区渲染器：只替换标签栏。
pub(crate) struct TerminalDockSkin {
    inner: Rc<DockSkin>,
    tabs: Rc<TabBarState>,
}

impl TerminalDockSkin {
    pub(crate) fn new(cx: &mut Context<DockArea>, root: WeakEntity<AppRoot>) -> Rc<Self> {
        let inner = DockSkin::new(cx);
        Rc::new(Self {
            inner,
            tabs: Rc::new(TabBarState::new(root)),
        })
    }
}

impl DockAreaRenderer for TerminalDockSkin {
    fn frame(&self, window: &mut Window, cx: &mut App) -> Stateful<gpui::Div> {
        self.inner.frame(window, cx)
    }

    fn center_frame(&self, window: &mut Window, cx: &mut App) -> Stateful<gpui::Div> {
        self.inner.center_frame(window, cx)
    }

    fn split_frame(
        &self,
        node: NodeId,
        axis: Axis,
        window: &mut Window,
        cx: &mut App,
    ) -> Stateful<gpui::Div> {
        self.inner.split_frame(node, axis, window, cx)
    }

    /// 分叉的分隔条（「两个标签组之间」那条缝）：本皮肤不自己画，转发给设计系统
    /// —— 和侧边栏、页内分屏同一套（细线 + 悬停 / 拖拽胶囊）。
    ///
    /// ⚠️ 不转发就等于用**基类默认**（返回 `None` = 朴素 1px 线）：这个钩子只决定
    /// *画什么*，抓取带、光标和拖拽都在 base 那边的 `ResizablePanelGroup` 里
    /// （dock 的每个分叉本来就是 `h_resizable` / `v_resizable`）。
    fn render_split_handle(
        &self,
        handle: &ResizeHandleContext,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<AnyElement> {
        self.inner.render_split_handle(handle, window, cx)
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

/// 标签栏跨帧状态。
struct TabBarState {
    scroll: ScrollHandle,
    last_active: Cell<Option<usize>>,
    hovered: Cell<Option<usize>>,
    menu_group: Cell<Option<NodeId>>,
    /// 应用根视图。
    root: WeakEntity<AppRoot>,
    group_focus: RefCell<HashMap<NodeId, FocusHandle>>,
}

impl TabBarState {
    fn new(root: WeakEntity<AppRoot>) -> Self {
        Self {
            scroll: ScrollHandle::default(),
            last_active: Cell::new(None),
            hovered: Cell::new(None),
            menu_group: Cell::new(None),
            root,
            group_focus: RefCell::new(HashMap::new()),
        }
    }

    fn group_focus(&self, node: NodeId, cx: &mut App) -> FocusHandle {
        self.group_focus
            .borrow_mut()
            .entry(node)
            .or_insert_with(|| cx.focus_handle())
            .clone()
    }

    /// 通知根视图重绘。
    fn repaint(&self, cx: &mut App) {
        _ = self.root.update(cx, |_, cx| cx.notify());
    }

    /// 记录悬停标签（变化才重绘）。
    fn set_hovered(&self, ix: Option<usize>, cx: &mut App) {
        if self.hovered.replace(ix) != ix {
            self.repaint(cx);
        }
    }
}

pub(crate) struct TerminalTabBar {
    state: Rc<TabBarState>,
}

impl TabGroupRenderer for TerminalTabBar {
    fn frame(&self, group: &TabGroupContext, _: &mut Window, cx: &mut App) -> Stateful<gpui::Div> {
        let focus = group.active_panel().map(|panel| panel.focus_handle(cx));
        let group_focus = self.state.group_focus(group.node(), cx);
        div()
            .id("terminal-tab-group")
            .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                // 组内工具按钮不能冒泡到应用背景并清掉整组焦点。
                if let Some(focus) = &focus
                    && !focus.contains_focused(window, cx)
                    && !group_focus.contains_focused(window, cx)
                {
                    window.focus(focus, cx);
                }
                cx.stop_propagation();
            })
    }

    fn render_tab_bar(
        &self,
        group: &TabGroupContext,
        window: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        if group.is_zoomed() {
            return div().into_any_element();
        }
        self.render_title_bar(group, window, cx)
    }

    /// 内容区：缓存 + 铺满。
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

    /// 拖拽落点预览块。
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
    pub(crate) fn new(root: WeakEntity<AppRoot>) -> Self {
        Self {
            state: Rc::new(TabBarState::new(root)),
        }
    }

    pub(crate) fn render_title_bar(
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

        // 活动标签滚入可视区。
        if self.state.last_active.replace(Some(active_ix)) != Some(active_ix)
            && let Some(visible_ix) = visible.iter().position(|ix| *ix == active_ix)
        {
            self.state.scroll.scroll_to_item(visible_ix);
        }

        let tabs: Vec<AnyElement> = visible
            .iter()
            .map(|ix| self.render_tab(group, *ix, active_ix, window, cx))
            .collect();

        let group_focus = self.state.group_focus(group.node(), cx);
        // 终端与工具栏共同组成停靠组的焦点范围。
        let toolbar = group
            .active_panel()
            .filter(|panel| {
                group.is_zoomed()
                    || panel.focus_handle(cx).contains_focused(window, cx)
                    || group_focus.contains_focused(window, cx)
                    || self.state.menu_group.get() == Some(group.node())
            })
            .and_then(|panel| PanelHandle::of(panel))
            .and_then(|handle| handle.view().downcast::<TabPane>().ok())
            .map(|pane| {
                let state = self.state.clone();
                let node = group.node();
                pane.read(cx).new_terminal_menu(move |open, _, cx| {
                    if *open {
                        state.menu_group.set(Some(node));
                    } else if state.menu_group.get() == Some(node) {
                        state.menu_group.set(None);
                    }
                    cx.refresh_windows();
                })
            });

        let (bar_bg, border) = (cx.theme().tab_bar, cx.theme().border);

        let zoom_group = group.clone();
        let bar = h_flex()
            .w_full()
            .h(TAB_BAR_HEIGHT)
            .when(group.is_zoomed(), |bar| bar.h_full())
            .bg(bar_bg)
            // 焦点落在标签栏/工具按钮上时的 key context（默认 keymap 未使用，留给用户）。
            .key_context(crate::keymap::DOCK_CONTEXT)
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .overflow_hidden()
                    // Zoom 的整行底线由根标题栏统一绘制。
                    .when(!group.is_zoomed(), |this| {
                        this.child(
                            div()
                                .absolute()
                                .bottom_0()
                                .left_0()
                                .w_full()
                                .h(px(1.))
                                .bg(border),
                        )
                    })
                    .child(
                        h_flex()
                            .size_full()
                            .child(
                                h_flex()
                                    .id("terminal-tab-strip")
                                    .w(TAB_WIDTH * visible.len() as f32)
                                    .max_w_full()
                                    .h_full()
                                    .overflow_x_scroll()
                                    .track_scroll(&self.state.scroll)
                                    .children(tabs),
                            )
                            .child(
                                div()
                                    .id("terminal-title-drag-area")
                                    .flex_1()
                                    .min_w_0()
                                    .h_full()
                                    .when(group.is_zoomed(), |this| {
                                        this.window_control_area(WindowControlArea::Drag)
                                    }),
                            ),
                    ),
            )
            .when_some(toolbar, |this, menu| {
                this.child(
                    h_flex()
                        .id("terminal-toolbar")
                        .occlude()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .track_focus(&group_focus)
                        .flex_none()
                        .h_full()
                        .px_1()
                        .gap_1()
                        .border_l_1()
                        .border_color(border)
                        .child(menu)
                        .child(
                            Button::new("zoom-terminal-group")
                                .xsmall()
                                .ghost()
                                .tab_stop(false)
                                .icon(if group.is_zoomed() {
                                    IconName::Minimize
                                } else {
                                    IconName::Maximize
                                })
                                .tooltip(if group.is_zoomed() {
                                    "还原终端布局"
                                } else {
                                    "放大终端组"
                                })
                                .accessibility_label(if group.is_zoomed() {
                                    "还原终端布局"
                                } else {
                                    "放大终端组"
                                })
                                .on_click(move |_, window, cx| zoom_group.toggle_zoom(window, cx)),
                        ),
                )
            });
        bar.into_any_element()
    }

    fn render_tab(
        &self,
        group: &TabGroupContext,
        ix: usize,
        active_ix: usize,
        _window: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        let tab_active = cx.theme().tab_active;
        let tab_active_fg = cx.theme().tab_active_foreground;
        let tab_fg = cx.theme().tab_foreground;
        let muted = cx.theme().muted;
        let muted_fg = cx.theme().muted_foreground;
        let border = cx.theme().border;
        let drag_border = cx.theme().drag_border;

        let panel = &group.panels()[ix];
        let focus_handle = panel.focus_handle(cx);
        let selected = ix == active_ix;
        let hovered = self.state.hovered.get() == Some(ix);
        // 关闭按钮：选中常驻，否则悬停。
        let show_close = selected || hovered;
        let panel_id = panel.panel_id(cx);
        let title: SharedString = PanelHandle::of(panel)
            .and_then(|handle| handle.tab_name(cx))
            .unwrap_or_else(|| panel.panel_name(cx).into());

        // id 带组节点，避免同下标撞车。
        let tab_id = (group.node().as_u64() as usize) << 8 | ix;
        // 相邻标签的竖线。
        let draw_left = ix > 0 && ix <= active_ix;
        let draw_right = ix >= active_ix;

        let drag = if group.is_draggable() || group.is_zoomed() {
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
                        cx.stop_propagation();
                        let _ =
                            root.update(cx, |root, cx| root.close_panel_id(panel_id, window, cx));
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
            .occlude()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .flex_none()
            .w(TAB_WIDTH)
            .h_full()
            .relative()
            .gap(TAB_GAP)
            .px(TAB_PADDING_X)
            .border_color(border)
            .when(draw_left, |this| this.border_l_1())
            .when(draw_right, |this| this.border_r_1())
            .when(selected, |this| {
                this.bg(tab_active).text_color(tab_active_fg)
            })
            .when(!selected, |this| {
                this.text_color(tab_fg).hover(|style| style.bg(muted))
            })
            .cursor(CursorStyle::PointingHand)
            .on_hover({
                let state = self.state.clone();
                move |hovered: &bool, _, cx| state.set_hovered(hovered.then_some(ix), cx)
            })
            .on_click({
                let group = group.clone();
                move |_, window, cx| {
                    group.select_tab(ix, window, cx);
                    // 已选中的标签仍需把焦点从其他停靠组移回来。
                    window.focus(&focus_handle, cx);
                }
            })
            .on_mouse_up(MouseButton::Middle, {
                let root = self.state.root.clone();
                move |_, window, cx| {
                    let _ = root.update(cx, |root, cx| root.close_panel_id(panel_id, window, cx));
                }
            })
            // 拖动换位 / 分屏：只给载荷与落点。
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
            .when(group.is_droppable() || group.is_zoomed(), {
                let on_drop_panel = {
                    let group = group.clone();
                    move |drag: &DragPanel, window: &mut Window, cx: &mut App| {
                        if !group.is_zoomed() || drag.source() == group.node() {
                            group.drop_panel(drag.clone(), Some(ix), true, window, cx);
                        }
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

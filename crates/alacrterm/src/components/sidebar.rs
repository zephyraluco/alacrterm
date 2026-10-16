//! 侧边栏面板:顶部标题栏 + 工具栏 + 下方可滚动的内容列表 + 底部状态栏。

use std::rc::Rc;

use gpui::{
    AnyElement, App, ElementId, InteractiveElement as _, IntoElement, ListAlignment, ListState,
    ParentElement as _, RenderOnce, SharedString, Styled as _, Window, div, list,
    prelude::FluentBuilder as _, px,
};
use gpui_kit::base::{Scrollbar, v_flex};
use gpui_kit::component::ActiveTheme as _;

const CONTENT_PADDING: gpui::Pixels = px(12.);
const HEADER_PADDING: gpui::Pixels = px(12.);
/// 工具栏与下方列表之间的空隙。
const TOOLBAR_GAP: gpui::Pixels = px(8.);
const LIST_OVERDRAW_RATIO: f32 = 0.3;

/// 侧边栏里的一行。
pub(crate) trait SidebarRow: Clone + 'static {
    /// 画这一行。
    fn render(self, cx: &mut App) -> AnyElement;
}

/// 侧边栏面板(左栏)。
#[derive(IntoElement)]
pub(crate) struct Sidebar<E: SidebarRow> {
    id: ElementId,
    header: Option<AnyElement>,
    /// 标题栏下面那一行工具条（「文件管理器」的图标按钮就摆这儿）。
    toolbar: Option<AnyElement>,
    status_bar: Option<AnyElement>,
    rows: Vec<E>,
}

impl<E: SidebarRow> Sidebar<E> {
    pub(crate) fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            header: None,
            toolbar: None,
            status_bar: None,
            rows: Vec::new(),
        }
    }

    /// 顶部标题栏。
    pub(crate) fn header(mut self, header: impl IntoElement) -> Self {
        self.header = Some(header.into_any_element());
        self
    }

    /// 标题栏下面那一行工具条（固定、不随内容滚动）。
    pub(crate) fn toolbar(mut self, toolbar: Option<AnyElement>) -> Self {
        self.toolbar = toolbar;
        self
    }

    /// 底部状态栏（贴在面板下沿，不随内容滚动）。
    pub(crate) fn status_bar(mut self, status_bar: impl IntoElement) -> Self {
        self.status_bar = Some(status_bar.into_any_element());
        self
    }

    /// 内容:一行一项。
    pub(crate) fn children(mut self, rows: impl IntoIterator<Item = E>) -> Self {
        self.rows.extend(rows);
        self
    }
}

impl<E: SidebarRow> RenderOnce for Sidebar<E> {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let id = self.id;
        let count = self.rows.len();
        let overdraw = px(window.viewport_size().height.as_f32() * LIST_OVERDRAW_RATIO);
        let list_state = window
            .use_keyed_state(
                SharedString::from(format!("{id}-list-state")),
                cx,
                |_, _| ListState::new(count, ListAlignment::Top, overdraw),
            )
            .read(cx)
            .clone();
        if list_state.item_count() != count {
            list_state.reset(count);
        }

        let rows = Rc::new(self.rows);
        v_flex()
            .id(id.clone())
            .size_full()
            .overflow_hidden()
            .bg(cx.theme().tokens.sidebar)
            .text_color(cx.theme().sidebar_foreground)
            .when_some(self.header, |this, header| {
                this.child(
                    v_flex()
                        .id(format!("{id}-header"))
                        .pt(HEADER_PADDING)
                        .px(HEADER_PADDING)
                        .gap_2()
                        .child(header),
                )
            })
            // 工具栏:标题栏与列表之间那条(只有「文件管理器」才挂)。
            .when_some(self.toolbar, |this, toolbar| {
                this.child(
                    v_flex()
                        .id(format!("{id}-toolbar"))
                        .px(CONTENT_PADDING)
                        .pb(TOOLBAR_GAP)
                        .child(toolbar),
                )
            })
            .child(
                div()
                    .id(format!("{id}-scroll"))
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .child(
                        v_flex()
                            .id(format!("{id}-content"))
                            .size_full()
                            .px(CONTENT_PADDING)
                            .gap_y(CONTENT_PADDING)
                            .child(
                                list(list_state.clone(), move |ix, _window, cx| {
                                    let row = rows.get(ix).cloned();
                                    let is_first = ix == 0;
                                    let is_last = ix + 1 == count;
                                    div()
                                        .id(ix)
                                        .when(is_first, |this| this.pt(CONTENT_PADDING))
                                        .when(is_last, |this| this.pb(CONTENT_PADDING))
                                        .when_some(row, |this, row| this.child(row.render(cx)))
                                        .into_any_element()
                                })
                                .size_full(),
                            ),
                    )
                    .child(
                        div().absolute().inset_0().child(
                            Scrollbar::vertical(&list_state)
                                .id(format!("{id}-scrollbar"))
                                .viewport_from_layout(),
                        ),
                    ),
            )
            // 底部状态栏：固定高度、贴面板下沿（标题栏在上、内容区自己滚）。
            .when_some(self.status_bar, |this, status_bar| {
                this.child(status_bar)
            })
    }
}

//! 侧边栏面板:顶部标题栏 + 下方可滚动的内容列表。用 `base` 的 `v_flex`/`div`、`Scrollbar`
//! 与 gpui 的 `list`(`ListState` 按可见区间取行)实现。
//!
//! 列表按索引懒渲染,行类型需 `Clone`(见 [`SidebarRow`]);行自己的交互(点击 / 右键 /
//! 拖放)在行元素里,这里只管排版与滚动。

use std::rc::Rc;

use gpui::{
    AnyElement, App, ElementId, InteractiveElement as _, IntoElement, ListAlignment, ListState,
    ParentElement as _, RenderOnce, SharedString, Styled as _, Window, div, list,
    prelude::FluentBuilder as _, px,
};
use gpui_kit::base::{Scrollbar, v_flex};
use gpui_kit::component::ActiveTheme as _;

/// 内容区左右内边距 / 行间距。
const CONTENT_PADDING: gpui::Pixels = px(12.);
/// 标题栏内边距。
const HEADER_PADDING: gpui::Pixels = px(12.);
/// 列表按视口高度额外渲染的比例(滚动时不露白)。
const LIST_OVERDRAW_RATIO: f32 = 0.3;

/// 侧边栏里的一行。
///
/// 列表按索引懒渲染 ⇒ 行必须 `Clone`。
pub(crate) trait SidebarRow: Clone + 'static {
    /// 画这一行(`render` 拿的是普通 `&mut App`)。
    fn render(self, cx: &mut App) -> AnyElement;
}

/// 侧边栏面板(左栏):尺寸由外层槽位给,内容一行一项由调用方给。
#[derive(IntoElement)]
pub(crate) struct Sidebar<E: SidebarRow> {
    id: ElementId,
    header: Option<AnyElement>,
    rows: Vec<E>,
}

impl<E: SidebarRow> Sidebar<E> {
    pub(crate) fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            header: None,
            rows: Vec::new(),
        }
    }

    /// 顶部标题栏(不随内容滚动)。
    pub(crate) fn header(mut self, header: impl IntoElement) -> Self {
        self.header = Some(header.into_any_element());
        self
    }

    /// 内容:一行一项,按索引懒渲染。
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
            .child(
                // 滚动区 + 滚动条(后者是 sibling,盖在上面、不跟内容滚)。
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
                                // 视口 = 这块覆盖层的边界。
                                .viewport_from_layout(),
                        ),
                    ),
            )
    }
}

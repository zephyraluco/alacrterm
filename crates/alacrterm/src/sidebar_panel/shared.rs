//! 「会话」与「文件管理器」两棵树共用的行渲染、摊平缓存与拖拽预览。

use std::rc::Rc;

use gpui::{
    AnyElement, App, Context, IntoElement, ParentElement as _, Pixels, Render, SharedString,
    Styled as _, Window, div, prelude::FluentBuilder as _, px,
};
use gpui_kit::component::{ActiveTheme as _, Icon, StyledExt as _, h_flex, list::ListItem};

use crate::assets::IconName;

/// 树行高度。
pub(super) const TREE_ROW_HEIGHT: Pixels = px(28.);

/// 行内容的缩进。
pub(super) fn indent(depth: usize) -> Pixels {
    px(8. + depth as f32 * 16.)
}

/// 行标题：单行省略号。
pub(super) fn ellipsis_label(label: SharedString) -> AnyElement {
    div()
        .min_w_0()
        .overflow_hidden()
        .whitespace_nowrap()
        .text_ellipsis()
        .child(label)
        .into_any_element()
}

/// 行的骨架：高度 / 缩进 / 圆角 / 选中高亮。
pub(super) fn row_shell(ix: usize, depth: usize, selected: bool, cx: &mut App) -> ListItem {
    let (radius, accent_fg) = {
        let theme = cx.theme();
        (theme.radius, theme.sidebar_accent_foreground)
    };
    ListItem::new(ix)
        .h(TREE_ROW_HEIGHT)
        .w_full()
        .pr_2()
        .rounded(radius)
        .text_sm()
        .overflow_x_hidden()
        .pl(indent(depth))
        .when(selected, |this| this.font_medium().text_color(accent_fg))
}

/// 行的内容：前导图标 + 类型图标 + 名字。
pub(super) fn row_content(
    leading: Option<IconName>,
    icon: IconName,
    label: SharedString,
) -> AnyElement {
    let content = h_flex().gap_x_2().items_center();
    let content = match leading {
        Some(leading) => content.child(Icon::new(leading).size_3()),
        None => content,
    };
    content
        .child(Icon::new(icon).size_3())
        .child(ellipsis_label(label))
        .into_any_element()
}

/// 摊平结果的缓存。
pub(super) struct RowCache<T> {
    version: u64,
    rows: Option<(u64, Rc<Vec<T>>)>,
}

impl<T> Default for RowCache<T> {
    fn default() -> Self {
        Self {
            version: 0,
            rows: None,
        }
    }
}

impl<T> RowCache<T> {
    /// 让缓存失效。
    pub(super) fn bump(&mut self) {
        self.version += 1;
    }

    /// 版本没变就返回上次摊平的结果。
    pub(super) fn cached(&self) -> Option<Rc<Vec<T>>> {
        let (version, rows) = self.rows.as_ref()?;
        (*version == self.version).then(|| rows.clone())
    }

    /// 存下这次摊平的结果。
    pub(super) fn store(&mut self, rows: Vec<T>) -> Rc<Vec<T>> {
        let rows = Rc::new(rows);
        self.rows = Some((self.version, rows.clone()));
        rows
    }

    /// 按行号取一行。
    pub(super) fn row(&self, ix: usize) -> Option<T>
    where
        T: Clone,
    {
        self.cached()?.get(ix).cloned()
    }
}

/// 拖动时跟着鼠标的小卡片。
pub(super) struct DragPreview {
    pub(super) label: SharedString,
}

impl Render for DragPreview {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .px_2()
            .h(super::PANEL_HEADER_HEIGHT)
            .flex()
            .items_center()
            .rounded(cx.theme().radius)
            .bg(cx.theme().tokens.accent)
            .text_color(cx.theme().accent_foreground)
            .child(self.label.clone())
    }
}

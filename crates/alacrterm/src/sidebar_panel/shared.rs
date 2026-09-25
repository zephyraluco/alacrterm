//! 侧边栏共用的小件：「会话」与「文件管理器」两棵树的行渲染骨架 + 摊平缓存 + 拖拽预览。
//!
//! 两个视图都遵循「摊平成一行一项，交给侧边栏自己的虚拟列表」（动机见 `files` 模块文档），
//! 于是这些部件天然重复：行高 / 缩进 / 圆角 / 选中高亮 / 名字截断 / 摊平结果缓存 /
//! 拖动时跟着鼠标的小卡片。收在这里，改一处两处生效。

use std::rc::Rc;

use gpui::{
    AnyElement, App, Context, IntoElement, ParentElement as _, Pixels, Render, SharedString,
    Styled as _, Window, div, prelude::FluentBuilder as _, px,
};
use gpui_kit::component::{ActiveTheme as _, Icon, StyledExt as _, h_flex, list::ListItem};

use crate::assets::IconName;

/// 树行高度（逻辑像素）：两棵树的行高一致。
pub(super) const TREE_ROW_HEIGHT: Pixels = px(28.);

/// 行内容的缩进：每层 16px，顶层留 8px 内边距。
pub(super) fn indent(depth: usize) -> Pixels {
    px(8. + depth as f32 * 16.)
}

/// 行标题：单行省略号（树不换行，窄侧边栏里长名字要被截断）。
pub(super) fn ellipsis_label(label: SharedString) -> AnyElement {
    div()
        .min_w_0()
        .overflow_hidden()
        .whitespace_nowrap()
        .text_ellipsis()
        .child(label)
        .into_any_element()
}

/// 行的骨架：高度 / 缩进 / 圆角 / 选中高亮（悬停与选中的观感都交给 [`ListItem`]）。
///
/// ⚠️ 选中底色是主题 `accent`：见 [`crate::config::change_theme`] 里关掉
/// `list.active_highlight` 的原因（默认那个太淡，分不出悬停）。
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

/// 行的内容：可选的前导图标（文件夹的 caret）+ 类型图标 + 名字。
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
///
/// ⚠️ 必须缓存：侧边栏每帧都会来要一次行清单，而摊平是 `O(条目数)`。
/// 内容 / 展开状态一变就 [`RowCache::bump`]，取清单走 [`RowCache::cached`] + [`RowCache::store`]。
// ⚠️ 不用 `derive(Default)`：那会给 `T` 强加 `Default` 约束，而行类型（`FileRow` 等）没有。
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
    /// 让缓存失效（条目树 / 展开状态变了）。
    pub(super) fn bump(&mut self) {
        self.version += 1;
    }

    /// 版本没变就返回上次摊平的结果。
    pub(super) fn cached(&self) -> Option<Rc<Vec<T>>> {
        let (version, rows) = self.rows.as_ref()?;
        (*version == self.version).then(|| rows.clone())
    }

    /// 存下这次摊平的结果（返回句柄，免得调用方再 clone 一次）。
    pub(super) fn store(&mut self, rows: Vec<T>) -> Rc<Vec<T>> {
        let rows = Rc::new(rows);
        self.rows = Some((self.version, rows.clone()));
        rows
    }

    /// 按行号取一行（缓存未命中 / 越界 ⇒ `None`：行数刚变过时外层可能还在渲染旧下标）。
    pub(super) fn row(&self, ix: usize) -> Option<T>
    where
        T: Clone,
    {
        self.cached()?.get(ix).cloned()
    }
}

/// 拖动一个条目 / 标签时跟着鼠标的小卡片（侧边栏的两处拖拽共用）。
pub(super) struct DragPreview {
    pub(super) label: SharedString,
}

impl Render for DragPreview {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .px_2()
            .h(super::TAB_HEIGHT)
            .flex()
            .items_center()
            .rounded(cx.theme().radius)
            .bg(cx.theme().tokens.accent)
            .text_color(cx.theme().accent_foreground)
            .child(self.label.clone())
    }
}

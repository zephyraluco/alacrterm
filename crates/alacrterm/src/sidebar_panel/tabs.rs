//! 侧边栏顶部的**视图标签条**:点选、拖动换位、拖到另一条侧边栏。
//!
//! 外观照 **VS Code 的面板标签**(问题 / 输出 / 调试控制台 / 终端 / 端口):纯文字、
//! 无边框、按内容宽度左对齐,只有悬停 / 选中的那块有圆角浅灰底
//! (`tokens.accent` 底 + `accent_foreground` 字,圆角用主题 `radius`)。
//! 只有一个标签也照画(右侧边栏默认就是一项)。
//!
//! 标签数据(顺序 + 选中)在 [`SidebarTabs`],由它所属的那条 [`Sidebar`](super::Sidebar) 持有;
//! 本模块只画标签、把交互转发回那个实体(换位 / 跨栏搬运全在 `Sidebar` 的方法里,
//! 视图内容见 [`super::sessions`] / [`super::session_info`])。

use gpui::{
    AnyElement, AppContext as _, Context, CursorStyle, InteractiveElement as _, IntoElement,
    ParentElement as _, Pixels, Render, SharedString, StatefulInteractiveElement as _, Styled as _,
    Window, div, prelude::FluentBuilder as _, px,
};
use gpui_kit::component::{ActiveTheme as _, h_flex};

use super::{Sidebar, SidebarSide, SidebarTabs, SidebarView, TAB_HEIGHT};

/// 标签之间的缝:主要间距来自各自的左右内边距。
const TAB_GAP: Pixels = px(2.);
/// 标签左右内边距。
const TAB_PADDING_X: Pixels = px(8.);

/// 拖动中的侧边栏标签（拖放载荷：从哪一侧的第几个标签拖出来的）。
pub(super) struct DragSidebarTab {
    /// 来自哪一侧。
    pub(super) side: SidebarSide,
    /// 在来源侧标签条里的下标（按下标取标签，避免同名视图重定位）。
    pub(super) index: usize,
}

/// 拖标签时跟着鼠标的小卡片。
struct TabDragPreview {
    label: SharedString,
}

impl Render for TabDragPreview {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .px_2()
            .h(TAB_HEIGHT)
            .flex()
            .items_center()
            .rounded(cx.theme().radius)
            .bg(cx.theme().tokens.accent)
            .text_color(cx.theme().accent_foreground)
            .child(self.label.clone())
    }
}

impl Sidebar {
    /// 本栏顶部的标签条（点选 / 拖动换位 / 拖到另一栏，见 [`tab_bar`]）。
    ///
    /// 挂在 `Sidebar` 组件的 `header` 上 ⇒ 固定在顶部（不随内容滚动），侧边栏折叠时一起隐藏。
    /// 设置入口不在这里——它是标题栏左侧的「设置」文字按钮（见 [`crate::AppRoot::render`]）。
    ///
    /// 返回 [`AnyElement`] 而非 `impl IntoElement`：本 crate 是 edition 2024，
    /// `impl Trait` 会捕获 `&mut Context` 的生命周期，导致同一渲染树里
    /// 连续调用多个 `&mut cx` 的渲染方法时借用冲突；装箱可彻底规避。
    pub(super) fn render_tabs(&self, cx: &mut Context<Self>) -> AnyElement {
        tab_bar(self.side, &self.tabs, cx)
    }
}

/// 标签条：一条侧边栏顶部那一行标签（点选 / 拖动换位 / 拖到另一栏）。
///
/// 拖放：每个标签自己既是拖源也是落点（落在第 `ix` 个标签上 = 占它的位置）；
/// 标签条本身的空白区域（含空标签条）是**兜底落点** = 追加到末尾。
/// 落点语义由 [`Sidebar::drop_tab`] / [`Sidebar::append_tab`] 实现。
fn tab_bar(side: SidebarSide, tabs: &SidebarTabs, cx: &mut Context<Sidebar>) -> AnyElement {
    // 先收成 `Vec`：下面还要用 `cx.listener` 借一次 `cx`，迭代器会一直绕着它。
    let items: Vec<AnyElement> = tabs
        .views
        .iter()
        .enumerate()
        .map(|(ix, view)| tab_element(side, ix, *view, ix == tabs.active, cx))
        .collect();

    h_flex()
        .w_full()
        // 必须显式给高度：标签条可能一个标签都没有（都被拖走），
        // 没有子元素时 flex 容器的高度会塌成 0 ⇒ 兜底落点根本悬停不到。
        .h(TAB_HEIGHT)
        .gap(TAB_GAP)
        // 兜底落点：拖到标签右侧的空白（或目标栏为空时只能拖到这里）⇒ 追加到末尾。
        .on_drop(
            cx.listener(move |this, drag: &DragSidebarTab, _, cx| this.append_tab(drag, cx)),
        )
        .children(items)
        .into_any_element()
}

/// 一个标签：点选 + 可拖动（拖到本栏另一个标签上 = 换位，拖到另一栏 = 换面板）。
///
/// 左/右两条标签条共用；`side` 与 `index` 是拖放载荷的来源信息（见 [`DragSidebarTab`]）。
fn tab_element(
    side: SidebarSide,
    index: usize,
    view: SidebarView,
    selected: bool,
    cx: &mut Context<Sidebar>,
) -> AnyElement {
    let (accent, accent_fg, fg, radius) = {
        let theme = cx.theme();
        (
            theme.tokens.accent,
            theme.accent_foreground,
            theme.tab_foreground,
            theme.radius,
        )
    };

    h_flex()
        .id(format!("sidebar-tab-{}", view.id()))
        .flex_none()
        .h(TAB_HEIGHT)
        .px(TAB_PADDING_X)
        .items_center()
        .rounded(radius)
        // 常驻一条透明左边框：拖动悬停时把它点亮成插入位置提示，
        // 这样高亮不会把标签尺寸顶变（否则整条标签会在拖动时抽动）。
        .border_l_2()
        .border_color(accent.opacity(0.0))
        .cursor(CursorStyle::PointingHand)
        .text_color(if selected { accent_fg } else { fg })
        // 标签文字很短，给读屏 / 无障碍一个完整说明。
        .aria_label(view.tooltip())
        .when(selected, |this| this.bg(accent))
        .when(!selected, |this| {
            this.hover(|style| style.bg(accent).text_color(accent_fg))
        })
        .on_click(cx.listener(move |this, _, _, cx| this.select_tab(index, cx)))
        .on_drag(DragSidebarTab { side, index }, move |_, _, _, cx| {
            cx.new(|_| TabDragPreview {
                label: view.label().into(),
            })
        })
        .drag_over::<DragSidebarTab>(move |style, _, _, _| {
            style.border_l_2().border_color(accent)
        })
        .on_drop(
            cx.listener(move |this, drag: &DragSidebarTab, _, cx| this.drop_tab(drag, index, cx)),
        )
        .child(view.label())
        .into_any_element()
}

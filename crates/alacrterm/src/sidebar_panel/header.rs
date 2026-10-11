//! 侧边栏顶部的**面板标题栏**:当前视图的标题 + 右端「…」菜单(照 VS Code 的侧边栏标题栏)。
//!
//! 面板里不再排标签:显示哪个视图由**活动栏**点选(见 [`super::activity_bar`]),这一行只摆
//! 当前那个的标题。标题**可拖动** —— 拖到另一条侧边栏的标题栏上 = 把这个视图搬过去
//! (落点语义见 [`Sidebar::append_view`];视图清单与选中项在 [`SidebarViews`] 里)。
//! 「…」菜单把**可用的视图**都列出来、勾选当前那个,点一下就打开它([`Sidebar::open_view`]:
//! 本栏没有的视图会从另一栏搬过来)—— 右侧边栏没有活动栏,这是它切换视图的入口。

use gpui::{
    Anchor, AnyElement, AppContext as _, Context, CursorStyle, Entity, InteractiveElement as _,
    IntoElement, ParentElement as _, StatefulInteractiveElement as _, Styled as _, div,
};
use gpui_kit::component::{
    ActiveTheme as _, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    menu::{DropdownMenu as _, PopupMenuItem},
};

use super::shared::{DragPreview, ellipsis_label};
use super::{PANEL_HEADER_HEIGHT, Sidebar, SidebarSide, SidebarView};
use crate::assets::IconName;

/// 拖动中的侧边栏视图(拖放载荷:从哪一侧的第几个视图拖出来的)。
pub(super) struct DragSidebarView {
    /// 来自哪一侧。
    pub(super) side: SidebarSide,
    /// 在来源侧视图清单里的下标(按下标取,避免同名视图重定位)。
    pub(super) index: usize,
}

impl Sidebar {
    /// 本栏顶部的标题栏(挂在 `Sidebar` 的 `header` 上 ⇒ 固定顶部、不随内容滚动)。
    ///
    /// 返回 [`AnyElement`] 而非 `impl IntoElement`:edition 2024 下 `impl Trait` 会捕获
    /// `&mut Context` 的生命周期,同一渲染树里连续调用多个渲染方法会借用冲突。
    pub(super) fn render_header(&self, cx: &mut Context<Self>) -> AnyElement {
        let active = self.active_view();
        let title = active
            .and_then(|view| self.views.index_of(view).map(|index| (view, index)))
            .map(|(view, index)| view_title(self.side, view, index));

        h_flex()
            .id(format!("sidebar-header-{}", self.side.id()))
            .h(PANEL_HEADER_HEIGHT)
            .w_full()
            .items_center()
            .justify_between()
            .gap_2()
            .overflow_hidden()
            .rounded(cx.theme().radius)
            .text_xs()
            .text_color(cx.theme().sidebar_foreground)
            // 落点:从另一条侧边栏拖来的视图落在本栏(落下即选中它)。
            .on_drop(cx.listener(|this, drag: &DragSidebarView, _, cx| this.append_view(drag, cx)))
            .drag_over::<DragSidebarView>(|style, _, _, cx| style.bg(cx.theme().tokens.accent))
            .children(title)
            .child(view_menu(self.side, active, self.files_enabled, cx.entity()))
            .into_any_element()
    }
}

/// 标题:面板当前视图的名字。可拖动(抓手指针),拖到另一条侧边栏就换一栏。
fn view_title(side: SidebarSide, view: SidebarView, index: usize) -> AnyElement {
    div()
        .id(format!("sidebar-title-{}", side.id()))
        .min_w_0()
        .h_full()
        .flex()
        .items_center()
        .cursor(CursorStyle::OpenHand)
        .on_drag(DragSidebarView { side, index }, move |_, _, _, cx| {
            cx.new(|_| DragPreview {
                label: view.label().into(),
            })
        })
        .child(ellipsis_label(view.label().into()))
        .into_any_element()
}

/// 右端的「…」菜单:每个可用视图一项、勾选当前那个,点一下就打开它
/// (本栏没有的视图会从另一栏搬过来,见 [`Sidebar::open_view`])。
fn view_menu(
    side: SidebarSide,
    active: Option<SidebarView>,
    files_enabled: bool,
    sidebar: Entity<Sidebar>,
) -> AnyElement {
    Button::new(format!("sidebar-menu-{}", side.id()))
        .ghost()
        .xsmall()
        .icon(IconName::Ellipsis)
        .tooltip("视图")
        // 菜单右对齐弹出(VS Code 的「…」菜单也是贴在右端往左下展开)。
        .dropdown_menu_with_anchor(Anchor::TopRight, move |menu, _window, _cx| {
            SidebarView::ALL
                .iter()
                .copied()
                .filter(|view| view.visible_with(files_enabled))
                .fold(menu, |menu, view| {
                    let sidebar = sidebar.clone();
                    menu.item(
                        PopupMenuItem::new(view.label())
                            .checked(active == Some(view))
                            .on_click(move |_, _window, cx| {
                                let _ = sidebar.update(cx, |sidebar, cx| sidebar.open_view(view, cx));
                            }),
                    )
                })
        })
        .into_any_element()
}

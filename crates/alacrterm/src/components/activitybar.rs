//! **VS Code 风格的左侧整列**:48px 的「活动栏」(图标导航) + 它右侧的侧边栏面板。
//!
//! 侧边栏是活动栏的一个成员([`ActivityBar::sidebar`]):整列 = `h_flex[图标列, 侧边栏]`;
//! 图标列固定 48px、常驻,不给侧边栏时整列就只有那 48px。
//!
//! 一枚活动项 = 一个 48px 见方的图标按钮:
//! - 图标一律同色,当前项靠左端一条 2px 指示条(主题 `primary`)区分;
//! - 悬停 / 按下有底色(主题 `secondary_hover` / `secondary_active`);
//! - 悬停提示(右侧弹出)与无障碍名称取活动项的文字。
//!
//! 结构用 `base` 的 `v_flex`/`h_flex`/`div`;图标与悬停提示复用 `component` 的 `Icon`
//! 与 `Button::tooltip`。
//!
//! 组件不认识「视图」这类应用概念:哪个图标亮、点了做什么、侧边栏里摆什么由使用处决定。

use std::rc::Rc;

use gpui::{
    AnyElement, App, ClickEvent, ElementId, InteractiveElement as _, IntoElement,
    ParentElement as _, Pixels, RenderOnce, SharedString, Styled as _, Window, div,
    prelude::FluentBuilder as _, px,
};
use gpui_kit::base::{h_flex, v_flex};
use gpui_kit::component::{
    ActiveTheme as _, Icon, Placement, Sizable as _,
    button::{Button, ButtonCustomVariant, ButtonRounded, ButtonVariants as _},
};

use crate::assets::IconName;

/// 一枚活动项的点击回调。
type ClickHandler = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

/// 图标列宽度。
pub(crate) const ACTIVITY_BAR_WIDTH: Pixels = px(48.);
/// 一枚活动项的点击区尺寸(与图标列同宽)。
const ACTIVITY_ITEM_SIZE: Pixels = px(48.);
/// 图标按钮的名义尺寸:图标 = 名义尺寸 × 0.75。
const ACTIVITY_CONTROL_SIZE: Pixels = px(32.);
/// 当前项左端指示条的宽度。
const ACTIVITY_INDICATOR_WIDTH: Pixels = px(2.);

/// 活动栏上的一枚活动项:图标 + 文字 + 点击回调 + 是否高亮。
///
/// 文字不出现在栏上(图标态),只作悬停提示与无障碍名称。
#[derive(Clone)]
pub(crate) struct ActivityBarItem {
    id: SharedString,
    icon: IconName,
    tooltip: SharedString,
    active: bool,
    on_click: ClickHandler,
}

impl ActivityBarItem {
    /// 新建一枚活动项;`id` 是本栏内唯一的元素 id。
    pub(crate) fn new(
        id: impl Into<SharedString>,
        icon: IconName,
        tooltip: impl Into<SharedString>,
        on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        Self {
            id: id.into(),
            icon,
            tooltip: tooltip.into(),
            active: false,
            on_click: Rc::new(on_click),
        }
    }

    /// 是否为当前项。
    pub(crate) fn active(mut self, active: bool) -> Self {
        self.active = active;
        self
    }
}

/// VS Code 风格的左侧整列:一列固定宽度的图标按钮 + 它右侧的侧边栏面板。
///
/// [`ActivityBar::item`] 的项自上而下摆在图标列顶部,[`ActivityBar::footer_item`] 的项贴底,
/// [`ActivityBar::sidebar`] 是整列的第二块(不给就只有图标列)。
#[derive(IntoElement)]
pub(crate) struct ActivityBar {
    id: ElementId,
    items: Vec<ActivityBarItem>,
    footer_items: Vec<ActivityBarItem>,
    sidebar: Option<AnyElement>,
}

impl ActivityBar {
    pub(crate) fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            items: Vec::new(),
            footer_items: Vec::new(),
            sidebar: None,
        }
    }

    /// 在图标列顶部追加一枚活动项(按调用顺序自上而下)。
    pub(crate) fn item(mut self, item: ActivityBarItem) -> Self {
        self.items.push(item);
        self
    }

    /// 在图标列底部追加一枚活动项(贴底)。
    pub(crate) fn footer_item(mut self, item: ActivityBarItem) -> Self {
        self.footer_items.push(item);
        self
    }

    /// 挂上侧边栏面板:整列 = 图标列 + 侧边栏,宽度由侧边栏那侧吸收。
    pub(crate) fn sidebar(mut self, sidebar: impl IntoElement) -> Self {
        self.sidebar = Some(sidebar.into_any_element());
        self
    }
}

impl RenderOnce for ActivityBar {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let background = cx.theme().title_bar;
        let items = self.items.into_iter().map(|item| activity_button(item, cx));
        let footer = self
            .footer_items
            .into_iter()
            .map(|item| activity_button(item, cx));

        h_flex()
            .id(self.id)
            .h_full()
            // 无侧边栏 = 只有图标列那 48px;有侧边栏则铺满外层槽位。
            .when(self.sidebar.is_none(), |this| this.w(ACTIVITY_BAR_WIDTH))
            .when(self.sidebar.is_some(), |this| this.w_full())
            .child(
                v_flex()
                    .id("activity-bar-icons")
                    .flex_none()
                    .h_full()
                    .w(ACTIVITY_BAR_WIDTH)
                    .items_center()
                    // 顶部一组 + 底部一组(中间留白)。
                    .justify_between()
                    .bg(background)
                    // 图标列右缘的分界线。
                    .border_r_1()
                    .border_color(cx.theme().sidebar_border)
                    .child(v_flex().w_full().children(items))
                    .child(v_flex().w_full().children(footer)),
            )
            .when_some(self.sidebar, |this, sidebar| {
                this.child(div().flex_1().min_w_0().h_full().child(sidebar))
            })
    }
}

/// 一枚活动项:整格 48px 见方的图标按钮,当前项另加一条左端指示条。
///
/// 按钮本体撑到 48px(整格可点),名义尺寸给 32px 以得到 24px 的图标。
fn activity_button(item: ActivityBarItem, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let variant = ButtonCustomVariant::new(cx)
        .foreground(theme.foreground)
        .hover(theme.secondary_hover)
        .active(theme.secondary_active);

    let ActivityBarItem {
        id,
        icon,
        tooltip,
        active,
        on_click,
    } = item;

    div()
        .relative()
        .flex_none()
        .size(ACTIVITY_ITEM_SIZE)
        .child(
            Button::new(id)
                .custom(variant)
                .rounded(ButtonRounded::None)
                .with_size(ACTIVITY_CONTROL_SIZE)
                .size(ACTIVITY_ITEM_SIZE)
                .icon(Icon::new(icon))
                .tooltip(tooltip.clone())
                .tooltip_placement(Placement::Right)
                .accessibility_label(tooltip)
                .on_click(move |event, window, cx| on_click(event, window, cx)),
        )
        // 指示条后画,压在按钮底色之上。
        .when(active, |this| {
            this.child(
                div()
                    .absolute()
                    .left_0()
                    .top_0()
                    .bottom_0()
                    .w(ACTIVITY_INDICATOR_WIDTH)
                    .bg(theme.primary),
            )
        })
        .into_any_element()
}

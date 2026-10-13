//! **VS Code 风格的左侧整列**：48px 图标活动栏 + 它右侧的侧边栏面板。

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

pub(crate) const ACTIVITY_BAR_WIDTH: Pixels = px(48.);
const ACTIVITY_ITEM_SIZE: Pixels = px(48.);
const ACTIVITY_CONTROL_SIZE: Pixels = px(32.);
const ACTIVITY_INDICATOR_WIDTH: Pixels = px(2.);

/// 活动栏上的一枚活动项。
#[derive(Clone)]
pub(crate) struct ActivityBarItem {
    id: SharedString,
    icon: IconName,
    tooltip: SharedString,
    active: bool,
    on_click: ClickHandler,
}

impl ActivityBarItem {
    /// 新建一枚活动项。
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

    pub(crate) fn active(mut self, active: bool) -> Self {
        self.active = active;
        self
    }
}

/// 左侧整列:一列图标按钮 + 右侧的侧边栏面板。
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

    /// 在图标列顶部追加一枚活动项。
    pub(crate) fn item(mut self, item: ActivityBarItem) -> Self {
        self.items.push(item);
        self
    }

    /// 在图标列底部追加一枚活动项。
    pub(crate) fn footer_item(mut self, item: ActivityBarItem) -> Self {
        self.footer_items.push(item);
        self
    }

    /// 挂上右侧的侧边栏面板。
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
            // 侧边栏自己的 key context（默认 keymap 未使用，留给用户）。
            .key_context(crate::keymap::SIDEBAR_CONTEXT)
            .when(self.sidebar.is_none(), |this| this.w(ACTIVITY_BAR_WIDTH))
            .when(self.sidebar.is_some(), |this| this.w_full())
            .child(
                v_flex()
                    .id("activity-bar-icons")
                    .flex_none()
                    .h_full()
                    .w(ACTIVITY_BAR_WIDTH)
                    .items_center()
                    .justify_between()
                    .bg(background)
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

/// 一枚活动项:整格 48px 的图标按钮。
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

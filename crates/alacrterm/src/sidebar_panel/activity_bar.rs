//! Status bar entries are generated from each host's registered panels.
use crate::components::SidebarHost;
use crate::status_bar::{STATUS_BAR_ITEM_HEIGHT, new_status_bar};
use gpui::{AnyElement, App, Entity, IntoElement, ParentElement as _, Styled as _, div, px};
use gpui_kit::component::{
    ActiveTheme as _, Placement, Selectable as _, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
};

pub(crate) fn status_bar(
    left: &Entity<SidebarHost>,
    right: &Entity<SidebarHost>,
    monitor: Entity<crate::system_monitor::SystemMonitor>,
    cx: &mut App,
) -> AnyElement {
    let mut bar = new_status_bar().child(monitor);
    for (host, is_left) in [(left, true), (right, false)] {
        let panels = host.read(cx).panels(cx);
        if panels.is_empty() {
            continue;
        }
        // Submit a single region item so StatusBar cannot insert its default
        // 8px gap between the panel button groups.
        let mut group = h_flex().items_center().gap_0();
        for info in panels {
            let selected = host.read(cx).is_selected(info.id, cx);
            let state = host.downgrade();
            let button = Button::new(info.id.0)
                .ghost()
                .small()
                .h(STATUS_BAR_ITEM_HEIGHT)
                .w(STATUS_BAR_ITEM_HEIGHT)
                .icon(crate::assets::ui_icon(info.icon))
                .tooltip(info.tooltip)
                .tooltip_placement(Placement::Top)
                .accessibility_label(info.tooltip)
                .selected(selected)
                .on_click(move |_, _, cx| {
                    let _ = state.update(cx, |host, cx| host.toggle_view(info.id, cx));
                });
            let separator = div()
                .flex_none()
                .w(px(1.))
                .h(px(14.))
                .mx_1()
                .bg(cx.theme().border);
            group = if is_left {
                group.child(h_flex().items_center().child(button).child(separator))
            } else {
                group.child(h_flex().items_center().child(separator).child(button))
            };
        }
        bar = if is_left {
            bar.left(group)
        } else {
            bar.right(group)
        };
    }
    bar.into_any_element()
}

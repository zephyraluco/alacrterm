//! Business panels registered with the reusable SidebarHost.
use crate::components::PanelId;
use crate::{AppRoot, assets::IconName};
use gpui::{Context, ParentElement as _, Styled as _};
use gpui_kit::component::empty::{
    Empty, EmptyDescription, EmptyHeader, EmptyMedia, EmptyMediaVariant, EmptyTitle,
};
mod activity_bar;
pub(crate) mod chat;
pub(crate) mod files;
pub(crate) mod files_panel;
pub(crate) mod sessions;
pub(crate) mod sessions_panel;
mod shared;
pub(crate) use activity_bar::status_bar;

pub(super) fn empty_state(
    icon: IconName,
    title: &'static str,
    description: Option<&'static str>,
) -> Empty {
    let header = EmptyHeader::new()
        .media(
            EmptyMedia::new()
                .with_variant(EmptyMediaVariant::Icon)
                .child(crate::assets::ui_icon(icon)),
        )
        .title(EmptyTitle::new().child(title));
    let header = match description {
        Some(text) => header.description(EmptyDescription::new().child(text)),
        None => header,
    };

    Empty::new().flex_none().p_3().header(header)
}

impl AppRoot {
    pub(crate) fn toggle_sidebar(&mut self, cx: &mut Context<Self>) {
        self.sidebar.update(cx, |sidebar, cx| {
            sidebar.set_visible(!sidebar.visible(), cx)
        });
    }
    pub(crate) fn open_sidebar_view(&mut self, id: PanelId, cx: &mut Context<Self>) {
        if let Some(host) = [&self.sidebar, &self.right_sidebar]
            .into_iter()
            .find(|host| host.read(cx).contains(id))
        {
            host.update(cx, |host, cx| host.open_view(id, cx));
        }
    }
}

//! Terminal session business panel.
use super::{header::panel_header, sessions::SessionsState};
use crate::actions::{NewFolder, NewSession};
use crate::assets::IconName;
use crate::components::{PanelId, PanelInfo, SidebarList, SidebarPanel};
use gpui::{
    Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _, Render, Styled as _,
    Subscription, Window, div, px,
};
use gpui_kit::component::menu::ContextMenuExt as _;

pub(crate) const SESSIONS_PANEL: PanelId = PanelId("sessions");
pub(crate) struct SessionsPanel {
    sessions: Entity<SessionsState>,
    _subscription: Subscription,
}
impl SessionsPanel {
    pub(crate) fn new(sessions: Entity<SessionsState>, cx: &mut Context<Self>) -> Self {
        Self {
            sessions: sessions.clone(),
            _subscription: cx.observe(&sessions, |_, _, cx| cx.notify()),
        }
    }
}
impl SidebarPanel for SessionsPanel {
    const INFO: PanelInfo = PanelInfo {
        id: SESSIONS_PANEL,
        title: "会话",
        icon: IconName::SquareTerminal,
        tooltip: "终端会话",
        default_width: px(220.),
    };
}
impl Render for SessionsPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let rows = self
            .sessions
            .update(cx, |sessions, cx| sessions.sidebar_items(cx));
        div()
            .size_full()
            .key_context(crate::keymap::SIDEBAR_CONTEXT)
            .context_menu(|menu, _, _| {
                menu.menu_with_icon(
                    "新建会话",
                    IconName::Plus,
                    Box::new(NewSession { folder: None }),
                )
                .menu_with_icon(
                    "新建文件夹",
                    IconName::FolderPlus,
                    Box::new(NewFolder { parent: None }),
                )
            })
            .child(
                SidebarList::new("sidebar-sessions")
                    .header(panel_header(Self::INFO.title, cx))
                    .children(rows),
            )
    }
}

impl crate::components::SidebarRow for super::sessions::SessionsItem {
    fn render(self, cx: &mut gpui::App) -> gpui::AnyElement {
        super::sessions::SessionsItem::render(self, cx)
    }
}

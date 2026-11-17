//! Terminal session business panel.
use super::sessions::SessionsState;
use crate::actions::{NewFolder, NewSession};
use crate::assets::IconName;
use crate::components::{PanelId, PanelInfo, SidebarPanel};
use gpui::{
    Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _, Render, Styled as _,
    Subscription, Window, div, px,
};
use gpui::{ListAlignment, ListState, list, prelude::FluentBuilder as _};
use gpui_kit::base::Scrollbar;
use gpui_kit::component::menu::ContextMenuExt as _;
use gpui_kit::component::v_flex;
use std::rc::Rc;

pub(crate) const SESSIONS_PANEL: PanelId = PanelId("sessions");
pub(crate) struct SessionsPanel {
    sessions: Entity<SessionsState>,
    _subscription: Subscription,
    list_state: ListState,
}
impl SessionsPanel {
    pub(crate) fn new(sessions: Entity<SessionsState>, cx: &mut Context<Self>) -> Self {
        Self {
            sessions: sessions.clone(),
            list_state: ListState::new(0, ListAlignment::Top, px(400.)),
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
        let count = rows.len();
        if self.list_state.item_count() != count {
            self.list_state.reset(count);
        }
        let rows = Rc::new(rows);
        let state = self.list_state.clone();
        let content = div()
            .id("sessions-scroll")
            .relative()
            .flex_1()
            .min_h_0()
            .child(
                v_flex().size_full().px_3().child(
                    list(state.clone(), move |index, _, cx| {
                        div()
                            .id(index)
                            .when(index == 0, |row| row.pt_3())
                            .when(index + 1 == count, |row| row.pb_3())
                            .when_some(rows.get(index).cloned(), |row, item| {
                                row.child(item.render(cx))
                            })
                            .into_any_element()
                    })
                    .size_full(),
                ),
            )
            .child(
                div().absolute().inset_0().child(
                    Scrollbar::vertical(&state)
                        .id("sessions-scrollbar")
                        .viewport_from_layout(),
                ),
            );

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
                v_flex()
                    .id("sidebar-sessions")
                    .size_full()
                    .overflow_hidden()
                    .child(content),
            )
    }
}

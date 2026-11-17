//! Application UI primitives, independent of concrete sidebar business views.
mod sidebar_host;
mod sidebar_list;
pub(crate) use sidebar_host::{PanelId, PanelInfo, SidebarHost, SidebarPanel, SidebarSide};
pub(crate) use sidebar_list::{SidebarList, SidebarRow};

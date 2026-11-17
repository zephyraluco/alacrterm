//! Registered panel container. Business state and rendering belong to each panel.
use crate::assets::IconName;
use gpui::{
    AnyView, App, AppContext as _, Context, Entity, IntoElement, ParentElement as _, Pixels,
    Render, Styled as _, Subscription, Window, div, px,
};
use gpui_kit::component::resizable::ResizableState;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PanelId(pub &'static str);

#[derive(Clone)]
pub(crate) struct PanelInfo {
    pub id: PanelId,
    pub title: &'static str,
    pub icon: IconName,
    pub tooltip: &'static str,
    pub default_width: Pixels,
}

/// Implemented by business views; the host erases their concrete entity types.
pub(crate) trait SidebarPanel: Render + Sized + 'static {
    const INFO: PanelInfo;
    fn available(&self, _: &App) -> bool {
        true
    }
    fn on_open(&mut self, _: &mut Context<Self>) {}
}

#[derive(Clone, Copy)]
pub(crate) enum SidebarSide {
    Left,
    Right,
}
impl SidebarSide {
    fn panel_index(self) -> usize {
        match self {
            Self::Left => 0,
            Self::Right => 1,
        }
    }
}

struct PanelEntry {
    info: PanelInfo,
    view: AnyView,
    width: Pixels,
    available: Box<dyn Fn(&App) -> bool>,
    open: Box<dyn Fn(&mut App)>,
    _subscription: Subscription,
}

#[derive(Default)]
struct Selection {
    active: Option<PanelId>,
    visible: bool,
}
impl Selection {
    fn open(&mut self, id: PanelId) {
        self.active = Some(id);
        self.visible = true;
    }
    fn toggle(&mut self, id: PanelId) -> bool {
        if self.visible && self.active == Some(id) {
            self.visible = false;
            false
        } else {
            self.open(id);
            true
        }
    }
}

const COLLAPSE_WIDTH: Pixels = px(24.);
fn width_after_drag(width: Pixels) -> Option<Pixels> {
    (width > COLLAPSE_WIDTH).then_some(width)
}

pub(crate) struct SidebarHost {
    side: SidebarSide,
    panels: Vec<PanelEntry>,
    selection: Selection,
    resize: Entity<ResizableState>,
    split_width: Option<Pixels>,
}
impl SidebarHost {
    pub(crate) fn new(side: SidebarSide, cx: &mut Context<Self>) -> Self {
        Self {
            side,
            panels: Vec::new(),
            selection: Selection::default(),
            resize: cx.new(|_| ResizableState::default()),
            split_width: None,
        }
    }

    pub(crate) fn register<T: SidebarPanel>(&mut self, panel: Entity<T>, cx: &mut Context<Self>) {
        assert!(!self.contains(T::INFO.id), "duplicate sidebar panel id");
        let availability = panel.clone();
        let activation = panel.clone();
        self.panels.push(PanelEntry {
            info: T::INFO,
            width: T::INFO.default_width,
            view: panel.clone().into(),
            available: Box::new(move |cx| availability.read(cx).available(cx)),
            open: Box::new(move |cx| activation.update(cx, |panel, cx| panel.on_open(cx))),
            _subscription: cx.observe(&panel, |_, _, cx| cx.notify()),
        });
        if self.selection.active.is_none() {
            self.selection.active = Some(T::INFO.id);
        }
        cx.notify();
    }

    pub(crate) fn contains(&self, id: PanelId) -> bool {
        self.panels.iter().any(|panel| panel.info.id == id)
    }
    pub(crate) fn panels(&self, cx: &App) -> Vec<PanelInfo> {
        self.panels
            .iter()
            .filter(|panel| (panel.available)(cx))
            .map(|panel| panel.info.clone())
            .collect()
    }
    fn active(&self) -> Option<&PanelEntry> {
        self.panels
            .iter()
            .find(|panel| Some(panel.info.id) == self.selection.active)
    }
    pub(crate) fn panel_visible(&self, cx: &App) -> bool {
        self.selection.visible && self.active().is_some_and(|panel| (panel.available)(cx))
    }
    pub(crate) fn is_selected(&self, id: PanelId, cx: &App) -> bool {
        self.panel_visible(cx) && self.selection.active == Some(id)
    }
    pub(crate) fn visible(&self) -> bool {
        self.selection.visible
    }
    pub(crate) fn set_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        if self.selection.visible != visible {
            self.selection.visible = visible;
            self.split_width = None;
            cx.notify();
        }
    }
    pub(crate) fn open_view(&mut self, id: PanelId, cx: &mut Context<Self>) {
        let Some(panel) = self
            .panels
            .iter()
            .find(|panel| panel.info.id == id && (panel.available)(cx))
        else {
            return;
        };
        (panel.open)(cx);
        self.selection.open(id);
        self.split_width = None;
        cx.notify();
    }
    pub(crate) fn toggle_view(&mut self, id: PanelId, cx: &mut Context<Self>) {
        if !self
            .panels
            .iter()
            .any(|panel| panel.info.id == id && (panel.available)(cx))
        {
            return;
        }
        if self.selection.toggle(id) {
            self.open_view(id, cx);
        } else {
            self.split_width = None;
            cx.notify();
        }
    }
    pub(crate) fn width(&self) -> Pixels {
        self.active().map_or(px(220.), |panel| panel.width)
    }
    pub(crate) fn set_panel_width(&mut self, width: Pixels, cx: &mut Context<Self>) {
        let Some(width) = width_after_drag(width) else {
            self.set_visible(false, cx);
            return;
        };
        if let Some(panel) = self
            .panels
            .iter_mut()
            .find(|panel| Some(panel.info.id) == self.selection.active)
        {
            panel.width = width;
        }
    }
    pub(crate) fn resize_state(&self) -> &Entity<ResizableState> {
        &self.resize
    }
    pub(crate) fn pin_width(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.panel_visible(cx) {
            return;
        }
        let container = self.resize.read(cx).container_size();
        if self.split_width != Some(container) {
            self.split_width = Some(container);
            let width = self.width();
            let index = self.side.panel_index();
            self.resize
                .update(cx, |state, cx| state.resize_panel(index, width, window, cx));
        }
    }
}
impl Render for SidebarHost {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .min_w_0()
            .min_h_0()
            .overflow_hidden()
            .children(
                self.panel_visible(cx)
                    .then(|| self.active().unwrap().view.clone()),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn toggling_and_switching_panels_preserves_selection_when_collapsed() {
        let first = PanelId("first");
        let second = PanelId("second");
        let mut state = Selection::default();
        assert!(state.toggle(first));
        assert!(!state.toggle(first));
        assert_eq!(state.active, Some(first));
        assert!(!state.visible);
        assert!(state.toggle(second));
        assert_eq!(state.active, Some(second));
        assert!(state.visible);
    }
    #[test]
    fn resizing_collapses_at_edge_and_side_determines_split_index() {
        assert_eq!(width_after_drag(px(24.)), None);
        assert_eq!(width_after_drag(px(-1.)), None);
        assert_eq!(width_after_drag(px(25.)), Some(px(25.)));
        assert_eq!(width_after_drag(px(2000.)), Some(px(2000.)));
        assert_eq!(SidebarSide::Left.panel_index(), 0);
        assert_eq!(SidebarSide::Right.panel_index(), 1);
    }
}

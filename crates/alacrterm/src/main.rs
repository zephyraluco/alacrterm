mod assets;
mod themes;

use crate::assets::Assets;
use crate::terminal_view::TerminalView;
use crate::themes::set_theme;
use gpui::*;
use gpui_component::{Root, TitleBar, h_flex, v_flex};

pub struct TerminalApp {
    terminal_view: Entity<TerminalView>,
}

impl TerminalApp {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let terminal_view = cx.new(|cx| TerminalView::new(window, cx));
        Self { terminal_view }
    }
}

impl Render for TerminalApp {
    fn render(&mut self, _: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .size_full()
            .child(
                TitleBar::new().child(
                    h_flex()
                        .w_full()
                        .pr_2()
                        .justify_between()
                        .child("Alacrterm - Terminal Emulator")
                        .child(""),
                ),
            )
            .child(
                div()
                    .id("terminal-container")
                    .flex_grow()
                    .min_h_0()
                    .w_full()
                    .overflow_hidden()
                    .child(self.terminal_view.clone()),
            )
    }
}

fn main() {
    let app = gpui_platform::application().with_assets(Assets);

    app.run(move |cx| {
        gpui_component::init(cx);
        set_theme(cx, "Tokyo Night");

        cx.spawn(async move |cx| {
            let window_options = WindowOptions {
                titlebar: Some(TitleBar::title_bar_options()),
                ..Default::default()
            };

            cx.open_window(window_options, |window, cx| {
                let view = cx.new(|cx| TerminalApp::new(window, cx));
                cx.new(|cx| Root::new(view, window, cx))
            })?;

            Ok::<_, anyhow::Error>(())
        })
        .detach();
    });
}

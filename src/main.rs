mod assets;
mod themes;
mod terminal_view;

use crate::assets::Assets;
use crate::themes::set_theme;
use crate::terminal_view::TerminalView;
use gpui::*;
use gpui_component::{Root, TitleBar, h_flex, v_flex};

pub struct TerminalApp;

impl Render for TerminalApp {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let terminal_view = cx.new(|cx| TerminalView::new(cx));

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
                    .size_full()
                    .child(terminal_view)
            )
    }
}

fn main() {
    let app = Application::new().with_assets(Assets);

    app.run(move |cx| {
        gpui_component::init(cx);
        set_theme(cx, "Tokyo Night");

        cx.spawn(async move |cx| {
            let window_options = WindowOptions {
                titlebar: Some(TitleBar::title_bar_options()),
                ..Default::default()
            };

            cx.open_window(window_options, |window, cx| {
                let view = cx.new(|_| TerminalApp);
                cx.new(|cx| Root::new(view, window, cx))
            })?;

            Ok::<_, anyhow::Error>(())
        })
        .detach();
    });
}

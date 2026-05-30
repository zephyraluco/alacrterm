mod assets;
mod layout;
mod terminal_element;
mod terminal_view;
mod themes;

use crate::assets::Assets;
use crate::layout::TerminalApp;
use crate::themes::set_theme;
use gpui::*;
use gpui_component::{Root, TitleBar};

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
                let view = cx.new(|cx| TerminalApp::new(cx));
                cx.new(|cx| Root::new(view, window, cx))
            })?;

            Ok::<_, anyhow::Error>(())
        })
        .detach();
    });
}

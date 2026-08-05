mod assets;

use gpui::{App, AppContext as _, Bounds, WindowBounds, WindowOptions, px, size};
use terminal_view::TerminalView;
use util::shell::Shell;

fn main() {
    gpui_platform::application()
        .with_assets(assets::Assets)
        .run(|cx: &mut App| {
            let bounds = Bounds::centered(None, size(px(900.), px(600.)), cx);
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    ..Default::default()
                },
                |window, cx| cx.new(|cx| TerminalView::new(None, Shell::System, window, cx)),
            )
            .expect("failed to open window");

            cx.on_window_closed(|cx, _| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();

            cx.activate(true);
        });
}
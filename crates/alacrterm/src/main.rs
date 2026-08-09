mod assets;

use gpui::{
    App, AppContext as _, Bounds, Context, Entity, IntoElement, ParentElement, Render,
    SharedString, Styled, Window, WindowBounds, WindowOptions, div, px, size,
};
use gpui_component::{
    ActiveTheme as _, Icon, Sizable as _, StyledExt as _, Theme, ThemeMode, TitleBar, h_flex,
};
use terminal_view::TerminalView;
use util::shell::Shell;

use crate::assets::IconName;

/// 应用根视图：自定义标题栏（gpui_component::TitleBar）+ 终端。
struct AppRoot {
    terminal_view: Entity<TerminalView>,
    title: SharedString,
}

impl AppRoot {
    fn new(terminal_view: Entity<TerminalView>, cx: &mut Context<Self>) -> Self {
        let title = terminal_view.read_with(cx, |view, _| view.title());

        // 终端标题变化（OSC 0 等）时同步更新标题栏。
        cx.observe(&terminal_view, |this, terminal_view, cx| {
            let title = terminal_view.read_with(cx, |view, _| view.title());
            if title != this.title {
                this.title = title;
                cx.notify();
            }
        })
        .detach();

        Self {
            terminal_view,
            title,
        }
    }
}

impl Render for AppRoot {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .v_flex()
            .size_full()
            .child(
                TitleBar::new().child(
                    h_flex()
                        .w_full()
                        .px(px(8.))
                        .gap(px(8.))
                        .items_center()
                        .child(Icon::new(IconName::SquareTerminal).small())
                        .child(
                            div()
                                .text_size(px(13.))
                                .text_color(cx.theme().secondary_foreground)
                                .child(self.title.clone()),
                        ),
                ),
            )
            .child(self.terminal_view.clone())
    }
}

fn main() {
    gpui_platform::application()
        .with_assets(assets::Assets)
        .run(|cx: &mut App| {
            gpui_component::init(cx);
            // 终端为深色背景，标题栏跟随使用暗色主题。
            Theme::change(ThemeMode::Dark, None, cx);

            let bounds = Bounds::centered(None, size(px(900.), px(600.)), cx);
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    // 隐藏系统标题栏（macOS/Windows），改用 gpui_component 自绘标题栏。
                    titlebar: Some(TitleBar::title_bar_options()),
                    #[cfg(target_os = "linux")]
                    window_decorations: Some(gpui::WindowDecorations::Client),
                    ..Default::default()
                },
                |window, cx| {
                    let terminal_view =
                        cx.new(|cx| TerminalView::new(None, Shell::System, window, cx));
                    cx.new(|cx| AppRoot::new(terminal_view, cx))
                },
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
mod assets;
mod themes;

use std::{collections::HashMap, fs, path::PathBuf};

use crate::assets::Assets;
use crate::themes::set_theme;
use gpui::*;
use gpui_component::{Root, TitleBar, h_flex, v_flex};
use settings::{Settings, SettingsStore};
use terminal::{TerminalBuilder, terminal_settings::TerminalSettings};
use terminal_view::TerminalView;
use util::{paths::PathStyle, shell::Shell};

pub struct TerminalApp {
    terminal_view: Option<Entity<TerminalView>>,
    error: Option<String>,
}

impl TerminalApp {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let terminal_settings = TerminalSettings::get_global(cx).clone();
        let builder = TerminalBuilder::new(
            std::env::current_dir().ok(),
            Shell::System,
            HashMap::default(),
            terminal_settings.cursor_shape,
            terminal_settings.alternate_scroll,
            terminal_settings.max_scroll_history_lines,
            terminal_settings.path_hyperlink_regexes,
            terminal_settings.path_hyperlink_timeout_ms,
            false,
            0,
            None,
            cx,
            Vec::new(),
            PathStyle::local(),
        );

        cx.spawn_in(window, async move |app, cx| {
            match builder.await {
                Ok(builder) => {
                    app.update_in(cx, |app, window, cx| {
                        let terminal = cx.new(|cx| builder.subscribe(cx));
                        let terminal_view = cx.new(|cx| TerminalView::new(terminal, window, cx));
                        app.terminal_view = Some(terminal_view);
                        app.error = None;
                        cx.notify();
                    })?;
                }
                Err(error) => {
                    app.update(cx, |app, cx| {
                        app.error = Some(error.to_string());
                        cx.notify();
                    })?;
                }
            }

            anyhow::Ok(())
        })
        .detach_and_log_err(cx);

        Self {
            terminal_view: None,
            error: None,
        }
    }
}

impl Render for TerminalApp {
    fn render(&mut self, _: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let content = if let Some(terminal_view) = &self.terminal_view {
            terminal_view.clone().into_any_element()
        } else if let Some(error) = &self.error {
            div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .child(format!("Failed to start terminal: {error}"))
                .into_any_element()
        } else {
            div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .child("Starting terminal...")
                .into_any_element()
        };

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
                    .child(content),
            )
    }
}

fn main() {
    let app = gpui_platform::application().with_assets(Assets);

    app.run(move |cx| {
        gpui_component::init(cx);
        theme::init(theme::LoadThemes::JustBase, cx);
        terminal_view::init(cx);
        let mut store = SettingsStore::new().expect("failed to initialize settings store");
        let path = std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join("settings.json");
        if !path.exists() {
            fs::write(&path, "{}").expect("failed to create default settings.json");
        }
        store
            .load_user_settings(path)
            .expect("failed to load user settings");
        cx.set_global(store);
        let store = cx.global::<SettingsStore>();

        println!("Default settings loaded: {:#?}", store.global_settings());
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

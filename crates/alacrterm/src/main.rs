mod assets;
mod themes;

use std::{fs, path::PathBuf};

use crate::assets::Assets;
use crate::themes::set_theme;
use gpui::*;
use gpui_component::{Root, TitleBar, h_flex, v_flex};
use settings::SettingsStore;

pub struct TerminalApp;

impl TerminalApp {
    pub fn new(_: &mut Window, _: &mut Context<Self>) -> Self {
        Self
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
                    .flex()
                    .items_center()
                    .justify_center()
                    .child("Alacrterm"),
            )
    }
}

fn main() {
    let app = gpui_platform::application().with_assets(Assets);

    app.run(move |cx| {
        gpui_component::init(cx);
        let mut store = SettingsStore::new().expect("failed to initialize settings store");
        let path = std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join("settings.json");
        if !path.exists() {
            fs::write(&path, "{}")
                .expect("failed to create default settings.json");
        }
        store.load_user_settings(path).expect("failed to load user settings");
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

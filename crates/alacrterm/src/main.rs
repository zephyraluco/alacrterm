#![cfg_attr(
    all(target_os = "windows", not(debug_assertions)),
    windows_subsystem = "windows"
)]

//! 二进制入口：应用实现都在库 `alacrterm` 里（见 `lib.rs`），这里保留原有的启动流程，
//! 只把模块声明换成对库的引用。

use alacrterm::AppRoot;
use alacrterm::assets;
use alacrterm::config;
use alacrterm::keymap;
use alacrterm::preview_cache;
use alacrterm::updates;

#[cfg(target_os = "macos")]
use alacrterm::exit_coordinator;

use gpui::{App, AppContext as _, Bounds, WindowBounds, WindowOptions, px, size};
use gpui_kit::{
    QuitMode,
    component::{Root, ThemeMode, TitleBar},
};

/// 启动应用事件循环。
fn main() {
    let logging = std::sync::Arc::new(std::sync::Mutex::new(Some(util::logging::init(
        "alacrterm",
    ))));
    let quit_logging = logging.clone();
    #[cfg(windows)]
    platform::initialize();

    gpui_kit::application()
        .with_assets(assets::Assets)
        .with_quit_mode(QuitMode::LastWindowClosed)
        .run(move |cx: &mut App| {
            cx.on_app_quit(move |_| {
                platform::shutdown_wayx();
                let guard = quit_logging
                    .lock()
                    .ok()
                    .and_then(|mut logging| logging.take());
                // AppKit may exit without returning from the event loop. Flush after
                // GPUI releases its windows, while normal Rust main teardown remains a fallback.
                async move {
                    drop(guard);
                }
            })
            .detach();
            gpui_kit::init(cx);
            config::install(cx);
            updates::install(cx);
            config::load_themes(cx);
            config::apply_saved_themes(cx);
            config::change_theme(ThemeMode::Dark, cx);
            // 上次跑剩下的预览缓存(交给系统程序打开的那些副本)现在没人用了,清掉。
            // ⚠️ 必须在任何窗口打开之前。
            preview_cache::PreviewCache::install(cx);

            let bounds = Bounds::centered(None, size(px(1100.), px(700.)), cx);
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    app_id: Some("alacrterm".into()),
                    ..TitleBar::window_options()
                },
                |window, cx| {
                    let root = cx.new(|cx| AppRoot::new(window, cx));
                    #[cfg(target_os = "macos")]
                    exit_coordinator::attach_native(root.downgrade(), window, cx);
                    #[cfg(target_os = "macos")]
                    updates::attach_sparkle_guard(root.downgrade(), window, cx);
                    root.update(cx, |root, cx| root.sync_active_context(window, cx));
                    let close_guard = root.downgrade();
                    window.on_window_should_close(cx, move |window, cx| {
                        AppRoot::should_close(&close_guard, window, cx)
                    });
                    AppRoot::register_actions(root.downgrade(), cx);
                    // 键位全部来自 keymap JSON；必须在 gpui_kit::init 之后注册
                    // （它自己的 Root / Dialog 绑定不能被清掉），见 [`keymap`]。
                    keymap::install(cx);
                    cx.new(|cx| Root::new(root, window, cx))
                },
            )
            .expect("failed to open window");
        });
    platform::shutdown_wayx();
}

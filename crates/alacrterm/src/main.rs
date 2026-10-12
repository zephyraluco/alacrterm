//! 程序入口与根视图 [`AppRoot`]：标题栏 + 侧边栏 + 中间终端区 + 状态栏。
//! 外壳分模块（`components` / `sidebar_panel` / `terminal_panel` 等），终端核心在 `crates/terminal`。

mod actions;
mod assets;
mod components;
mod config;
#[cfg(windows)]
mod conpty_backend;
mod dialog;
mod keymap;
mod settings_window;
mod sidebar_panel;
mod status_bar;
mod tab_bar;
mod terminal_panel;
mod welcome;

use components::ACTIVITY_BAR_WIDTH;
use gpui::{
    App, AppContext as _, AsyncApp, Bounds, Context, Entity, FocusHandle, InteractiveElement as _,
    IntoElement, MouseButton, MouseDownEvent, ParentElement as _, Pixels, Render, Styled as _,
    Subscription, WeakEntity, Window, WindowBounds, WindowHandle, WindowOptions, div, px, size,
};
use gpui_kit::{
    QuitMode,
    component::{
        ActiveTheme as _, Root, ThemeMode, TitleBar,
        dock::{DockArea, DockEvent, TabGroup},
        h_flex,
        resizable::{h_resizable, resizable_panel},
        v_flex,
    },
};
use sidebar_panel::files::FilesState;
use sidebar_panel::sessions::SessionsState;
use sidebar_panel::Sidebar;
use tab_bar::TerminalDockSkin;
use terminal_panel::Session;

fn main() {
    #[cfg(windows)]
    conpty_backend::ensure();

    gpui_kit::application()
        .with_assets(assets::Assets)
        .with_quit_mode(QuitMode::LastWindowClosed)
        .run(|cx: &mut App| {
            gpui_kit::init(cx);
            config::install(cx);
            config::load_themes(cx);
            config::apply_saved_themes(cx);
            config::change_theme(ThemeMode::Dark, cx);

            let bounds = Bounds::centered(None, size(px(1100.), px(700.)), cx);
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    ..TitleBar::window_options()
                },
                |window, cx| {
                    let root = cx.new(|cx| AppRoot::new(window, cx));
                    AppRoot::register_actions(root.downgrade(), cx);
                    // 键位全部来自 keymap JSON；必须在 gpui_kit::init 之后注册
                    // （它自己的 Root / Dialog 绑定不能被清掉），见 [`keymap`]。
                    keymap::install(cx);
                    cx.new(|cx| Root::new(root, window, cx))
                },
            )
            .expect("failed to open window");
        });
}

/// 应用根视图（只存共享状态）。
struct AppRoot {
    /// 所有终端会话。
    terminals: Vec<Session>,
    /// 当前会话下标。
    active: usize,
    /// 侧边栏。
    sidebar: Entity<Sidebar>,
    /// 「会话」视图状态。
    sessions: Entity<SessionsState>,
    /// 「文件管理器」视图状态。
    files: Entity<FilesState>,
    /// 终端会话的 dock。
    dock: Entity<DockArea>,
    /// dock 布局变化订阅。
    dock_layout_sub: Option<Subscription>,
    /// 下一个新建会话的标签组（用一次即清空）。
    pending_session_group: Option<WeakEntity<TabGroup>>,
    /// 设置窗口句柄。
    settings_window: Option<WindowHandle<Root>>,
    /// 「背景」焦点。
    background_focus: FocusHandle,
}

impl AppRoot {
    /// 点击终端之外时把焦点从终端拿走。
    fn on_background_mouse_down(
        &mut self,
        _: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let terminal_focused = self
            .terminals
            .iter()
            .any(|session| session.view.read(cx).is_focused(window));
        if terminal_focused {
            window.focus(&self.background_focus, cx);
        }
    }

    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let dock = Self::build_dock(window, cx);
        let sessions = cx.new(|_| SessionsState::new());
        let files = cx.new(|cx| FilesState::new(window, cx));
        let sidebar = cx.new(|cx| Sidebar::new(sessions.clone(), files.clone(), cx));

        let mut this = Self {
            terminals: Vec::new(),
            active: 0,
            sidebar,
            sessions,
            files,
            dock,
            dock_layout_sub: None,
            pending_session_group: None,
            settings_window: None,
            background_focus: cx.focus_handle(),
        };
        this.dock_layout_sub = Some(cx.subscribe_in(
            &this.dock,
            window,
            |root, _, event, _window, cx| {
                if matches!(event, DockEvent::LayoutChanged) {
                    root.sync_sessions_with_dock(cx);
                }
            },
        ));
        // 启动时不建会话（显示欢迎页）。
        this
    }

    /// 建立终端会话的 dock。
    fn build_dock(window: &mut Window, cx: &mut Context<Self>) -> Entity<DockArea> {
        let root = cx.weak_entity();
        let dock = cx.new(|cx| {
            let skin = TerminalDockSkin::new(cx, root);
            DockArea::new("terminal-dock", None, window, cx).with_renderer(skin)
        });

        dock.update(cx, |area, cx| {
            area.set_locked(false, window, cx);
        });
        dock
    }

    /// 记下下一个新建会话该进的标签组。
    pub(crate) fn set_pending_session_group(&mut self, group: Option<WeakEntity<TabGroup>>) {
        self.pending_session_group = group;
    }

    /// 把需要窗口的操作推迟到本次窗口更新之后执行。
    pub(crate) fn defer_after_update(
        root: WeakEntity<Self>,
        cx: &mut App,
        action: impl FnOnce(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) {
        cx.spawn(async move |cx: &mut AsyncApp| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(1))
                .await;
            let _ = root.update_in(cx, |this, window, cx| action(this, window, cx));
        })
        .detach();
    }
}

impl Render for AppRoot {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // 主体：侧边栏 | 中间列。

        // 「当前会话」以键盘焦点为准。
        if let Some(index) = self
            .terminals
            .iter()
            .position(|session| session.view.read(cx).is_focused(window))
        {
            self.active = index;
        }

        // 「文件管理器」只服务远端会话。
        let active = self.terminals.get(self.active);
        let remote = active.is_some_and(|session| session.is_remote(cx));
        // 当前会话的 SFTP 句柄；未连上返回 `None`。
        let fs = if remote {
            active
                .filter(|session| session.view.read(cx).is_connected(cx))
                .and_then(|session| session.view.read(cx).remote_fs(cx))
        } else {
            None
        };
        self.files
            .update(cx, |files, cx| files.sync(fs, window, cx));
        self.sidebar
            .update(cx, |sidebar, cx| sidebar.set_files_enabled(remote, cx));

        // 侧边栏宽度每帧钉回用户拖拽值。
        self.sidebar
            .update(cx, |sidebar, cx| sidebar.pin_width(window, cx));

        // 中间列：终端区（dock 或欢迎页）。
        let terminal_area = if self.terminals.is_empty() {
            self.render_welcome(cx)
        } else {
            self.dock.clone().into_any_element()
        };
        let mid_column = div()
            .h_full()
            .w_full()
            .overflow_hidden()
            .child(terminal_area)
            .into_any_element();

        // 分栏组：最左整列 | 中间列。
        let visible = self.sidebar.read(cx).visible();
        let width = self.sidebar.read(cx).width();
        let resize = self.sidebar.read(cx).resize_state().clone();
        let sidebar_state = self.sidebar.downgrade();
        let panel = self.sidebar.clone();
        let body = if visible {
            h_resizable("main-split")
                .with_state(&resize)
                // 拖拽结束后记下整列宽度。
                .on_resize(move |state, _window, cx| {
                    if let Some(width) = state.read(cx).sizes().first().copied() {
                        let _ = sidebar_state
                            .update(cx, |sidebar, cx| sidebar.set_panel_width(width, cx));
                    }
                })
                .child(
                    resizable_panel()
                        .size(ACTIVITY_BAR_WIDTH + width)
                        // 下界是图标栏，上界不限。
                        .size_range(ACTIVITY_BAR_WIDTH..Pixels::MAX)
                        .flex_none()
                        .child(panel),
                )
                .child(resizable_panel().child(mid_column))
                .into_any_element()
        } else {
            // 折叠态：中间列铺满剩余宽度。
            h_flex()
                .h_full()
                .w_full()
                .child(panel)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .overflow_hidden()
                        .child(mid_column),
                )
                .into_any_element()
        };

        v_flex()
            .id("app-root")
            .size_full()
            .track_focus(&self.background_focus)
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_background_mouse_down))
            .bg(cx.theme().background)
            // 顶部：自绘标题栏。
            .child(
                TitleBar::new().child(
                    h_flex()
                        .w_full()
                        .px(px(8.))
                        .items_center()
                        .child(
                            div()
                                .flex_1()
                                .overflow_hidden()
                                .text_size(px(13.))
                                .text_color(cx.theme().secondary_foreground)
                                .child("Alacrterm"),
                        ),
                ),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .overflow_hidden()
                    .child(body),
            )
            .child(status_bar::empty_status_bar())
            // 覆盖层由 gpui-kit 的 `Root` 渲染。
    }
}

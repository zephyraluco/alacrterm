//! 程序入口与根视图 [`AppRoot`]：标题栏、侧边栏与终端主体、全局底部状态栏。
//! 外壳分模块（`components` / `sidebar_panel` / `terminal_panel` 等），终端核心在 `crates/terminal`。

mod actions;
mod assets;
mod components;
mod completion;
mod config;
mod dialog;
use util::format;
mod keymap;
mod pane_layout;
mod preview_cache;
mod preview_window;
mod resource_registry;
mod settings_window;
mod shell_integration;
mod sidebar_panel;
mod status_bar;
mod tab_bar;
mod terminal_panel;
mod transfer_manager;
mod welcome;
mod wayx_control;
mod x11;

use gpui::{
    App, AppContext as _, AsyncApp, Bounds, Context, Entity, FocusHandle, InteractiveElement as _,
    IntoElement, MouseButton, MouseDownEvent, ParentElement as _, Pixels, Render, Styled as _,
    Subscription, WeakEntity, Window, WindowBounds, WindowHandle, WindowOptions, div,
    prelude::FluentBuilder as _, px, size,
};
use gpui_kit::{
    QuitMode,
    component::{
        ActiveTheme as _, Root, ThemeMode, TitleBar, WindowExt as _,
        button::{Button, ButtonVariants as _},
        dialog::DialogFooter,
        dock::{DockArea, DockEvent, TabGroup},
        h_flex,
        notification::Notification,
        resizable::{h_resizable, resizable_panel},
        v_flex,
    },
};
use sidebar_panel::files::FilesState;
use sidebar_panel::sessions::SessionsState;
use sidebar_panel::{Sidebar, SidebarView};
use tab_bar::{TerminalDockSkin, TerminalTabBar};
use terminal_panel::TabPane;
use transfer_manager::{TransferDone, TransferManager};

fn close_after_transfers(active: usize, waiting: bool) -> bool {
    active == 0 && waiting
}

/// 这次同步需要真的应用吗。
///
/// 快照没变、又没有「必须拿窗口才能做」的待办时跳过 —— 事件可能因为同一帧内的多次
/// 重绘而重复触发。
fn needs_context_apply(
    last: Option<&ActiveContext>,
    next: &ActiveContext,
    pending_window_work: bool,
) -> bool {
    pending_window_work || last != Some(next)
}

#[cfg(test)]
mod close_tests {
    use super::{ActiveContext, close_after_transfers, needs_context_apply};

    fn context(title: &str, directory: Option<&str>) -> ActiveContext {
        ActiveContext {
            tab: None,
            pane: None,
            session: None,
            connected: false,
            remote: false,
            directory: directory.map(str::to_string),
            title: title.to_string(),
        }
    }

    #[test]
    fn exits_only_after_pending_transfers_finish() {
        assert!(!close_after_transfers(2, true));
        assert!(!close_after_transfers(0, false));
        assert!(close_after_transfers(0, true));
    }

    #[test]
    fn identical_context_is_not_reapplied() {
        let current = context("Alacrterm", None);
        assert!(!needs_context_apply(Some(&current), &current.clone(), false));
    }

    #[test]
    fn directory_or_title_change_reapplies() {
        let current = context("Alacrterm", Some("/home"));
        assert!(needs_context_apply(
            Some(&current),
            &context("Alacrterm", Some("/home/user")),
            false
        ));
        assert!(needs_context_apply(
            Some(&current),
            &context("other", Some("/home")),
            false
        ));
    }

    #[test]
    fn pending_window_work_forces_one_apply() {
        let current = context("Alacrterm", None);
        assert!(needs_context_apply(Some(&current), &current.clone(), true));
        assert!(needs_context_apply(None, &current, false));
    }
}

fn main() {
    #[cfg(windows)]
    platform::initialize();

    gpui_kit::application()
        .with_assets(assets::Assets)
        .with_quit_mode(QuitMode::LastWindowClosed)
        .run(|cx: &mut App| {
            cx.on_app_quit(|_| {
                platform::shutdown_wayx();
                async {}
            })
            .detach();
            gpui_kit::init(cx);
            config::install(cx);
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
                    ..TitleBar::window_options()
                },
                |window, cx| {
                    let root = cx.new(|cx| AppRoot::new(window, cx));
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

/// 应用根视图（只存共享状态）。
struct AppRoot {
    /// 所有标签页（每页内部自己分屏）。
    tabs: Vec<Entity<TabPane>>,
    /// 当前标签页下标。
    active: usize,
    /// 侧边栏。
    sidebar: Entity<Sidebar>,
    _sidebar_sub: Subscription,
    right_sidebar: Entity<Sidebar>,
    _right_sidebar_sub: Subscription,
    /// 「会话」视图状态。
    sessions: Entity<SessionsState>,
    /// 「文件管理器」视图状态。
    files: Entity<FilesState>,
    _transfers: Entity<TransferManager>,
    _transfer_sub: Subscription,
    _transfer_completion_sub: Subscription,
    close_when_transfers_finish: bool,
    /// 终端会话的 dock。
    dock: Entity<DockArea>,
    _dock_sub: Subscription,
    zoom_tab_bar: TerminalTabBar,
    /// dock 布局变化订阅。
    dock_layout_sub: Option<Subscription>,
    /// 下一个新建会话的标签组（用一次即清空）。
    pending_session_group: Option<WeakEntity<TabGroup>>,
    /// 设置窗口句柄。
    settings_window: Option<WindowHandle<Root>>,
    /// 文件预览窗口句柄(一个窗口装多个标签,见 [`crate::preview_window`])。
    preview_window: Option<WindowHandle<Root>>,
    /// 「背景」焦点。
    background_focus: FocusHandle,
    /// 上一次已经应用过的活动上下文（见 [`AppRoot::sync_active_context`]）。
    last_context: Option<ActiveContext>,
    /// 已经安排了一次延后的上下文同步（合并同一帧内的多次触发）。
    context_sync_scheduled: bool,
    wayx_status: platform::WayxStatus,
    _wayx_watch: gpui::Task<()>,
    _palette_watch: Subscription,
}

/// 当前活动会话的「快照」。
///
/// 只在**变化**时才重新应用（标题 / 文件管理器 / 侧栏可用性）。以前这些是在
/// `render` 里逐帧做的，现在由终端、焦点、标签页和 dock 的事件触发。
#[derive(Clone, PartialEq, Eq)]
struct ActiveContext {
    /// 活动标签页。
    tab: Option<gpui::EntityId>,
    /// 活动标签页里聚焦的那一格。
    pane: Option<gpui::EntityId>,
    /// 它所属的连接实例（远端会话才有）。
    session: Option<terminal::SessionId>,
    /// 是否已连上（本地会话恒为真）。
    connected: bool,
    /// 是不是远端会话（决定文件管理器入口）。
    remote: bool,
    /// 远端 shell 报的当前目录。
    directory: Option<String>,
    /// 窗口标题。
    title: String,
}

impl AppRoot {
    fn close_after_pending_transfers(
        &mut self,
        cancel: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if cancel {
            self._transfers.update(cx, |manager, cx| manager.cancel_all(cx));
        }
        if self._transfers.read(cx).active_count() == 0 {
            self.finish_main_close(window, cx);
        } else {
            self.close_when_transfers_finish = true;
        }
    }

    fn finish_main_close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.close_when_transfers_finish = false;
        if Self::preview_has_pending_changes(self.preview_window, cx) {
            window.push_notification(
                Notification::warning("预览文件仍在保存或有未保存的改动,请先在预览窗口处理"),
                cx,
            );
            return;
        }
        let root = cx.weak_entity();
        window.defer(cx, move |window, cx| {
            let Ok((active, preview)) = root.read_with(cx, |root, cx| {
                (root._transfers.read(cx).active_count(), root.preview_window)
            }) else {
                return;
            };
            if active == 0 && !Self::preview_has_pending_changes(preview, cx) {
                window.remove_window();
            }
        });
    }

    fn should_close(root: &WeakEntity<Self>, window: &mut Window, cx: &mut App) -> bool {
        let Ok((active, preview)) = root.read_with(cx, |root, cx| {
            (root._transfers.read(cx).active_count(), root.preview_window)
        }) else {
            return true;
        };
        if Self::preview_has_pending_changes(preview, cx) {
            window.push_notification(
                Notification::warning("预览文件仍在保存或有未保存的改动,请先在预览窗口处理"),
                cx,
            );
            return false;
        }
        if close_after_transfers(active, true) {
            return true;
        }
        if !window.has_active_dialog(cx) {
            let returning = root.clone();
            let dismissed = root.clone();
            let waiting = root.clone();
            let cancelling = root.clone();
            window.open_dialog(cx, move |dialog, _window, _cx| {
                let returning = returning.clone();
                let dismissed = dismissed.clone();
                let waiting = waiting.clone();
                let cancelling = cancelling.clone();
                dialog
                    .title("传输尚未完成")
                    .w(px(420.))
                    .child(div().text_sm().child(format!("仍有 {active} 项传输在运行。")))
                    .footer(
                        DialogFooter::new()
                            .child(Button::new("transfer-exit-back").label("返回").on_click(move |_, window, cx| {
                                window.close_dialog(cx);
                                let _ = returning.update(cx, |root, _| root.close_when_transfers_finish = false);
                            }))
                            .child(Button::new("transfer-exit-cancel").label("取消传输并退出").on_click(move |_, window, cx| {
                                window.close_dialog(cx);
                                let _ = cancelling.update(cx, |root, cx| {
                                    root.close_after_pending_transfers(true, window, cx);
                                });
                            }))
                            .child(Button::new("transfer-exit-wait").label("等待完成").primary().on_click(move |_, window, cx| {
                                window.close_dialog(cx);
                                let _ = waiting.update(cx, |root, cx| {
                                    root.close_after_pending_transfers(false, window, cx);
                                });
                            })),
                    )
                    .on_cancel(move |_, _, cx| {
                        let _ = dismissed.update(cx, |root, _| root.close_when_transfers_finish = false);
                        true
                    })
            });
        }
        false
    }

    /// 点击终端之外时把焦点从终端拿走。
    fn on_background_mouse_down(
        &mut self,
        _: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let terminal_focused = self
            .tabs
            .iter()
            .any(|tab| tab.read(cx).is_focused(window, cx));
        if terminal_focused {
            window.focus(&self.background_focus, cx);
        }
    }

    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let dock = Self::build_dock(window, cx);
        let dock_sub = cx.observe(&dock, |_, _, cx| cx.notify());
        let palette_watch = cx.observe_global::<config::ColorSchemeRegistry>(|root, cx| {
            if let Some(render) = config::refresh_color_scheme(cx) {
                root.apply_render_settings(render, cx);
            }
            cx.refresh_windows();
        });
        let sessions = cx.new(|_| SessionsState::new());
        let app_root = cx.weak_entity();
        let transfers = cx.new(|_| TransferManager::default());
        let transfer_sub = cx.observe_in(&transfers, window, |root: &mut Self, manager, window, cx| {
            if close_after_transfers(manager.read(cx).active_count(), root.close_when_transfers_finish) {
                root.finish_main_close(window, cx);
            }
        });
        let transfer_completion_sub = cx.subscribe(&transfers, |root: &mut Self, _, event, cx| {
            if event.result.is_ok() {
                match &event.done {
                    TransferDone::Preview(request) => {
                        let request = request.clone();
                        let root = cx.weak_entity();
                        cx.defer(move |cx| {
                            let _ = root.update(cx, |root, cx| root.open_preview_window(request, cx));
                        });
                    },
                    TransferDone::OpenExternally(path) => cx.open_with_system(path),
                    TransferDone::Nothing => {}
                }
            }
            root.files.update(cx, |files, cx| files.on_transfer_done(
                event.session, event.label.clone(), &event.result, event.refresh.clone(), cx,
            ));
        });
        let files = cx.new(|cx| FilesState::new(window, app_root, transfers.clone(), cx));
        let sidebar = cx.new(|cx| Sidebar::new(sessions.clone(), files.clone(), SidebarView::Sessions, cx));
        let right_sidebar = cx.new(|cx| Sidebar::new(sessions.clone(), files.clone(), SidebarView::Files, cx));
        let right_sidebar_sub = cx.observe(&right_sidebar, |_, _, cx| cx.notify());
        let sidebar_sub = cx.observe(&sidebar, |_, _, cx| cx.notify());

        platform::start_wayx();
        let wayx_watch = cx.spawn(async move |root, cx| {
            loop {
                cx.background_executor().timer(std::time::Duration::from_millis(200)).await;
                let status = platform::wayx_status();
                if root.update(cx, |root, cx| {
                    if root.wayx_status != status { root.wayx_status = status; cx.notify(); }
                }).is_err() { break; }
            }
        });
        let mut this = Self {
            tabs: Vec::new(),
            active: 0,
            sidebar,
            _sidebar_sub: sidebar_sub,
            right_sidebar,
            _right_sidebar_sub: right_sidebar_sub,
            sessions,
            files,
            _transfers: transfers,
            _transfer_sub: transfer_sub,
            _transfer_completion_sub: transfer_completion_sub,
            close_when_transfers_finish: false,
            dock,
            _dock_sub: dock_sub,
            zoom_tab_bar: TerminalTabBar::new(cx.weak_entity()),
            dock_layout_sub: None,
            pending_session_group: None,
            settings_window: None,
            preview_window: None,
            background_focus: cx.focus_handle(),
            last_context: None,
            context_sync_scheduled: false,
            wayx_status: platform::wayx_status(),
            _wayx_watch: wayx_watch,
            _palette_watch: palette_watch,
        };
        this.dock_layout_sub = Some(cx.subscribe_in(
            &this.dock,
            window,
            |root, _, event, window, cx| {
                if matches!(event, DockEvent::LayoutChanged) {
                    root.sync_tabs_with_dock(cx);
                    root.sync_active_context(window, cx);
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

    /// 记下下一个新建标签页该进的标签组。
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

    /// 安排一次「下一帧重算活动上下文」。
    ///
    /// 给**没有窗口**或**正处在别的实体更新中**的回调用（例如终端视图的通知回调）：
    /// 那里不能同步重读那个实体（gpui 会报重复租借），也不应该每帧都算一遍。
    /// 同一帧内多次调用只排一次。
    pub(crate) fn schedule_context_sync(&mut self, cx: &mut Context<Self>) {
        if self.context_sync_scheduled {
            return;
        }
        self.context_sync_scheduled = true;
        let root = cx.weak_entity();
        Self::defer_after_update(root, cx, |root, window, cx| {
            root.context_sync_scheduled = false;
            root.sync_active_context(window, cx);
        });
    }

    /// 往主窗口推一条通知。
    ///
    /// 给「没有窗口句柄」的地方用(比如文件管理器的传输任务回调)。没有活动窗口 = 应用在
    /// 收尾,直接丢掉。
    pub(crate) fn notify(&self, note: Notification, cx: &mut App) {
        let Some(window) = cx.active_window() else {
            log::debug!("没有活动窗口,通知被丢掉");
            return;
        };
        let _ = window.update(cx, |_, window, cx| window.push_notification(note, cx));
    }

    /// 让「文件管理器」把某个远端目录重读一次。
    ///
    /// 给预览窗口用:它把改过的文本写回远端之后,树里那份目录项(大小)就旧了(见
    /// [`crate::preview_window::PreviewWindow::refresh_parent_dir`])。
    /// `session` = 写回那条连接；文件树已经切到别的连接时这次刷新会被忽略。
    pub(crate) fn reload_files_dir(
        &mut self,
        session: terminal::SessionId,
        dir: &str,
        cx: &mut Context<Self>,
    ) {
        self.files
            .update(cx, |files, cx| files.reload_from(session, dir, cx));
    }

    /// 重算「当前会话」并只在**变化**时应用。
    ///
    /// 以前这段逻辑在 [`Self::render`] 里逐帧跑（依赖重绘驱动业务）。现在由
    /// 终端状态 / 焦点 / 标签页 / dock 的事件调用：
    ///
    /// - 焦点落在哪一格（[`TabPane::is_focused`]）⇒ 当前标签页；
    /// - 活动标签页的连接实例、连接状态、远端目录、标题；
    /// - 文件管理器同步（换连接时要重读根目录）与「跟着终端走」；
    /// - 侧栏「文件管理器」入口可用性。
    ///
    /// 幂等且带快照去重，事件重复触发也不会反复发请求。
    pub(crate) fn sync_active_context(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(index) = self
            .tabs
            .iter()
            .position(|tab| tab.read(cx).is_focused(window, cx))
        {
            self.active = index;
        }

        let tab = self.tabs.get(self.active);
        let remote = tab.is_some_and(|tab| tab.read(cx).is_remote());
        let connected = tab.is_some_and(|tab| tab.read(cx).is_connected(cx));
        let snapshot = ActiveContext {
            tab: tab.map(|tab| tab.entity_id()),
            pane: tab.map(|tab| tab.read(cx).focused_id()),
            session: tab.and_then(|tab| tab.read(cx).remote_session(cx)),
            connected,
            remote,
            directory: tab
                .filter(|tab| tab.read(cx).is_connected(cx))
                .and_then(|tab| tab.read(cx).remote_directory(cx)),
            title: tab
                .map(|tab| tab.read(cx).name(cx).to_string())
                .unwrap_or_else(|| "Alacrterm".to_string()),
        };

        // 路径框要等窗口才能更新（见 `FilesState::path_input_pending`），所以即便快照没变
        // 也得放它过去。
        let pending_input = self.files.read(cx).needs_window_sync();
        if !needs_context_apply(self.last_context.as_ref(), &snapshot, pending_input) {
            return;
        }
        self.last_context = Some(snapshot.clone());

        window.set_window_title(&snapshot.title);

        // 未连上时不给句柄：连上之前目录树没有意义（也避免展示上一轮的残留）。
        let fs = tab
            .filter(|_| snapshot.connected)
            .and_then(|tab| tab.read(cx).remote_fs(cx));
        self.files.update(cx, |files, cx| {
            files.sync(fs, window, cx);
            // 远端终端在哪儿，文件管理器就在哪儿（用户手动跳走、shell 没动时不动）。
            files.follow_terminal(snapshot.directory.as_deref(), cx);
        });
        self.right_sidebar
            .update(cx, |sidebar, cx| sidebar.set_files_enabled(snapshot.remote, cx));
    }
}

impl Render for AppRoot {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(node) = self.dock.read(cx).zoomed_group() {
            let group = self.tabs.iter()
                .filter_map(|tab| tab.read(cx).group())
                .filter_map(|group| group.upgrade())
                .find(|group| group.read(cx).node() == node);
            if let Some(group) = group {
                let context = group.read(cx).context(cx);
                let tabs = self.zoom_tab_bar.render_title_bar(&context, window, cx);
                return v_flex()
                    .id("app-root-zoomed")
                    .size_full()
                    .bg(cx.theme().background)
                    .child(
                        TitleBar::new()
                            .bg(cx.theme().tab_bar)
                            .when(!cfg!(target_os = "macos"), |bar| bar.pl_0())
                            .child(tabs),
                    )
                    .child(
                        div().flex_1().min_h_0().w_full().overflow_hidden()
                            .child(self.dock.clone()),
                    )
                    .into_any_element();
            }
        }

        // 主体：侧边栏 | 中间列。
        //
        // ⚠️ 这里**不做**业务同步（当前标签页 / 窗口标题 / 文件管理器 / 侧栏可用性都由
        // [`Self::sync_active_context`] 在事件里完成）。渲染只管按现有状态摆界面。
        // 唯一保留的「每帧动作」是侧栏宽度钉回拖拽值 —— 那是纯布局，没有副作用。

        // 侧边栏宽度每帧钉回用户拖拽值。
        self.sidebar
            .update(cx, |sidebar, cx| sidebar.pin_width(window, cx));
        self.right_sidebar
            .update(cx, |sidebar, cx| sidebar.pin_width(window, cx));

        // 中间列：终端区或欢迎页。
        let has_tabs = !self.tabs.is_empty();
        let terminal_area = if has_tabs {
            self.dock.clone().into_any_element()
        } else {
            self.render_welcome(cx)
        };
        let mid_column = v_flex()
            .h_full()
            .w_full()
            .overflow_hidden()
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .overflow_hidden()
                    .child(terminal_area),
            )
            .into_any_element();

        // 分栏组：最左整列 | 中间列。
        let visible = self.sidebar.read(cx).panel_visible(cx);
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
                        .size(width)
                        // 可以拖到左沿以折叠面板。
                        .size_range(px(0.)..Pixels::MAX)
                        .flex_none()
                        .child(panel),
                )
                .child(resizable_panel().child(mid_column))
                .into_any_element()
        } else {
            mid_column
        };

        let right = self.right_sidebar.read(cx);
        let right_visible = right.panel_visible(cx);
        let right_width = right.width();
        let right_resize = right.resize_state().clone();
        let right_state = self.right_sidebar.downgrade();
        let body = if right_visible {
            h_resizable("right-sidebar-split")
                .with_state(&right_resize)
                .on_resize(move |state, _, cx| {
                    if let Some(width) = state.read(cx).sizes().get(1).copied() {
                        let _ = right_state.update(cx, |sidebar, cx| sidebar.set_panel_width(width, cx));
                    }
                })
                .child(resizable_panel().child(body))
                .child(
                    resizable_panel().size(right_width)
                        .size_range(px(0.)..Pixels::MAX).flex_none()
                        .child(self.right_sidebar.clone()),
                )
                .into_any_element()
        } else {
            body
        };

        let status_bar = self.sidebar.update(cx, |sidebar, cx| {
            sidebar.render_status_bar(&self.right_sidebar, cx)
        });

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
                        )
                        .child(self.wayx_button(cx))
                        .child(
                            div()
                                .id("settings-control-area")
                                .occlude()
                                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                                .child(
                                    Button::new("settings-control")
                                        .ghost()
                                        .icon(assets::IconName::Settings)
                                        .tooltip("设置")
                                        .accessibility_label("设置")
                                        .on_click(|_, window, cx| {
                                            window.dispatch_action(Box::new(actions::OpenSettings), cx);
                                        }),
                                ),
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
            .child(status_bar)
            // 覆盖层由 gpui-kit 的 `Root` 渲染。
            .into_any_element()
    }
}

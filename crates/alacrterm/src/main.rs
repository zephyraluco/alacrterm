//! 基于 gpui-kit (gpui-component 0.6) 外壳 + alacrterm 终端核心的终端应用。
//!
//! 架构（方案 A，移植自 https://github.com/zephyraluco/alacrterm）：
//! - `crates/terminal`      —— 终端仿真核心（alacritty_terminal 0.26 + PTY + 事件循环）
//! - `crates/terminal_view` —— 终端视图（自定义 gpui Element 逐 cell 渲染）
//! - `crates/util`          —— Shell 探测 / 路径工具（来自 Zed）
//!
//! 应用外壳按「左右两条侧边栏 + 一条共用状态栏」拆分为独立文件：
//! - [`sidebar_panel`]  —— 左 / 右侧边栏（+ 顶部可拖动的视图标签与两枚折叠开关），
//!   以及侧边栏「会话」列表的记录模型与渲染（[`sidebar_panel::sessions`]）
//! - [`terminal_panel`] —— 中间容器：标签栏 + 终端，以及运行时句柄 [`terminal_panel::Session`]
//! - [`welcome`]        —— 终端容器关闭后中间列的欢迎页（默认背景板）
//! - [`status_bar`]     —— **全程序共用的唯一状态栏**（常驻窗口底部，见下）
//! - [`dialog`]         —— 对话框：「新建会话」（SSH 参数）/「新建文件夹」
//! - [`settings_window`]  —— 主窗口的从属设置子窗口（非对话框）
//!
//! 本文件只保留程序入口、根视图 [`AppRoot`]（共享状态 + 布局装配 + 设置弹窗）。
//! 窗内只有三级结构：标题栏 / 主体 / 状态栏。主体是两级嵌套的可拖拽分栏组
//! （官方 resizable 面板组）：内层 `main-split` = 左侧边栏 | 中间列，
//! 外层 `right-split` = 内层 | 右侧边栏；弹窗层也在此装配。
//! 中间列里的**终端区**额外包了一层 dock（[`terminal_panel`]，gpui-kit `DockArea`
//! 的 center）——侧边栏不进 dock，仍由上面的分栏面板管宽度与折叠。
//!
//! **两条独立的线**（务必区分，它们是本应用最容易混淆的一对概念）：
//! - **会话记录**（[`SessionEntry`]，侧边栏「会话」列表）：只是连接参数与分组结构
//!   （文件夹可以嵌套，行上不显示「里面有几条」）。列表形态参考 MobaXterm：
//!   顶层直接是用户自己建的文件夹与记录（**没有自动生成的根文件夹**）；
//!   状态栏左下角 `+` 建文件夹、右下角 `+` 新建会话（**总是落在顶层**），
//!   文件夹行右键可以「在这里新建会话」；双击记录才开终端。
//! - **终端会话**（[`AppRoot::terminals`]，dock 里的标签页）：真正在跑的终端，
//!   由标签栏的 `+`（本地终端）或双击某条记录创建，关掉标签只影响终端。
//!
//! 折叠规则：**左侧边栏折叠**时，它连同顶部的视图标签一起让位给终端；**右侧边栏折叠**时整块让位给终端。
//! 两条侧边栏**默认折叠**（启动即折叠态），因此首屏只有标题栏 + 中间列 + 状态栏。
//! **终端标签页全部关闭**时中间容器消失，中间列改显示欢迎页（[`welcome`]，
//! 见 [`AppRoot::render_welcome`]；**启动时同样是这个状态**——不自动开会话；
//! 之后可从欢迎页的「新建终端」或标签栏的 `+` 重新开一个本地终端，或双击侧边栏里的会话记录连远端）。
//! 这些情况都**不影响底部状态栏**（三条状态栏常驻），而侧边栏的「折叠 / 展开」开关与「设置」入口
//! 都在**标题栏**（右端 / 左端）：标题栏永远在，因此不会出现「窗口全空、没有任何恢复入口」的死角。

mod actions;
mod assets;
mod config;
#[cfg(windows)]
mod conpty_backend;
mod dialog;
mod settings_window;
mod sidebar_panel;
mod status_bar;
mod tab_bar;
mod terminal_panel;
mod welcome;

use gpui::{
    App, AppContext as _, AsyncApp, Bounds, Context, Entity, FocusHandle, InteractiveElement as _,
    IntoElement, KeyBinding, MouseButton, MouseDownEvent, ParentElement as _, Render, Styled as _,
    Subscription, WeakEntity, Window, WindowBounds, WindowHandle, WindowOptions, div, px, size,
};
use gpui_kit::{
    QuitMode,
    component::{
        ActiveTheme as _, Root, Sizable as _, ThemeMode, TitleBar,
        button::{Button, ButtonVariants as _},
        dock::{DockArea, DockEvent, TabGroup},
        h_flex,
        resizable::{h_resizable, resizable_panel},
        v_flex,
    },
};
use sidebar_panel::{
    RIGHT_SIDEBAR_MAX_WIDTH, RIGHT_SIDEBAR_MIN_WIDTH, SIDEBAR_MAX_WIDTH, SIDEBAR_MIN_WIDTH,
    Sidebar, SidebarSide, toggle_button,
};
use sidebar_panel::files::FilesState;
use sidebar_panel::sessions::SessionsState;
use tab_bar::TerminalDockSkin;
use terminal_panel::Session;

fn main() {
    // 必须在建第一个 PTY 之前执行（决定 conpty.dll 能否命中）。
    #[cfg(windows)]
    conpty_backend::ensure();

    gpui_kit::application()
        // 注册自有资产源（assets/icons 经 rust-embed 嵌入，IconName 由宏扫描生成）。
        .with_assets(assets::Assets)
        .with_quit_mode(QuitMode::LastWindowClosed)
        .run(|cx: &mut App| {
            gpui_kit::init(cx);
            // 配置三步（顺序不能换）：装全局 → 登记主题库（含热重载）→ 按配置挂主题槽位。
            config::install(cx);
            config::load_themes(cx);
            config::apply_saved_themes(cx);
            // 终端是深色背景，界面主题跟随用暗色。
            config::change_theme(ThemeMode::Dark, cx);

            let bounds = Bounds::centered(None, size(px(1100.), px(700.)), cx);
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    // 隐藏系统标题栏，改由 gpui-kit `TitleBar` 处理拖拽 / 窗口按钮。
                    ..TitleBar::window_options()
                },
                |window, cx| {
                    let root = cx.new(|cx| AppRoot::new(window, cx));
                    // 注册全局 action 监听器（用实体弱引用）。
                    AppRoot::register_actions(root.downgrade(), cx);
                    // `Ctrl+,` 打开设置；绑在 `None` context 上 ⇒ 焦点在终端里也能触发。
                    cx.bind_keys([KeyBinding::new("ctrl-,", actions::OpenSettings, None)]);
                    cx.new(|cx| Root::new(root, window, cx))
                },
            )
            .expect("failed to open window");
        });
}

/// 应用根视图：装配标题栏 + 两条侧边栏 + 中间列 + 弹窗层（只存跨组件共享状态）。
struct AppRoot {
    /// 所有终端会话（保持运行，切换仅切换显示）。
    terminals: Vec<Session>,
    /// 当前显示的终端下标。
    active: usize,
    /// 左 / 右两条侧边栏（各一个实体：自带标签、折叠、宽度与渲染，见 [`sidebar_panel::Sidebar`]）。
    left_sidebar: Entity<Sidebar>,
    right_sidebar: Entity<Sidebar>,
    /// 侧边栏「会话」视图的状态（两条侧边栏共用；记录与 [`AppRoot::terminals`] 无关）。
    sessions: Entity<SessionsState>,
    /// 侧边栏「文件管理器」视图的状态（两条侧边栏共用；数据源 = 当前远端会话的 SFTP）。
    files: Entity<FilesState>,
    /// 终端会话的 dock（一个会话 = center 里的一块面板；没有会话时整块换成欢迎页）。
    dock: Entity<DockArea>,
    /// dock 布局变化的订阅：会话表顺序 / 成员跟着 dock 走。
    dock_layout_sub: Option<Subscription>,
    /// 下一个新建会话放进哪个标签组（标签栏 `+` 设置，用一次即清空）。
    pending_session_group: Option<WeakEntity<TabGroup>>,
    /// 设置窗口的句柄（重复点击只激活已有窗口；窗口关掉后句柄失效、下次重新开）。
    settings_window: Option<WindowHandle<Root>>,
    /// 「背景」焦点：点击终端之外时接管焦点（用真实句柄而非 `Window::blur`，否则 Tab 会失效）。
    background_focus: FocusHandle,
}

impl AppRoot {
    /// 点击终端**之外**时把焦点从终端拿走（gpui 不会因点击非可获焦区域而移焦）；
    /// 只在当前焦点确实是某个终端时动手。
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
        // 三个子组件实体：会话列表 + 文件管理器（两条侧边栏共用）+ 左右两条侧边栏。
        // 这两个视图状态只有侧边栏需要，根视图只借用它们装配，不持有。
        let sessions = cx.new(SessionsState::new);
        let files = cx.new(|cx| FilesState::new(window, cx));
        let left_sidebar = cx.new(|cx| {
            Sidebar::new(SidebarSide::Left, sessions.clone(), files.clone(), cx)
        });
        let right_sidebar =
            cx.new(|cx| Sidebar::new(SidebarSide::Right, sessions.clone(), files.clone(), cx));
        // 两条侧边栏互指：标签跨栏拖动时要把视图从另一条实体取过来（见 `Sidebar::take_dragged`）。
        left_sidebar.update(cx, |sidebar, _| sidebar.connect(right_sidebar.downgrade()));
        right_sidebar.update(cx, |sidebar, _| sidebar.connect(left_sidebar.downgrade()));

        let mut this = Self {
            terminals: Vec::new(),
            active: 0,
            left_sidebar,
            right_sidebar,
            sessions,
            files,
            dock,
            dock_layout_sub: None,
            pending_session_group: None,
            settings_window: None,
            background_focus: cx.focus_handle(),
        };
        // 会话表跟着 dock 的布局走：拖标签换位 / 拖出分屏 / 从面板菜单关掉面板，
        // 都会在这里把顺序、成员与当前会话同步过来。
        this.dock_layout_sub = Some(cx.subscribe_in(
            &this.dock,
            window,
            |root, _, event, _window, cx| {
                if matches!(event, DockEvent::LayoutChanged) {
                    root.sync_sessions_with_dock(cx);
                }
            },
        ));
        // 启动时不建会话：中间列显示欢迎页，终端由用户从欢迎页 / 标签栏 `+` 自己开。
        this
    }

    /// 建立终端会话的 dock（[`crate::terminal_panel`]）。
    ///
    /// 布局**不锁定**：标签可拖动换位、拖到边缘能把 center 分屏（终端只进 center，
    /// 侧边栏不进 dock）。皮肤是我们自绘的（`tab_bar`）。
    fn build_dock(window: &mut Window, cx: &mut Context<Self>) -> Entity<DockArea> {
        let root = cx.weak_entity();
        let dock = cx.new(|cx| {
            // 皮肤 = gpui-kit 的 `DockSkin` + 自绘标签栏（见 `tab_bar`）。
            let skin = TerminalDockSkin::new(cx, root);
            DockArea::new("terminal-dock", None, window, cx).with_renderer(skin)
        });

        dock.update(cx, |area, cx| {
            // 不锁布局 ⇒ 标签可拖动重排 / 拖出分屏；面板由会话创建流程添加。
            area.set_locked(false, window, cx);
        });
        dock
    }

    /// 记下「下一个新建的会话该进哪一组」（标签栏 `+` 按钮调用）。
    pub(crate) fn set_pending_session_group(&mut self, group: Option<WeakEntity<TabGroup>>) {
        self.pending_session_group = group;
    }

    /// 把一个「需要窗口 + 需要 &mut 根视图」的操作推迟到本次窗口更新之后执行。
    ///
    /// 用于对话框 `on_ok`、全局 action 监听器这类回调：它们执行期间目标窗口仍在
    /// 「更新栈」上，`WeakEntity::update_in` 会因 `App::with_window` 取不到窗口而失败
    /// （`Err("entity has no current window")`；同帧内的 `window.defer` 也一样）。
    /// 让出一拍（异步任务 + 1ms 定时器）后窗口已放回，`update_in` 即可成功。
    ///
    /// 用 `App::spawn`（不依赖窗口），因此回调里即使只有 `&mut App` 也能调用。
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
        // 主体：侧边栏与终端由各自的模块渲染，中间是可拖拽分隔条（视图标签在侧边栏顶部）。

        // 会话表变了就同步进会话树（`TreeState` 是快照，必须在渲染前对齐，见该方法文档）。
        self.sessions.update(cx, |sessions, cx| sessions.sync_tree(cx));

        // 「文件管理器」只服务**远端**会话：本地目录用系统自己的文件管理器打开就好，
        // 应用里再摆一份既多余、又只能看到本机目录。所以本地终端（以及没有会话时）
        // 既不摆这个视图，也不去连远端文件系统。
        let active = self.terminals.get(self.active);
        let remote = active.is_some_and(|session| session.is_remote(cx));
        // 数据源 = 当前会话的 SFTP 句柄（懒连接，拿到它不会联网）。**必须等会话连上**
        // 再给：连上之前主机密钥可能还没确认，这条连接会因为没有确认通道而失败。
        // 终端实体是异步建的，刚开完会话的头几帧可能还没有 ⇒ 视图先摆空占位。
        let fs = if remote {
            active
                .filter(|session| session.view.read(cx).is_connected(cx))
                .and_then(|session| session.view.read(cx).remote_fs(cx))
        } else {
            None
        };
        self.files
            .update(cx, |files, cx| files.sync(fs, window, cx));
        self.left_sidebar
            .update(cx, |sidebar, cx| sidebar.set_files_enabled(remote, cx));
        self.right_sidebar
            .update(cx, |sidebar, cx| sidebar.set_files_enabled(remote, cx));

        // 侧边栏宽度只由用户拖拽决定：容器尺寸变化引起的比例重排先钉回去（见子实体）。
        self.left_sidebar
            .update(cx, |sidebar, cx| sidebar.pin_width(window, cx));
        self.right_sidebar
            .update(cx, |sidebar, cx| sidebar.pin_width(window, cx));

        // 中间列 = 终端 dock + 公共状态栏（常驻）；状态栏在 dock 外面，
        // 没有会话时整块 dock 换成欢迎页。
        let terminal_area = if self.terminals.is_empty() {
            self.render_welcome(cx)
        } else {
            self.dock.clone().into_any_element()
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
            .child(status_bar::empty_status_bar())
            .into_any_element();

        // 两级嵌套分栏（两组面板各自持有宽度，互不干扰）：
        //   main-split = 左侧边栏 | 中间列；right-split = 内层 | 右侧边栏
        // 侧边栏状态在各自实体里，这里只取装配所需的值（`read` 的借用很短）。
        // 侧边栏本身作为元素直接 `.child(实体)`：它自己实现 `Render`。
        let left_visible = self.left_sidebar.read(cx).visible();
        let left_width = self.left_sidebar.read(cx).width();
        let left_resize = self.left_sidebar.read(cx).resize_state().clone();
        let left_state = self.left_sidebar.downgrade();
        let left_panel = self.left_sidebar.clone();
        let center = if left_visible {
            h_resizable("main-split")
                // 绑定侧边栏实体持有的状态：折叠再展开后宽度不会丢失。
                .with_state(&left_resize)
                // 拖拽结束后记下新宽度，作为窗口尺寸变化时的「期望宽度」。
                .on_resize(move |state, _window, cx| {
                    if let Some(width) = state.read(cx).sizes().first().copied() {
                        let _ = left_state.update(cx, |sidebar, _| sidebar.set_width(width));
                    }
                })
                .child(
                    resizable_panel()
                        .size(left_width)
                        .size_range(SIDEBAR_MIN_WIDTH..SIDEBAR_MAX_WIDTH)
                        // flex_none：宽度完全由面板状态决定，否则组件内置的
                        // flex_grow_1 会把侧边栏撑得比设定值更宽。
                        .flex_none()
                        .child(left_panel),
                )
                .child(resizable_panel().child(mid_column))
                .into_any_element()
        } else {
            // 折叠态：左侧边栏整块不渲染（它自己不在树里），中间列铺满。
            mid_column
        };

        let right_visible = self.right_sidebar.read(cx).visible();
        let right_width = self.right_sidebar.read(cx).width();
        let right_resize = self.right_sidebar.read(cx).resize_state().clone();
        let right_state = self.right_sidebar.downgrade();
        let right_panel = self.right_sidebar.clone();
        let body = if right_visible {
            h_resizable("right-split")
                .with_state(&right_resize)
                // 拖拽结束后记下新宽度（右侧边栏是本组的最后一个面板，下标 1）。
                .on_resize(move |state, _window, cx| {
                    if let Some(width) = state.read(cx).sizes().get(1).copied() {
                        let _ = right_state.update(cx, |sidebar, _| sidebar.set_width(width));
                    }
                })
                // 左栏（左侧边栏 + 中间列）撑满剩余宽度，右栏宽度完全由面板状态决定。
                .child(resizable_panel().child(center))
                .child(
                    resizable_panel()
                        .size(right_width)
                        .size_range(RIGHT_SIDEBAR_MIN_WIDTH..RIGHT_SIDEBAR_MAX_WIDTH)
                        .flex_none()
                        .child(right_panel),
                )
                .into_any_element()
        } else {
            center
        };

        v_flex()
            .id("app-root")
            .size_full()
            .track_focus(&self.background_focus)
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_background_mouse_down))
            .bg(cx.theme().background)
            // ── 顶部：自绘标题栏（左：设置入口 / 中：标题 / 右：侧边栏开关 + 窗口控制）——
            // 标题栏内容区本身就是窗口拖拽区，但其中的按钮仍有自己的 hitbox，
            // 点击会被正常派发（与窗口控制按钮同理），因此这些按钮可以放在这里。
            .child(
                TitleBar::new().child(
                    h_flex()
                        .w_full()
                        .px(px(8.))
                        .gap(px(8.))
                        .items_center()
                        .child(
                            // 设置入口：文字态按钮（无边框无底色，hover 提亮），放在最左端。
                            //
                            // 必须 `div().occlude()` 包一层：标题栏内容区整体是窗口拖拽区
                            // （`WindowControlArea::Drag`），gpui 在 WM_NCHITTEST 里一旦命中
                            // 拖拽区就返回 HTCAPTION，点击会被系统当作「拖标题栏」而收不到
                            // （表现为点了没反应）。occlude 阻断它下方拖拽区的命中，
                            // 这块区域于是按普通客户区处理，点击正常派发给按钮。
                            div().occlude().child(
                                Button::new("open-settings")
                                    .text()
                                    .small()
                                    .label("设置")
                                    .on_click(
                                        cx.listener(|this, _, window, cx| {
                                            this.open_settings_window(window, cx)
                                        }),
                                    ),
                            ),
                        )
                        .child(
                            div()
                                .flex_1()
                                .overflow_hidden()
                                .text_size(px(13.))
                                .text_color(cx.theme().secondary_foreground)
                                .child("Alacrterm"),
                        )
                        // 侧边栏折叠开关（左 / 右各一枚），靠 flex_1 的标题顶到
                        // 内容区最右端、窗口控制按钮左侧（按钮本身由侧边栏模块给出）。
                        .child(
                            h_flex()
                                .items_center()
                                .gap_1()
                                .child(toggle_button(&self.left_sidebar, cx))
                                .child(toggle_button(&self.right_sidebar, cx)),
                        ),
                ),
            )
            // —— 中部：主体（左侧边栏 | 中间列 | 右侧边栏）——
            // 底部不再有整窗状态栏：三列的底边分别是各自的宽度区块
            // （左/右栏的状态栏在 `sidebar_panel` 里，中间那条公共的在 `mid_column` 里）。
            // 分栏组内部使用 size_full，需要这层 flex_1 容器提供「剩余宽度」，
            // 否则其 100% 宽会溢出错位。
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .overflow_hidden()
                    .child(body),
            )
            // —— 覆盖层：gpui-kit 0.6 的 Root 不会自动渲染 Dialog 层，
            //    必须在渲染树中显式挂载（参考官方 ai_recipes 示例），否则弹窗不显示。——
            .children(Root::render_dialog_layer(window, cx))
    }
}

//! 基于 gpui-kit (gpui-component 0.6) 外壳 + alacrterm 终端核心的终端应用。
//!
//! 架构（方案 A，移植自 https://github.com/zephyraluco/alacrterm）：
//! - `crates/terminal`      —— 终端仿真核心（alacritty_terminal 0.26 + PTY + 事件循环）
//! - `crates/terminal_view` —— 终端视图（自定义 gpui Element 逐 cell 渲染）
//! - `crates/util`          —— Shell 探测 / 路径工具（来自 Zed）
//!
//! 应用外壳按「最左整列 + 一条整窗底部状态栏」拆分为独立文件：
//! - [`components`]     —— 应用自有的可复用组件（基于 gpui-kit `base` + 自绘）：**活动栏**
//!   [`components::ActivityBar`]（48px 图标栏 + 侧边栏面板这一整列）与侧边栏外壳
//!   [`components::Sidebar`]
//! - [`sidebar_panel`]  —— 把上面的整列填上本应用的语义（视图入口 + 底部「设置」+ 当前
//!   视图的标题栏与内容 + 折叠状态），以及侧边栏「会话」列表的记录模型与渲染
//!   （[`sidebar_panel::sessions`]）
//! - [`terminal_panel`] —— 中间容器：标签栏 + 终端，以及运行时句柄 [`terminal_panel::Session`]
//! - [`welcome`]        —— 终端容器关闭后中间列的欢迎页（默认背景板）
//! - [`status_bar`]     —— **整窗底部**的状态栏（空条、独占窗口最后一行，横跨全部列）
//! - [`dialog`]         —— 对话框：「新建会话」（SSH 参数）/「新建文件夹」
//! - [`settings_window`]  —— 主窗口的从属设置子窗口（非对话框）
//!
//! 本文件只保留程序入口、根视图 [`AppRoot`]（共享状态 + 布局装配 + 设置弹窗）。
//! 窗内只有三级结构：标题栏 / 主体 / 状态栏 —— **状态栏独占窗口最后一行**（横跨全部列）。
//! 主体 = 可拖拽分栏组（官方 resizable 面板组，`main-split`）= **最左整列（48px 图标栏 +
//! 侧边栏面板，由 [`sidebar_panel::Sidebar`] 实体渲染）** | 中间列；弹窗层也在此装配。
//! 折叠时整列只剩那 48px 图标栏（没有可拖的缝，也不摆分栏组）—— 显示 / 折叠侧边栏与它显示
//! 哪个视图，全由图标栏的图标点选（见 `sidebar_panel::activity_bar`）。
//! 中间列里的**终端区**额外包了一层 dock（[`terminal_panel`]，gpui-kit `DockArea`
//! 的 center）——侧边栏不进 dock，仍由上面的分栏面板管宽度与折叠。
//!
//! **两条独立的线**（务必区分，它们是本应用最容易混淆的一对概念）：
//! - **会话记录**（[`SessionEntry`]，侧边栏「会话」列表）：只是连接参数与分组结构
//!   （文件夹可以嵌套，行上不显示「里面有几条」）。列表形态参考 MobaXterm：
//!   顶层直接是用户自己建的文件夹与记录（**没有自动生成的根文件夹**）；
//!   在「会话」视图的空白处 / 面板标题上右键 = 顶层「新建会话 / 新建文件夹」（**总是落在顶层**），
//!   文件夹行右键可以「在这里新建会话」；双击记录才开终端。
//! - **终端会话**（[`AppRoot::terminals`]，dock 里的标签页）：真正在跑的终端，
//!   由标签栏的 `+`（本地终端）或双击某条记录创建，关掉标签只影响终端。
//!
//! 折叠规则：**侧边栏折叠**时它整块让位给终端（最左那 48px 图标栏还在，点图标即可恢复）。
//! 侧边栏**默认折叠**（启动即折叠态），因此首屏只有标题栏 + 图标栏 + 中间列。
//! **终端标签页全部关闭**时中间容器消失，中间列改显示欢迎页（[`welcome`]，
//! 见 [`AppRoot::render_welcome`]；**启动时同样是这个状态**——不自动开会话；
//! 之后可从欢迎页的「新建终端」或标签栏的 `+` 重新开一个本地终端，或双击侧边栏里的会话记录连远端）。
//! 这些情况都**不影响**窗口底部那条空状态栏（它常驻），而「设置」入口与侧边栏的
//! 显示 / 折叠都在**图标栏**（常驻最左一列）：因此不会出现「窗口全空、没有任何恢复入口」的死角。

mod actions;
mod assets;
mod components;
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

use components::ACTIVITY_BAR_WIDTH;
use gpui::{
    App, AppContext as _, AsyncApp, Bounds, Context, Entity, FocusHandle, InteractiveElement as _,
    IntoElement, KeyBinding, MouseButton, MouseDownEvent, ParentElement as _, Pixels, Render,
    Styled as _, Subscription, WeakEntity, Window, WindowBounds, WindowHandle, WindowOptions, div,
    px, size,
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

/// 应用根视图：装配标题栏 + 侧边栏 + 中间列 + 弹窗层（只存跨组件共享状态）。
struct AppRoot {
    /// 所有终端会话（保持运行，切换仅切换显示）。
    terminals: Vec<Session>,
    /// 当前会话下标：以**键盘焦点**为准（[`AppRoot::render`] 每帧按焦点校正，
    /// 见 `terminal_panel::SessionPane::set_active` 那条说明），焦点不在任何终端里时
    /// 保持最后一次的值。
    active: usize,
    /// 唯一的侧边栏（自带视图清单、折叠、宽度与渲染，见 [`sidebar_panel::Sidebar`]）。
    sidebar: Entity<Sidebar>,
    /// 侧边栏「会话」视图的状态（记录与 [`AppRoot::terminals`] 无关）。
    sessions: Entity<SessionsState>,
    /// 侧边栏「文件管理器」视图的状态（数据源 = 当前远端会话的 SFTP）。
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
        // 三个子组件实体：会话列表 + 文件管理器（侧边栏的两个视图）+ 侧边栏本身。
        // 这两个视图状态只有侧边栏需要，根视图只借用它们装配，不持有。
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

        // 「当前会话」以**键盘焦点**为准：dock 是按**每个标签组**的显示标签回调
        // `SessionPane::set_active` 的，分屏时两组都会把自己那枚报成当前会话
        // （还会顺手把焦点抢过去），光看它分不出用户到底在哪枚终端里 —— 那正是
        // 「文件管理器」该跟着走的那枚。焦点不在任何终端里（点了侧边栏 / 设置、
        // 窗口失焦）时就什么都不动 ⇒ 保留最后一次的值：否则一点自己的文件树，
        // 视图就跳到另一个会话上去了。
        if let Some(index) = self
            .terminals
            .iter()
            .position(|session| session.view.read(cx).is_focused(window))
        {
            self.active = index;
        }

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
        self.sidebar
            .update(cx, |sidebar, cx| sidebar.set_files_enabled(remote, cx));

        // 侧边栏宽度只由用户拖拽决定：容器尺寸变化引起的比例重排先钉回去（见子实体）。
        self.sidebar
            .update(cx, |sidebar, cx| sidebar.pin_width(window, cx));

        // 中间列 = 终端区（有会话 = dock，没有 = 欢迎页）。
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

        // 分栏组：main-split = 最左整列（图标栏 + 侧边栏面板）| 中间列。
        // 面板宽 = 图标栏 + 侧边栏宽度；折叠时整列只剩图标栏，直接「图标栏 | 中间列」。
        let visible = self.sidebar.read(cx).visible();
        let width = self.sidebar.read(cx).width();
        let resize = self.sidebar.read(cx).resize_state().clone();
        let sidebar_state = self.sidebar.downgrade();
        let panel = self.sidebar.clone();
        let body = if visible {
            h_resizable("main-split")
                // 分界线由拖拽条自己画（hairline，悬停变胶囊条，与 dock 里的分隔条一致）。
                .with_state(&resize)
                // 拖拽结束后记下整列宽度，实体里再折算成侧边栏宽度。
                .on_resize(move |state, _window, cx| {
                    if let Some(width) = state.read(cx).sizes().first().copied() {
                        let _ = sidebar_state
                            .update(cx, |sidebar, cx| sidebar.set_panel_width(width, cx));
                    }
                })
                .child(
                    resizable_panel()
                        .size(ACTIVITY_BAR_WIDTH + width)
                        // 侧边栏没有最小 / 最大宽度：下界是图标栏，上界不限。
                        .size_range(ACTIVITY_BAR_WIDTH..Pixels::MAX)
                        // 宽度完全由面板状态决定，避免组件内置的 flex_grow_1 撑宽整列。
                        .flex_none()
                        .child(panel),
                )
                .child(resizable_panel().child(mid_column))
                .into_any_element()
        } else {
            // 折叠态：没有分栏组可拖，中间列铺满剩下的宽度。
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
            // 顶部：自绘标题栏（左：应用名 / 右：窗口控制）。
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
            // —— 中部：主体（最左整列 | 中间列）——
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
            // —— 底部：整窗状态栏（空条、常驻）——
            // 它**独占最后一行**、横跨全部列 ⇒ 窗口是「标题栏 / 主体 / 状态栏」三级结构；
            // 侧边栏与终端区都在它上面结束（侧边栏的描边也到它上沿为止）。
            .child(status_bar::empty_status_bar())
            // 覆盖层（Dialog / Sheet / 通知 / Tooltip / 原生菜单）由 gpui-kit 的 `Root` 自己按
            // 「插件」渲染（`gpui_kit::init` 注册，见 `crates/base/src/root.rs`），
            // 所以这里**不要再**挂 `Root::render_dialog_layer`（那个 API 已经没有了）。
    }
}

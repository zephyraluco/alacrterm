//! 基于 gpui-kit (gpui-component 0.6) 外壳 + alacrterm 终端核心的终端应用。
//!
//! 架构（方案 A，移植自 https://github.com/zephyraluco/alacrterm）：
//! - `crates/terminal`      —— 终端仿真核心（alacritty_terminal 0.26 + PTY + 事件循环）
//! - `crates/terminal_view` —— 终端视图（自定义 gpui Element 逐 cell 渲染）
//! - `crates/util`          —— Shell 探测 / 路径工具（来自 Zed）
//!
//! 应用外壳按「左右两条侧边栏 + 一条共用状态栏」拆分为独立文件：
//! - [`sidebar_panel`]  —— 左 / 右侧边栏（+ 两枚折叠开关与活动栏图标的渲染）
//! - [`terminal_panel`] —— 中间容器：标签栏 + 终端
//! - [`welcome`]        —— 终端容器关闭后中间列的欢迎页（默认背景板）
//! - [`status_bar`]     —— **全程序共用的唯一状态栏**（常驻窗口底部，见下）
//! - [`connection_dialog`] —— 「新建终端」建连对话框（IP / 端口 / 名称 / 用户名 / 密码）
//! - [`settings_window`]  —— 主窗口的从属设置子窗口（非对话框）
//! - [`status_metrics`]   —— 状态栏指标采样（连接状态 / CPU / 内存 / 网络）
//!
//! 本文件只保留程序入口、根视图 [`AppRoot`]（共享状态 + 布局装配 + 设置弹窗）。
//! 窗内只有三级结构：标题栏 / 主体 / 状态栏。主体是两级嵌套的可拖拽分栏组
//! （官方 resizable 面板组）：内层 `main-split` = 左侧边栏 | 终端，
//! 外层 `right-split` = 内层 | 右侧边栏；弹窗层也在此装配。
//!
//! 折叠规则：**左侧边栏折叠**时，它连同活动栏图标一起让位给终端
//! （图标由状态栏按折叠状态显示 / 隐藏）；**右侧边栏折叠**时整块让位给终端。
//! **终端标签页全部关闭**时中间容器消失，中间列改显示欢迎页（[`welcome`]，
//! 见 [`AppRoot::render_welcome`]；之后可从欢迎页或左侧边栏会话条目的右键菜单
//! 「新建终端」重新打开）。这些情况都**不影响底部状态栏**：它是全程序共用的一条、
//! 常驻不消失，两端的「折叠 / 展开侧边栏」开关因此永远可点，
//! 不会出现「窗口全空、没有任何恢复入口」的死角。

mod actions;
mod assets;
mod connection_dialog;
#[cfg(windows)]
mod conpty_backend;
mod settings_window;
mod sidebar_panel;
mod status_bar;
mod status_metrics;
mod tab_bar;
mod terminal_panel;
mod welcome;

use gpui::{
    App, AppContext as _, AsyncApp, Bounds, Context, Entity, FocusHandle, Hsla,
    InteractiveElement as _, IntoElement, MouseButton, MouseDownEvent, ParentElement as _, Pixels,
    Render, ScrollHandle, SharedString, Styled as _, WeakEntity, Window, WindowBounds, WindowHandle,
    WindowOptions, div, px, size,
};
use gpui_kit::{
    QuitMode,
    component::{
        ActiveTheme as _, Root, Sizable as _, Theme, ThemeMode, TitleBar,
        button::{Button, ButtonVariants as _},
        h_flex,
        resizable::{ResizableState, h_resizable, resizable_panel},
        v_flex,
    },
};
use terminal_view::TerminalView;
use util::shell::Shell;

use sidebar_panel::{
    RIGHT_SIDEBAR_DEFAULT_WIDTH, RIGHT_SIDEBAR_MAX_WIDTH, RIGHT_SIDEBAR_MIN_WIDTH,
    SIDEBAR_DEFAULT_WIDTH, SIDEBAR_MAX_WIDTH, SIDEBAR_MIN_WIDTH, SidebarView,
};
use status_metrics::{SAMPLE_INTERVAL, SystemMonitor};

/// 应用（或切换）界面主题，并重新压上我们的主题覆盖。
///
/// **分栏边界的那条竖线统一由拖拽条（`ResizeHandle`）来画**：它静止时会在边界处
/// 画一条 1px 线，`h_full` 贯穿整列（含两侧状态栏行）。所以我们要做的是**反方向**的
/// 覆盖——把侧边栏组件自己那条边框关掉：`Sidebar` 内部固定 `Side::Left → border_r_1()`
/// / `Side::Right → border_l_1()`，颜色取 `cx.theme().sidebar_border`（默认 = `border`，
/// 与拖拽条同色）。把它置为透明后，左右边界就只剩拖拽条那一条线，宽度天然一致。
///
/// ⚠️ 只做一次不够：`gpui_component::Theme::change()` 会把整套配色**投影**回主题 global
/// （包含 `sidebar_border`），所以**每次**换主题都要重新压。设置窗口的深浅色开关已经
/// 改走本函数，以后新增换主题的地方也必须走它。
///
/// ⚠️ 副作用：`sidebar_border` 还兼作侧边栏菜单「嵌套项缩进导线」的颜色
/// （gpui-component `sidebar/menu.rs`），它也会一起变成透明。
///
/// 传 `None` 作为窗口参数（与原先一致）：调用方需要自行 `cx.refresh_windows()`。
pub(crate) fn change_theme(mode: ThemeMode, cx: &mut App) {
    Theme::change(mode, None, cx);
    Theme::global_mut(cx).sidebar_border = Hsla::transparent_black();
}

fn main() {
    // 必须在建第一个 PTY **之前**执行：决定 conpty.dll 命中与否（看该模块文档）。
    #[cfg(windows)]
    conpty_backend::ensure();

    gpui_kit::application()
        // 注册自有资产源（alacrterm assets.rs 方式）：本 crate 的 assets/icons 目录
        // 经 rust-embed 嵌入，`crate::assets::IconName` 由 icon_named! 宏扫描生成，
        // 可自由增删图标文件。
        .with_assets(assets::Assets)
        .with_quit_mode(QuitMode::LastWindowClosed)
        .run(|cx: &mut App| {
            gpui_kit::init(cx);
            // 终端为深色背景，应用主题跟随使用暗色。
            change_theme(ThemeMode::Dark, cx);

            let bounds = Bounds::centered(None, size(px(1100.), px(700.)), cx);
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    // TitleBar::window_options()：隐藏系统标题栏（appears_transparent）,
                    // 由 gpui-kit TitleBar 自行处理拖拽 / 双击最大化 / 窗口控制按钮。
                    ..TitleBar::window_options()
                },
                |window, cx| {
                    let root = cx.new(|cx| AppRoot::new(window, cx));
                    // 注册全局 action 监听器（右键菜单项会派发这些 action），
                    // 用 `Entity<AppRoot>` 的弱引用：这里正好拿得到已构造好的实体。
                    AppRoot::register_actions(root.downgrade(), cx);
                    cx.new(|cx| Root::new(root, window, cx))
                },
            )
            .expect("failed to open window");
        });
}

/// 会话的连接目标：决定状态栏「连接状态」一栏显示什么。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SessionTarget {
    /// 本地系统 shell。
    Local,
    /// 通过 ssh 连接的远端主机。
    Ssh {
        user: String,
        host: String,
        port: String,
    },
}

impl SessionTarget {
    /// 状态栏显示用的简短描述。
    pub(crate) fn label(&self) -> String {
        match self {
            Self::Local => "本地".to_string(),
            Self::Ssh { user, host, port } => {
                if user.is_empty() {
                    format!("SSH {host}:{port}")
                } else {
                    format!("SSH {user}@{host}:{port}")
                }
            }
        }
    }
}

/// 新建会话所需的参数（显示名 + 要启动的 shell + 连接目标）。
pub(crate) struct SessionRequest {
    /// 用户填写的显示名；`None` 表示回退到终端自身标题。
    pub(crate) name: Option<SharedString>,
    /// 要启动的 shell（本地系统 shell，或 `ssh` 等外部命令）。
    pub(crate) shell: Shell,
    /// 连接目标（用于状态栏展示）。
    pub(crate) target: SessionTarget,
}

/// 一个终端会话：终端视图 + 显示名 + 连接目标。
///
/// 之所以不直接用 `TerminalView::title()`：它的标题来自终端通过 OSC 上报的内容，
/// 而对话框里的「名称」是用户命名的连接名，需要优先展示（标签页 / 侧边栏 / 状态栏）。
pub(crate) struct Session {
    pub(crate) view: Entity<TerminalView>,
    /// 用户填写的名称；`None` 表示未填写，回退到终端自身标题。
    pub(crate) name: Option<SharedString>,
    /// 连接目标，供状态栏显示连接状态。
    pub(crate) target: SessionTarget,
}

impl Session {
    /// 会话显示名：优先用户命名，否则用终端标题（无标题时为「终端」）。
    pub(crate) fn title(&self, cx: &App) -> SharedString {
        self.name
            .clone()
            .unwrap_or_else(|| self.view.read(cx).title())
    }
}

/// 应用根视图：装配标题栏 + 左侧容器 + 右侧容器 + 弹窗层。
///
/// 只保存两个容器共享的状态；容器各自的渲染与逻辑见
/// [`crate::sidebar_panel`] / [`crate::terminal_panel`]。
struct AppRoot {
    /// 所有终端会话（保持运行，切换仅切换显示）。
    terminals: Vec<Session>,
    /// 当前显示的终端下标。
    active: usize,
    /// 左侧边栏是否可见（只由状态栏最左端的折叠按钮切换；活动栏图标不会改变它）。
    sidebar_visible: bool,
    /// 左侧边栏当前视图（由状态栏里的活动栏图标切换；点击当前视图图标是空操作）。
    sidebar_view: SidebarView,
    /// 右侧边栏是否可见（由状态栏右端的折叠开关切换）。
    right_sidebar_visible: bool,
    /// 标签栏滚动句柄：跟踪 tabs 横向滚动，激活会话时把选中标签滚入可视区。
    tab_scroll_handle: ScrollHandle,
    /// 鼠标当前悬停的终端标签下标（`None` = 没有悬停任何标签）。
    ///
    /// 自绘标签栏（[`crate::tab_bar`]）用它决定关闭按钮是否渲染：gpui-pre 既没有
    /// `visible_on_hover` / `invisible`，又不能只靠透明度隐藏（会留下看不见但
    /// 仍可点击的热区），所以把悬停态存下来。
    hovered_tab: Option<usize>,
    /// 左侧边栏 / 终端分栏面板组（`main-split`）的共享状态。
    ///
    /// 实体由根视图持有（而非交给组件内部的 keyed state），这样侧边栏折叠再展开、
    /// 乃至窗口重绘后，用户拖出来的宽度都不会丢失。
    resize_state: Entity<ResizableState>,
    /// 终端 / 右侧边栏分栏面板组（`right-split`）的共享状态。
    ///
    /// 刻意与左侧分成两组嵌套面板：面板宽度按**下标**存在状态里，
    /// 若把三个面板塞进同一组，任一侧折叠都会让另一侧的下标漂移、宽度丢失。
    right_resize_state: Entity<ResizableState>,
    /// 左侧边栏的「期望宽度」（逻辑像素）：用户拖拽分隔条后的宽度记在这里。
    ///
    /// 分栏容器在**容器尺寸变化**时会把所有面板按比例重排（
    /// `ResizableState::adjust_to_container_size`），于是窗口一变宽、侧边栏就跟着
    /// 变宽。记下期望宽度后由 [`AppRoot::pin_sidebar_widths`] 把面板钉回去，
    /// 让宽窄变化全部由中间那一列吸收。
    sidebar_width: Pixels,
    /// 右侧边栏的「期望宽度」，含义同 [`AppRoot::sidebar_width`]。
    right_sidebar_width: Pixels,
    /// 上一次看到的 `main-split` / `right-split` 容器宽度。
    ///
    /// 分栏容器只在**容器宽度变化**的那一次布局里重排面板，所以「宽度和上次不一样」
    /// 就等于「刚发生过重排」——这是[`AppRoot::pin_sidebar_widths`] 判断该不该
    /// 动手的依据。拖拽分隔条不改变容器宽度，因此不会被误判成重排。
    main_split_width: Option<Pixels>,
    right_split_width: Option<Pixels>,
    /// 设置窗口的句柄（见 [`AppRoot::open_settings_window`]）。
    ///
    /// 用于「重复点击设置图标只激活已有窗口」；窗口被关闭后该句柄会失效，
    /// 下一次点击会重新开窗并覆盖它。
    settings_window: Option<WindowHandle<Root>>,
    /// 状态栏指标采样器（CPU / 内存 / 网络），由后台定时任务驱动。
    pub(crate) monitor: SystemMonitor,
    /// 「背景」焦点：点击终端之外时接管焦点。
    ///
    /// 用真实句柄而非 `Window::blur`：完全失焦后 gpui 的 `focus_next` 没有起点，Tab 会失效。
    background_focus: FocusHandle,
}

impl AppRoot {
    /// 点击终端**之外**时，把焦点从终端拿走（gpui 不会因点击非可获焦区域而移焦）。
    ///
    /// 只在「当前焦点是某个终端」时动手：点击落在终端里时 `TerminalView` 已
    /// `stop_propagation`；点击处控件自己获焦（对话框输入框等）时这里也不抢。
    /// 细节与实测见 `docs/terminal-architecture.md` §4.3「焦点归属」。
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
        let mut this = Self {
            terminals: Vec::new(),
            active: 0,
            sidebar_visible: true,
            sidebar_view: SidebarView::Sessions,
            right_sidebar_visible: true,
            tab_scroll_handle: ScrollHandle::new(),
            hovered_tab: None,
            resize_state: cx.new(|_| ResizableState::default()),
            right_resize_state: cx.new(|_| ResizableState::default()),
            sidebar_width: SIDEBAR_DEFAULT_WIDTH,
            right_sidebar_width: RIGHT_SIDEBAR_DEFAULT_WIDTH,
            main_split_width: None,
            right_split_width: None,
            settings_window: None,
            monitor: SystemMonitor::new(),
            background_focus: cx.focus_handle(),
        };
        this.spawn_terminal(window, cx);
        // 启动状态栏指标采样（CPU / 内存 / 网络），窗口存活期间持续运行。
        Self::start_metrics_sampling(cx);
        this
    }

    /// 确保两侧边栏保持各自的「期望宽度」，宽度变化全部由中间那一列吸收。
    ///
    /// 分栏容器在容器尺寸变化时会把**所有**面板按比例重排（
    /// `ResizableState::adjust_to_container_size`）——于是缩放窗口、折叠另一侧边栏
    /// 都会把侧边栏一起带宽 / 带窄。这里发现容器宽度与上次不同（即刚重排过）
    /// 就立刻 `resize_panel` 钉回 [`AppRoot::sidebar_width`] /
    /// [`AppRoot::right_sidebar_width`]；多出来 / 少掉的空间自然落到中间那一列。
    ///
    /// 两处调用时机很关键：
    /// - **在 [`AppRoot::render`] 开头同步调用**——紧接的布局就会用上钉好的宽度，
    ///   不会先闪一帧错误宽度；
    /// - **只在容器宽度变过时**才动手——拖拽分隔条不改变容器宽度，所以不会和
    ///   用户抢宽度（拖拽结果由 `on_resize` 回调记进上面两个字段）。
    ///
    /// 期望宽度与当前宽度相同时 `ResizableState::resize_panel` 直接返回，
    /// 因此可以每帧调用。
    fn pin_sidebar_widths(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.sidebar_visible {
            let width = self.sidebar_width;
            let container = self.resize_state.read(cx).container_size();
            if self.main_split_width != Some(container) {
                self.main_split_width = Some(container);
                self.resize_state.update(cx, |state, cx| {
                    state.resize_panel(0, width, window, cx);
                });
            }
        }
        if self.right_sidebar_visible {
            let width = self.right_sidebar_width;
            let container = self.right_resize_state.read(cx).container_size();
            if self.right_split_width != Some(container) {
                self.right_split_width = Some(container);
                self.right_resize_state.update(cx, |state, cx| {
                    // 右侧边栏是本组的最后一个面板：`resize_panel` 会通过挤压
                    // 前一个面板（中间列）来让它拿到这个宽度。
                    state.resize_panel(1, width, window, cx);
                });
            }
        }
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

    /// 启动状态栏指标采样任务：每 [`SAMPLE_INTERVAL`](status_metrics::SAMPLE_INTERVAL)
    /// 采样一次并刷新界面，根视图销毁后自动结束。
    fn start_metrics_sampling(cx: &mut Context<Self>) {
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            loop {
                cx.background_executor().timer(SAMPLE_INTERVAL).await;
                // update 返回 Err 说明根视图已销毁，退出循环即可。
                if this
                    .update(cx, |this, cx| this.sample_metrics(cx))
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
    }

    /// 采样当前会话的进程指标；仅在**显示内容变化**时重绘界面。
    ///
    /// 这里用 `this.update`（不需要窗口）而不是 `update_in`：定时任务运行在
    /// 窗口更新之外，无需借用窗口。
    pub(crate) fn sample_metrics(&mut self, cx: &mut Context<Self>) {
        // 无会话（标签页全部关闭）时传 None，CPU / 内存显示为未知。
        let pid = self
            .terminals
            .get(self.active)
            .and_then(|session| session.view.read(cx).pid(cx));
        // 每轮采样都刷新界面：状态栏里的网络速率本就是实时值，每次采样都会变。
        self.monitor.sample(pid);
        cx.notify();
    }
}

impl Render for AppRoot {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // 主体：侧边栏与终端容器由各自的容器模块渲染，两者之间是可拖拽的分隔条。
        // 活动栏已并入底部状态栏（见 `status_bar` 模块），此处不再有左侧竖栏。

        // 侧边栏宽度只由用户拖拽决定：容器尺寸变化带来的按比例重排先钉回去，
        // 免得窗口一变宽侧边栏就跟着变宽（见 `pin_sidebar_widths`）。
        self.pin_sidebar_widths(window, cx);

        // 终端容器：所有标签页关闭后整个容器一起关闭（标签栏 / 终端全部消失）。
        let terminal_container = (!self.terminals.is_empty())
            .then(|| self.render_terminal_container(cx));

        // 中间列 = 终端区（标签页全关时显示欢迎页）+ **公共状态栏**（常驻不消失）。
        // 底部三块状态栏的宽度就是各自列的宽度：两边的状态栏随侧边栏一起宽窄变化
        // （它们在各自的侧边栏容器里，见 `sidebar_panel`），中间这块铺满中间列。
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
                    // 没有会话 = 中间容器整体关闭，改显示欢迎页（`welcome` 模块）。
                    .child(
                        terminal_container
                            .unwrap_or_else(|| self.render_welcome(cx)),
                    ),
            )
            .child(self.render_status_bar(cx))
            .into_any_element();

        // 主体分两级装配（见 `AppRoot::right_resize_state` 的说明：两组面板各自持有
        // 宽度，互不干扰）：
        //   内层 `main-split`：左侧边栏 | 中间列（左侧不可见时中间列直接铺满）
        //   外层 `right-split`：内层 | 右侧边栏（右侧不可见时只渲染内层）
        let center = if self.sidebar_visible {
            h_resizable("main-split")
                // 绑定根视图持有的状态实体：侧边栏折叠再展开后宽度不会丢失。
                .with_state(&self.resize_state)
                // 拖拽结束后记下新宽度，作为窗口尺寸变化时的「期望宽度」。
                .on_resize({
                    let root = cx.entity().downgrade();
                    move |state, _window, cx| {
                        if let Some(width) = state.read(cx).sizes().first().copied() {
                            let _ = root.update(cx, |this, _| this.sidebar_width = width);
                        }
                    }
                })
                .child(
                    resizable_panel()
                        .size(self.sidebar_width)
                        .size_range(SIDEBAR_MIN_WIDTH..SIDEBAR_MAX_WIDTH)
                        // flex_none：宽度完全由面板状态决定，否则组件内置的
                        // flex_grow_1 会把侧边栏撑得比设定值更宽。
                        .flex_none()
                        .child(self.render_sidebar_container(cx)),
                )
                .child(resizable_panel().child(mid_column))
                .into_any_element()
        } else {
            // 折叠态：左侧边栏（含它自己的状态栏）整块不渲染，中间列铺满。
            // 左侧的「展开」按钮此时由中间那条公共状态栏提供（见 `render_status_bar`）。
            mid_column
        };

        let body = if self.right_sidebar_visible {
            h_resizable("right-split")
                .with_state(&self.right_resize_state)
                // 拖拽结束后记下新宽度（右侧边栏是本组的最后一个面板，下标 1）。
                .on_resize({
                    let root = cx.entity().downgrade();
                    move |state, _window, cx| {
                        if let Some(width) = state.read(cx).sizes().get(1).copied() {
                            let _ = root.update(cx, |this, _| this.right_sidebar_width = width);
                        }
                    }
                })
                // 左栏（左侧边栏 + 中间列）撑满剩余宽度，右栏宽度完全由面板状态决定。
                .child(resizable_panel().child(center))
                .child(
                    resizable_panel()
                        .size(self.right_sidebar_width)
                        .size_range(RIGHT_SIDEBAR_MIN_WIDTH..RIGHT_SIDEBAR_MAX_WIDTH)
                        .flex_none()
                        .child(self.render_right_sidebar_container(cx)),
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
            // —— 顶部：自绘标题栏（图标 / 标题 / 设置入口 / 窗口控制）——
            // 标题栏内容区本身就是窗口拖拽区，但其中的按钮仍有自己的 hitbox，
            // 点击会被正常派发（与窗口控制按钮同理），因此「设置」可以放在这里。
            .child(
                TitleBar::new().child(
                    h_flex()
                        .w_full()
                        .px(px(8.))
                        .gap(px(8.))
                        .items_center()
                        .child(
                            div()
                                .flex_1()
                                .overflow_hidden()
                                .text_size(px(13.))
                                .text_color(cx.theme().secondary_foreground)
                                .child("Alacrterm"),
                        )
                        .child(
                            // 设置入口：文字态按钮（无边框无底色，hover 提亮），
                            // 靠 flex_1 的标题占位顶到内容区最右端、窗口控制按钮左侧。
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

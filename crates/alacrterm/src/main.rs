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
//! - [`status_metrics`]   —— 状态栏指标采样（连接状态 / CPU / 内存 / 网络）
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
//! **终端标签页全部关闭**时中间容器消失，中间列改显示欢迎页（[`welcome`]，
//! 见 [`AppRoot::render_welcome`]；之后可从欢迎页的「新建终端」或标签栏的 `+`
//! 重新开一个本地终端，或双击侧边栏里的会话记录连远端）。这些情况都**不影响底部状态栏**
//! （三条状态栏常驻），而侧边栏的「折叠 / 展开」开关与「设置」入口都在**标题栏**
//! （右端 / 左端）：标题栏永远在，因此不会出现「窗口全空、没有任何恢复入口」的死角。

mod actions;
mod assets;
mod config;
#[cfg(windows)]
mod conpty_backend;
mod dialog;
mod settings_window;
mod sidebar_panel;
mod status_bar;
mod status_metrics;
mod tab_bar;
mod terminal_panel;
mod welcome;

use gpui::{
    App, AppContext as _, AsyncApp, Bounds, Context, Entity, FocusHandle, InteractiveElement as _,
    IntoElement, KeyBinding, MouseButton, MouseDownEvent, ParentElement as _, Pixels, Render,
    SharedString, Styled as _, Subscription, WeakEntity, Window, WindowBounds, WindowHandle,
    WindowOptions, div, px, size,
};
use gpui_kit::{
    QuitMode,
    component::{
        ActiveTheme as _, Root, Sizable as _, ThemeMode, TitleBar,
        button::{Button, ButtonVariants as _},
        dock::{DockArea, DockEvent, TabGroup},
        h_flex,
        resizable::{ResizableState, h_resizable, resizable_panel},
        tree::{TreeState},
        v_flex,
    },
};
use sidebar_panel::{
    RIGHT_SIDEBAR_DEFAULT_WIDTH, RIGHT_SIDEBAR_MAX_WIDTH, RIGHT_SIDEBAR_MIN_WIDTH,
    SIDEBAR_DEFAULT_WIDTH, SIDEBAR_MAX_WIDTH, SIDEBAR_MIN_WIDTH, SidebarTabs, SidebarView,
};
use sidebar_panel::sessions::{SessionEntry, SessionPath};
use status_metrics::{SAMPLE_INTERVAL, SystemMonitor};
use tab_bar::TerminalDockSkin;
use terminal_panel::Session;

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
            // 配置分三步（顺序不能换）：
            //   ① `install` 解析 `config/terminal.json` + `config/app.json` 装进全局；
            //   ② `load_themes` 把 `themes/` 登记成主题库（含热重载）；
            //   ③ `apply_saved_themes` 按配置把主题挂到两个槽位上（不写回、不投影）。
            // 没有配置文件时全用默认：gpui-kit 内置主题 + `RenderSettings::default()`。
            config::install(cx);
            config::load_themes(cx);
            config::apply_saved_themes(cx);
            // 终端为深色背景，应用主题跟随使用暗色（压覆盖的细节见 `config::change_theme`）。
            config::change_theme(ThemeMode::Dark, cx);

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
                    // 标准快捷键：`Ctrl+,` 打开设置（与 VS Code / Zed 一致）。
                    // 绑在 `None` context 上 ⇒ 焦点在终端里也能触发（键位派发在
                    // key listener 之前，见 `TerminalView` 里 tab 的同类说明）。
                    cx.bind_keys([KeyBinding::new("ctrl-,", actions::OpenSettings, None)]);
                    cx.new(|cx| Root::new(root, window, cx))
                },
            )
            .expect("failed to open window");
        });
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
    /// 侧边栏「会话」列表的**条目树**（文件夹 / 记录，见 [`SessionEntry`]）。
    ///
    /// 顺序 = 列表里的显示顺序。只有侧边栏状态栏的 `+` / 右键菜单会往这里加条目；
    /// 条目与 [`AppRoot::terminals`] **没有对应关系**（双击记录才按它开一个终端）。
    session_entries: Vec<SessionEntry>,
    /// 左侧边栏是否可见（只由标题栏右端的折叠开关切换；顶部标签不会改变它）。
    sidebar_visible: bool,
    /// 左侧边栏的标签（顺序 + 当前选中）；标签可以拖到右栏或在本栏内换位。
    left_tabs: SidebarTabs,
    /// 展开着的文件夹路径。
    ///
    /// 会话树的展开状态**本可以**存在 `TreeItem` 内部（共享的 `Rc<RefCell<..>>`），
    /// 但条目树每次同步都会重建 `TreeItem` ⇒ 那样会每刷一次就全部收起。
    /// 所以这里自己留一份：同步时按它给 `TreeItem::expanded(..)` 赋值，
    /// 同时它也是[【算行数】](AppRoot::visible_session_rows)的依据
    /// （树的高度必须手算，见 [`crate::sidebar_panel`]）。
    expanded_folders: Vec<SessionPath>,
    /// 「会话」树的交互状态（选中 / 滚动 / 键盘导航）。
    session_tree: Entity<TreeState>,
    /// 上一次同步树用的内容签名（条目类型 + 名字，含被收起的分支）：
    /// 变了才重建 items（`set_items` 会 notify，每帧无条件调用会自激）。
    ///
    /// ⚠️ `None` = 还没同步过：不能用「空 `Vec`」同时表达「没同步过」与「一条都没有」——
    /// 后者会让首次同步被当成「签名没变」跳掉，树就永远拿不到 items。
    session_tree_sig: Option<Vec<(SharedString, bool, usize)>>,
    /// 会话树的事件订阅：[`TreeEvent`](gpui_kit::component::tree::TreeEvent)
    /// （用户展开 / 收起文件夹）⇒ 记进 [`AppRoot::expanded_folders`] 并重绘
    /// （行数变了，树的高度要重算）。
    session_tree_sub: Option<Subscription>,
    /// 右侧边栏是否可见（由标题栏右端的折叠开关切换）。
    right_sidebar_visible: bool,
    /// 右侧边栏的标签（含义同 [`AppRoot::left_tabs`]）。
    right_tabs: SidebarTabs,
    /// 终端会话的 dock（[`crate::terminal_panel`]）：一个会话 = center 里的一块面板。
    ///
    /// 左右侧边栏**不在 dock 里**（仍是下面的分栏组）；没有会话时整块 dock 换成欢迎页。
    dock: Entity<DockArea>,
    /// dock 布局变化的订阅：会话表顺序 / 成员都要跟着 dock 走。
    ///
    /// 事件在 effect 阶段回调，那时 dock 的更新已结束，可以直接 `read` 它。
    dock_layout_sub: Option<Subscription>,
    /// 「下一个新建的会话放进哪个标签组」（标签栏 `+` 按钮设置，用一次即清空）：
    /// `add_panel_view` 只会塞进第一个标签组，靠它才能落回用户点的那一组。
    pending_session_group: Option<WeakEntity<TabGroup>>,
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
        let dock = Self::build_dock(window, cx);
        let mut this = Self {
            terminals: Vec::new(),
            active: 0,
            session_entries: Vec::new(),
            sidebar_visible: true,
            // 默认：左栏是「会话」，右栏是「会话信息」（都可以拖动改变）。
            left_tabs: SidebarTabs::new(vec![SidebarView::Sessions]),
            expanded_folders: Vec::new(),
            session_tree: cx.new(|cx| TreeState::new(cx)),
            session_tree_sig: None,
            session_tree_sub: None,
            right_sidebar_visible: true,
            right_tabs: SidebarTabs::new(vec![SidebarView::SessionInfo]),
            dock,
            dock_layout_sub: None,
            pending_session_group: None,
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
        this.spawn_terminal(window, cx);
        // 文件夹展开 / 收起会改变「树一共多少行」（高度按行数算），
        // 所以树的事件要立刻回传到根视图。
        this.session_tree_sub = Some(cx.subscribe(&this.session_tree, |root, _, event, cx| {
            root.on_session_tree_event(event, cx);
        }));
        // 启动状态栏指标采样（CPU / 内存 / 网络），窗口存活期间持续运行。
        Self::start_metrics_sampling(cx);
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
        // 主体：侧边栏与终端由各自的模块渲染，中间是可拖拽分隔条（视图标签在侧边栏顶部）。

        // 会话表变了就同步进会话树（`TreeState` 是快照，必须在渲染前对齐，见该方法文档）。
        self.sync_session_tree(cx);

        // 侧边栏宽度只由用户拖拽决定：容器尺寸变化引起的比例重排先钉回去。
        self.pin_sidebar_widths(window, cx);

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
            .child(self.render_status_bar(cx))
            .into_any_element();

        // 两级嵌套分栏（两组面板各自持有宽度，互不干扰）：
        //   main-split = 左侧边栏 | 中间列；right-split = 内层 | 右侧边栏
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
                        // 内容区最右端、窗口控制按钮左侧。两枚按钮各自包了 `occlude`。
                        .child(self.render_sidebar_toggles(cx)),
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

//! 基于 gpui-kit (gpui-component 0.6) 外壳 + alacrterm 终端核心的终端应用。
//!
//! 架构（方案 A，移植自 https://github.com/zephyraluco/alacrterm）：
//! - `crates/terminal`      —— 终端仿真核心（alacritty_terminal 0.26 + PTY + 事件循环）
//! - `crates/terminal_view` —— 终端视图（自定义 gpui Element 逐 cell 渲染）
//! - `crates/util`          —— Shell 探测 / 路径工具（来自 Zed）
//!
//! 应用外壳按「左右两个容器」拆分为独立文件：
//! - [`sidebar_panel`]  —— 左侧容器：活动栏 + 侧边栏 + 侧边栏状态栏
//! - [`terminal_panel`] —— 右侧容器：标签栏 + 终端 + 终端状态栏
//! - [`connection_dialog`] —— 「新建终端」建连对话框（IP / 端口 / 名称 / 用户名 / 密码）
//! - [`settings_window`]  —— 独立的设置窗口（非对话框）
//! - [`status_metrics`]   —— 状态栏指标采样（连接状态 / CPU / 内存 / 网络）
//!
//! 本文件只保留程序入口、根视图 [`AppRoot`]（共享状态 + 布局装配 + 设置弹窗）。
//! 两个容器之间是可拖拽的分隔条（官方 resizable 面板组，左右拖动调整侧边栏宽度），
//! 标题栏（TitleBar）与弹窗层也在根视图中装配。
//!
//! 布局装配遵循两条规则：侧边栏折叠、或**终端标签页全部关闭**时，
//! 对应的容器与分隔条一并消失（终端容器关闭后仅剩左侧容器与空白背景，
//! 可随时从侧边栏会话条目的右键菜单「新建终端」重新打开）。

mod actions;
mod assets;
mod connection_dialog;
mod settings_window;
mod sidebar_panel;
mod status_metrics;
mod terminal_panel;

use gpui::{
    App, AppContext as _, AsyncApp, Bounds, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, ScrollHandle, SharedString, Styled as _, WeakEntity, Window,
    WindowBounds, WindowHandle, WindowOptions, div, px, size,
};
use gpui_kit::{
    QuitMode,
    component::{
        ActiveTheme as _, Root, Theme, ThemeMode, TitleBar, h_flex,
        resizable::{ResizableState, h_resizable, resizable_panel},
        v_flex,
    },
};
use terminal_view::TerminalView;
use util::shell::Shell;

use sidebar_panel::{SIDEBAR_DEFAULT_WIDTH, SIDEBAR_MAX_WIDTH, SIDEBAR_MIN_WIDTH, SidebarView};
use status_metrics::{SAMPLE_INTERVAL, SystemMonitor};

fn main() {
    gpui_kit::application()
        // 注册自有资产源（alacrterm assets.rs 方式）：本 crate 的 assets/icons 目录
        // 经 rust-embed 嵌入，`crate::assets::IconName` 由 icon_named! 宏扫描生成，
        // 可自由增删图标文件。
        .with_assets(assets::Assets)
        .with_quit_mode(QuitMode::LastWindowClosed)
        .run(|cx: &mut App| {
            gpui_kit::init(cx);
            // 终端为深色背景，应用主题跟随使用暗色。
            Theme::change(ThemeMode::Dark, None, cx);

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
    /// 侧边栏是否可见（点击活动栏当前视图图标可隐藏 / 显示）。
    sidebar_visible: bool,
    /// 侧边栏当前视图（由活动栏图标切换）。
    sidebar_view: SidebarView,
    /// 侧边栏状态栏展示的 shell 程序名（如 pwsh.exe）。
    shell_name: SharedString,
    /// 标签栏滚动句柄：跟踪 tabs 横向滚动，激活会话时把选中标签滚入可视区。
    tab_scroll_handle: ScrollHandle,
    /// 侧边栏 / 终端分栏面板组的共享状态。
    ///
    /// 实体由根视图持有（而非交给组件内部的 keyed state），这样侧边栏折叠再展开、
    /// 乃至窗口重绘后，用户拖出来的宽度都不会丢失。
    resize_state: Entity<ResizableState>,
    /// 设置窗口的句柄（见 [`AppRoot::open_settings_window`]）。
    ///
    /// 用于「重复点击设置图标只激活已有窗口」；窗口被关闭后该句柄会失效，
    /// 下一次点击会重新开窗并覆盖它。
    settings_window: Option<WindowHandle<Root>>,
    /// 状态栏指标采样器（CPU / 内存 / 网络），由后台定时任务驱动。
    pub(crate) monitor: SystemMonitor,
}

impl AppRoot {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let shell_program = Shell::System.program();
        let shell_name = std::path::Path::new(&shell_program)
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or(shell_program);

        let mut this = Self {
            terminals: Vec::new(),
            active: 0,
            sidebar_visible: true,
            sidebar_view: SidebarView::Sessions,
            shell_name: shell_name.into(),
            tab_scroll_handle: ScrollHandle::new(),
            resize_state: cx.new(|_| ResizableState::default()),
            settings_window: None,
            monitor: SystemMonitor::new(),
        };
        this.spawn_terminal(window, cx);
        // 启动状态栏指标采样（CPU / 内存 / 网络），窗口存活期间持续运行。
        Self::start_metrics_sampling(cx);
        this
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
        // 左侧区域（活动栏 + 侧边栏 + 状态栏）与右侧区域（标签栏 + 终端 + 状态栏）
        // 分别由各自的容器模块渲染；两者之间是可拖拽的分隔条。
        let activity_bar = self.render_activity_bar(cx);

        // 终端容器：所有标签页关闭后整个容器一起关闭（标签栏 / 终端 / 状态栏全部消失）。
        let terminal_container = (!self.terminals.is_empty())
            .then(|| self.render_terminal_container(cx));

        // 侧边栏可见时始终使用分栏面板组，两栏之间保留可拖拽的分隔条；
        // 终端容器关闭后右栏退化为空白占位——**仍保留两栏结构**，因为
        // 单面板的分栏组会被 `adjust_to_container_size` 按比例拉伸到容器满宽，
        // 那样既没有分隔条、侧边栏也会横向铺满整个窗口。
        let body = if self.sidebar_visible {
            h_resizable("main-split")
                // 绑定根视图持有的状态实体：侧边栏折叠再展开后宽度不会丢失。
                .with_state(&self.resize_state)
                .child(
                    resizable_panel()
                        .size(SIDEBAR_DEFAULT_WIDTH)
                        .size_range(SIDEBAR_MIN_WIDTH..SIDEBAR_MAX_WIDTH)
                        // flex_none：宽度完全由面板状态决定，否则组件内置的
                        // flex_grow_1 会把侧边栏撑得比设定值更宽。
                        .flex_none()
                        .child(self.render_sidebar_container(cx)),
                )
                .child(match terminal_container {
                    Some(terminal) => resizable_panel().child(terminal),
                    None => resizable_panel().child(div()),
                })
                .into_any_element()
        } else {
            terminal_container.unwrap_or_else(|| div().into_any_element())
        };

        v_flex()
            .id("app-root")
            .size_full()
            .bg(cx.theme().background)
            // —— 顶部：自绘标题栏（图标 / 标题 / 窗口控制）——
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
                        ),
                ),
            )
            // —— 中部：左侧区域（活动栏 + 侧边栏 + 状态栏）+ 右侧区域（终端容器）——
            // 活动栏固定宽度、直到底部，不参与分栏拖拽，折叠侧边栏后靠它恢复。
            .child(
                h_flex()
                    .flex_1()
                    .overflow_hidden()
                    .child(activity_bar)
                    // 分栏组内部使用 size_full，需要这层 flex_1 容器提供「剩余宽度」，
                    // 否则其 100% 宽会把活动栏的 44px 也算进去而溢出。
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .overflow_hidden()
                            .child(body),
                    ),
            )
            // —— 覆盖层：gpui-kit 0.6 的 Root 不会自动渲染 Dialog 层，
            //    必须在渲染树中显式挂载（参考官方 ai_recipes 示例），否则弹窗不显示。——
            .children(Root::render_dialog_layer(window, cx))
    }
}

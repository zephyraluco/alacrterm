//! 「新建终端」连接对话框：收集 IP / 端口 / 名称 / 用户名 / 密码后创建会话。
//!
//! 由侧边栏会话条目的右键菜单「新建终端」触发（`NewTerminal` action →
//! [`AppRoot::open_new_terminal_dialog`]）。
//!
//! 行为约定：
//! - **填了 IP** → 用 `russh` 直连远端主机（不调用系统 `ssh` 客户端，见
//!   `crates/terminal/src/ssh.rs`）；密码字段就是这次连接的认证凭据；
//! - **IP 留空** → 启动本地系统 shell（等同于原来的「新建终端」）；
//! - **名称** 作为会话显示名（标签页 / 侧边栏 / 状态栏），留空时 SSH 会话回退到
//!   `user@host`、本地会话回退到终端自身标题；
//! - **用户名** 留空时取本机当前用户名（与 OpenSSH 的默认行为一致）。
//!
//! 认证顺序由后端决定：填了密码先用密码，否则先试免密、再依次试 `~/.ssh` 下的
//! 常用私钥；主机密钥采用 TOFU（首次连接记入 `known_hosts`，之后不一致即拒绝）。

use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, IntoElement, ParentElement as _,
    SharedString, Styled as _, Window, div, px,
};
use gpui_kit::component::{
    ActiveTheme as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    dialog::{DialogAction, DialogClose, DialogFooter},
    h_flex,
    input::{Input, InputState},
    v_flex,
};
use terminal::SshOptions;
use util::shell::Shell;

use crate::{AppRoot, SessionRequest};

/// 端口留空时使用的默认 SSH 端口。
const DEFAULT_SSH_PORT: &str = "22";

/// 建连表单的状态：五个输入框（实体长期存活，保证对话框重绘时输入不丢失）。
#[derive(Clone)]
pub(crate) struct ConnectionForm {
    host: Entity<InputState>,
    port: Entity<InputState>,
    name: Entity<InputState>,
    user: Entity<InputState>,
    password: Entity<InputState>,
}

impl ConnectionForm {
    /// 创建表单状态。输入框实体在此一次性创建——对话框的构建闭包是 `Fn`
    /// （每帧都会被调用），若在闭包内创建就会每帧重置输入。
    pub(crate) fn new(window: &mut Window, cx: &mut Context<AppRoot>) -> Self {
        let host = cx.new(|cx| InputState::new(window, cx).placeholder("例如：192.168.1.10"));
        let port = cx.new(|cx| InputState::new(window, cx).default_value(DEFAULT_SSH_PORT));
        let name = cx.new(|cx| InputState::new(window, cx).placeholder("例如：生产服务器"));
        let user = cx.new(|cx| InputState::new(window, cx).placeholder("例如：root"));
        // masked(true) 只影响渲染（显示为圆点），value() 仍能取到明文。
        let password = cx.new(|cx| InputState::new(window, cx).masked(true));

        Self {
            host,
            port,
            name,
            user,
            password,
        }
    }

    /// 去掉首尾空白后的输入值。
    fn trimmed(&self, input: &Entity<InputState>, cx: &App) -> String {
        input.read(cx).value().trim().to_string()
    }

    /// 依据表单内容得出「建连请求」。
    ///
    /// 名称留空返回 `None`：SSH 会话由 [`crate::AppRoot::spawn_session`] 回退到
    /// `user@host`，本地会话回退到终端自身标题。
    pub(crate) fn build(&self, cx: &App) -> SessionRequest {
        let name = self.trimmed(&self.name, cx);
        let host = self.trimmed(&self.host, cx);
        let port_text = self.trimmed(&self.port, cx);
        let user = self.trimmed(&self.user, cx);
        let password = self.trimmed(&self.password, cx);

        let name = (!name.is_empty()).then(|| SharedString::from(name));

        // IP 留空 → 本地系统 shell；否则走 russh 直连。
        if host.is_empty() {
            return SessionRequest::Local {
                name,
                shell: Shell::System,
            };
        }

        // 端口非法（空 / 非数字 / 0）时回退到 22：SSH 端口没有合理的自动纠正手段，
        // 静默回退比拒绝建连更符合这里的预期。
        let port: u16 = port_text.parse().unwrap_or_default();
        let port = if port == 0 {
            log::warn!("端口 {port_text:?} 不是合法端口，回退到 {DEFAULT_SSH_PORT}");
            DEFAULT_SSH_PORT.parse().unwrap_or(22)
        } else {
            port
        };

        SessionRequest::Ssh {
            name,
            options: SshOptions::new(
                host,
                port,
                if user.is_empty() {
                    current_username()
                } else {
                    user
                },
                (!password.is_empty()).then_some(password),
            ),
        }
    }

    /// 渲染表单字段。
    fn render_fields(&self, cx: &mut App) -> AnyElement {
        // 顺序按需求给出：IP、端口、名称、用户名、密码。
        // IP 与端口同排一行，端口用固定窄宽度。
        let host_port_row = h_flex()
            .gap_3()
            .items_end()
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_1()
                    .child(field_label("IP 地址", cx))
                    .child(Input::new(&self.host)),
            )
            .child(
                v_flex()
                    .w(px(96.))
                    .flex_shrink_0()
                    .gap_1()
                    .child(field_label("端口", cx))
                    .child(Input::new(&self.port)),
            );

        v_flex()
            .gap_3()
            .child(host_port_row)
            .child(field("名称", &self.name, cx))
            .child(field("用户名", &self.user, cx))
            .child(field("密码", &self.password, cx))
            .into_any_element()
    }
}

/// 用户名为空时使用的默认值：本机当前用户名（与 OpenSSH 的默认行为一致）。
fn current_username() -> String {
    std::env::var("USERNAME")
        .or_else(|_| std::env::var("USER"))
        .unwrap_or_else(|_| "root".to_string())
}

/// 单个字段：标题 + 输入框。
fn field(label: &'static str, input: &Entity<InputState>, cx: &App) -> AnyElement {
    v_flex()
        .gap_1()
        .child(field_label(label, cx))
        .child(Input::new(input))
        .into_any_element()
}

/// 字段标题（弱化色小字）。
fn field_label(text: &'static str, cx: &App) -> AnyElement {
    div()
        .text_sm()
        .text_color(cx.theme().muted_foreground)
        .child(text)
        .into_any_element()
}

impl AppRoot {
    /// 右键菜单「新建终端」的入口：弹出建连对话框。
    pub(crate) fn open_new_terminal_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let form = ConnectionForm::new(window, cx);
        // 对话框回调只有 &mut App，拿不到 AppRoot；用弱引用回到根视图创建会话。
        let root = cx.entity().downgrade();

        window.open_dialog(cx, move |dialog, _window, cx| {
            let form_for_ok = form.clone();
            let root_for_ok = root.clone();

            // 底部按钮：`Dialog::render` **不会**自动生成确定/取消按钮（`button_props`
            // 只被 AlertDialog 使用），必须用 `.footer(...)` 自己给出。
            // `DialogClose` / `DialogAction` 会分别派发 Cancel / Confirm action，
            // 从而触发下面注册的 `on_ok` / `on_cancel` 回调。
            let footer = DialogFooter::new()
                .child(DialogClose::new().child(Button::new("cancel").label("取消")))
                .child(
                    DialogAction::new().child(Button::new("confirm").label("连接").primary()),
                );

            dialog
                .title("新建终端")
                .w(px(420.))
                .footer(footer)
                // 返回 true 让对话框关闭。
                .on_ok(move |_, _, cx| {
                    // `on_ok` 是 `Fn`（可多次调用），因此每次调用都克隆一份表单。
                    let form = form_for_ok.clone();
                    // 动作回调执行期间窗口仍在更新栈上，直接 update_in 会失败
                    // （"entity has no current window"），交给 `defer_after_update`
                    // 让出一拍再创建会话。
                    AppRoot::defer_after_update(
                        root_for_ok.clone(),
                        cx,
                        move |this, window, cx| {
                            this.spawn_session(form.build(cx), window, cx);
                        },
                    );
                    true
                })
                .child(form.render_fields(cx))
        });
    }
}

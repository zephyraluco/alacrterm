//! 「新建会话」对话框：收集 SSH 连接参数（IP / 端口 / 名称 / 用户名 / 密码）后，
//! **只往侧边栏的会话列表里加一条记录**（见 [`crate::sidebar_panel::sessions::SessionRecord`]）。
//!
//! 由侧边栏「会话」栏底部状态栏**右下角的 `+`** 触发（`NewSession` action →
//! [`AppRoot::open_new_session_dialog`]），新记录落在**顶层**；文件夹行的右键菜单
//! 「在这里新建会话」走同一个入口，只是带上 `folder` 参数（落进那个文件夹）。
//!
//! 行为约定：
//! - **只支持 SSH**：IP、名称、用户名三项必填（任一为空时「添加」按钮禁用，
//!   `on_ok` 里再兜一次校验），端口留空用 22；
//! - **不打开终端**：本对话框只落一条记录，双击侧边栏里那条记录才真正开终端
//!   （[`AppRoot::open_session_record`]）。这两件事刻意分开——记录是配置，
//!   终端是运行实例，同一条记录可以开多个终端；
//! - 因此也不需要「IP 留空 = 本地终端」那种兜底：本地终端请用标签栏的 `+`
//!   （[`NewLocalTerminal`](crate::actions::NewLocalTerminal)）。
//!
//! 关于密码字段：本机 `ssh` 客户端不接受命令行传入的密码（这是 ssh 的刻意设计），
//! 因此这里只做输入与掩码，**不随记录保存**；实际认证仍需在终端里按提示交互输入。
//! 后续若要真正免交互登录，需要改用 SSH 库或 `sshpass` 之类的辅助程序。

use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, IntoElement, ParentElement as _,
    SharedString, Styled as _, Window, div, px,
};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    dialog::{DialogAction, DialogClose, DialogFooter},
    h_flex,
    input::{Input, InputState},
    v_flex,
};

use crate::sidebar_panel::sessions::{SessionPath, SessionRecord};
use crate::AppRoot;

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

    /// 表单是否可提交：IP、名称、用户名三项必填（端口可空 → 用默认端口）。
    ///
    /// 按钮的禁用态与 `on_ok` 里的兜底校验都读它，两处永远一致。
    pub(crate) fn is_valid(&self, cx: &App) -> bool {
        !self.trimmed(&self.host, cx).is_empty()
            && !self.trimmed(&self.name, cx).is_empty()
            && !self.trimmed(&self.user, cx).is_empty()
    }

    /// 依据表单内容生成一条会话记录；必填项缺失时返回 `None`。
    pub(crate) fn build(&self, cx: &App) -> Option<SessionRecord> {
        if !self.is_valid(cx) {
            return None;
        }
        let port = self.trimmed(&self.port, cx);
        Some(SessionRecord {
            name: SharedString::from(self.trimmed(&self.name, cx)),
            user: self.trimmed(&self.user, cx),
            host: self.trimmed(&self.host, cx),
            port: if port.is_empty() {
                DEFAULT_SSH_PORT.to_string()
            } else {
                port
            },
        })
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
            .child(field_hint(self.is_valid(cx), cx))
            .into_any_element()
    }
}

/// 表单下方的说明行：必填项缺失时给一句红字提示，齐全时留一句中性说明。
///
/// 「添加」按钮的禁用态已经说明了问题，但用户未必知道缺哪一项，所以这里补一句。
fn field_hint(valid: bool, cx: &App) -> AnyElement {
    let (text, color) = if valid {
        (
            "会话只是一条记录，双击列表里的条目才打开终端",
            cx.theme().muted_foreground,
        )
    } else {
        ("IP、名称、用户名不能为空", cx.theme().danger)
    };
    div()
        .text_xs()
        .text_color(color)
        .child(text)
        .into_any_element()
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
    /// 侧边栏状态栏 `+` / 文件夹右键菜单的入口：弹出「新建会话」对话框。
    ///
    /// `folder` = 新记录落在哪个文件夹里（`None` = 顶层，见 [`SessionPath`]）；
    /// 无论哪种情况都**只加记录、不开终端**。
    pub(crate) fn open_new_session_dialog(
        &mut self,
        folder: Option<SessionPath>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let form = ConnectionForm::new(window, cx);
        // 对话框回调只有 &mut App，拿不到 AppRoot；用弱引用回到根视图加记录。
        let root = cx.entity().downgrade();

        window.open_dialog(cx, move |dialog, _window, cx| {
            let form_for_ok = form.clone();
            let root_for_ok = root.clone();
            let folder_for_ok = folder.clone();
            // 必填项是否齐全：驱动「添加」的禁用态与下方的提示行。
            let valid = form.is_valid(cx);

            // 底部按钮：`Dialog::render` **不会**自动生成确定/取消按钮（`button_props`
            // 只被 AlertDialog 使用），必须用 `.footer(...)` 自己给出。
            // `DialogClose` / `DialogAction` 会分别派发 Cancel / Confirm action，
            // 从而触发下面注册的 `on_ok` / `on_cancel` 回调。
            let footer = DialogFooter::new()
                .child(DialogClose::new().child(Button::new("cancel").label("取消")))
                .child(DialogAction::new().child(
                    Button::new("confirm")
                        .label("添加")
                        .primary()
                        .disabled(!valid),
                ));

            dialog
                .title("新建会话")
                .w(px(420.))
                .footer(footer)
                .on_ok(move |_, _, cx| {
                    // `on_ok` 是 `Fn`（可多次调用），因此每次调用都克隆一份表单。
                    let form = form_for_ok.clone();
                    // 兜底校验（禁用态拦不住回车等提交路径）：缺必填项就不关对话框。
                    let Some(record) = form.build(cx) else {
                        return false;
                    };
                    let folder = folder_for_ok.clone();
                    // 动作回调执行期间窗口仍在更新栈上，直接 update_in 会失败
                    // （"entity has no current window"），交给 `defer_after_update` 让出一拍。
                    AppRoot::defer_after_update(root_for_ok.clone(), cx, move |this, _, cx| {
                        this.add_session_record(record, folder, cx);
                    });
                    // 返回 true 让对话框关闭。
                    true
                })
                .child(form.render_fields(cx))
        });
    }
}

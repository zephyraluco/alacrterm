//! 「新建会话」对话框：收集 SSH 连接参数（IP / 端口 / 名称 / 用户名 / 密码），
//! **只往会话列表里加一条记录、不开终端**（见 [`crate::sidebar_panel::sessions::SessionRecord`]）。
//!
//! `folder` = 记录落在哪个文件夹下（`None` = 顶层）。IP、名称、用户名必填，端口留空用 22；
//! 密码只做输入与掩码，**不随记录保存**（认证在终端里交互完成）。

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

/// 建连表单：五个输入框。
#[derive(Clone)]
pub(crate) struct ConnectionForm {
    host: Entity<InputState>,
    port: Entity<InputState>,
    name: Entity<InputState>,
    user: Entity<InputState>,
    password: Entity<InputState>,
}

impl ConnectionForm {
    /// 创建表单状态（输入框实体只在此创建一次：对话框的构建闭包每帧都会被调用）。
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

    /// 表单是否可提交：IP、名称、用户名三项必填（端口可空 ⇒ 用默认端口）。
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

/// 表单下方的说明行：必填项缺失时红字提示，齐全时一句中性说明。
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
    /// 弹出「新建会话」对话框；`folder` = 新记录落在哪个文件夹里（`None` = 顶层），
    /// 只加记录、不开终端。
    pub(crate) fn open_new_session_dialog(
        &mut self,
        folder: Option<SessionPath>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let form = ConnectionForm::new(window, cx);
        let sessions = self.sessions.clone();

        window.open_dialog(cx, move |dialog, _window, cx| {
            let form_for_ok = form.clone();
            let sessions_for_ok = sessions.clone();
            let folder_for_ok = folder.clone();
            // 驱动「添加」的禁用态与下方的提示行。
            let valid = form.is_valid(cx);

            // 页脚自拼：`Dialog::render` 不自动生成确定/取消按钮；`DialogClose` / `DialogAction`
            // 分别派发 Cancel / Confirm，触发 `on_ok` / `on_cancel`。
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
                    let form = form_for_ok.clone();
                    // 兜底校验：缺必填项就不关对话框。
                    let Some(record) = form.build(cx) else {
                        return false;
                    };
                    let folder = folder_for_ok.clone();
                    // 只加记录，不建终端（不需要窗口，无需 defer）。
                    sessions_for_ok.update(cx, |state, cx| state.add_record(record, folder, cx));
                    true
                })
                .child(form.render_fields(cx))
        });
    }
}

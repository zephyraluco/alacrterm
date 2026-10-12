//! 「新建会话」对话框：收集 SSH 连接参数，只往会话列表加一条记录、不开终端。

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

const DEFAULT_SSH_PORT: u16 = 22;

/// 建连表单。
#[derive(Clone)]
pub(crate) struct ConnectionForm {
    host: Entity<InputState>,
    port: Entity<InputState>,
    name: Entity<InputState>,
    user: Entity<InputState>,
    password: Entity<InputState>,
}

impl ConnectionForm {
    pub(crate) fn new(window: &mut Window, cx: &mut Context<AppRoot>) -> Self {
        let host = cx.new(|cx| InputState::new(window, cx).placeholder("例如：192.168.1.10"));
        let port = cx.new(|cx| InputState::new(window, cx).default_value(DEFAULT_SSH_PORT.to_string()));
        let name = cx.new(|cx| InputState::new(window, cx).placeholder("例如：生产服务器"));
        let user = cx.new(|cx| InputState::new(window, cx).placeholder("例如：root"));
        let password = cx.new(|cx| InputState::new(window, cx).masked(true));

        Self {
            host,
            port,
            name,
            user,
            password,
        }
    }

    /// 去掉首尾空白的输入值。
    fn trimmed(&self, input: &Entity<InputState>, cx: &App) -> String {
        input.read(cx).value().trim().to_string()
    }

    /// 端口：留空用默认端口。
    fn port(&self, cx: &App) -> Option<u16> {
        let port = self.trimmed(&self.port, cx);
        if port.is_empty() {
            return Some(DEFAULT_SSH_PORT);
        }
        port.parse::<u16>().ok().filter(|port| *port > 0)
    }

    /// 表单是否可提交。
    pub(crate) fn is_valid(&self, cx: &App) -> bool {
        self.port(cx).is_some()
            && !self.trimmed(&self.host, cx).is_empty()
            && !self.trimmed(&self.name, cx).is_empty()
            && !self.trimmed(&self.user, cx).is_empty()
    }

    /// 依据表单内容生成一条会话记录。
    pub(crate) fn build(&self, cx: &App) -> Option<SessionRecord> {
        if !self.is_valid(cx) {
            return None;
        }
        let password = self.trimmed(&self.password, cx);
        Some(SessionRecord {
            id: 0,
            name: SharedString::from(self.trimmed(&self.name, cx)),
            user: self.trimmed(&self.user, cx),
            host: self.trimmed(&self.host, cx),
            port: self.port(cx)?,
            password: (!password.is_empty()).then_some(password),
        })
    }

    fn render_fields(&self, cx: &mut App) -> AnyElement {
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
            .child(field_hint(self, cx))
            .into_any_element()
    }
}

/// 表单下方的说明行。
fn field_hint(form: &ConnectionForm, cx: &App) -> AnyElement {
    let (text, color) = if !form.is_valid(cx) {
        if form.port(cx).is_none() {
            ("端口必须是 1~65535 之间的数字", cx.theme().danger)
        } else {
            ("IP、名称、用户名不能为空", cx.theme().danger)
        }
    } else {
        (
            "密码会以明文保存在会话文件中；留空则用 ssh-agent 与 ~/.ssh 里的私钥认证",
            cx.theme().muted_foreground,
        )
    };
    div()
        .text_xs()
        .text_color(color)
        .child(text)
        .into_any_element()
}

fn field(label: &'static str, input: &Entity<InputState>, cx: &App) -> AnyElement {
    v_flex()
        .gap_1()
        .child(field_label(label, cx))
        .child(Input::new(input))
        .into_any_element()
}

/// 字段标题。
fn field_label(text: &'static str, cx: &App) -> AnyElement {
    div()
        .text_sm()
        .text_color(cx.theme().muted_foreground)
        .child(text)
        .into_any_element()
}

impl AppRoot {
    /// 弹出「新建会话」对话框。
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
            let valid = form.is_valid(cx);

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
                    let Some(record) = form.build(cx) else {
                        return false;
                    };
                    let folder = folder_for_ok.clone();
                    sessions_for_ok.update(cx, |state, cx| state.add_record(record, folder, cx));
                    true
                })
                .child(form.render_fields(cx))
        });
    }
}

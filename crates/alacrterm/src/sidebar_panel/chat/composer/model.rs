use super::*;
use agent::provider::{CredentialSource, Protocol};
use gpui_kit::component::{
    WindowExt as _,
    dialog::{DialogAction, DialogClose, DialogFooter},
    input::{Input, InputState},
    select::{SearchableVec, Select, SelectState},
};

#[derive(Clone)]
struct ModelForm {
    initial: Option<ModelProfile>,
    name: Entity<InputState>,
    model: Entity<InputState>,
    url: Entity<InputState>,
    metadata: Entity<InputState>,
    key: Entity<InputState>,
    environment: Entity<InputState>,
    protocol: Entity<SelectState<SearchableVec<SharedString>>>,
}
impl ModelForm {
    fn new(
        initial: Option<ModelProfile>,
        window: &mut Window,
        cx: &mut Context<ChatPanel>,
    ) -> Self {
        let field =
            |value: String, placeholder: &str, window: &mut Window, cx: &mut Context<ChatPanel>| {
                cx.new(|cx| {
                    InputState::new(window, cx)
                        .default_value(value)
                        .placeholder(placeholder)
                })
            };
        let name = field(
            initial.as_ref().map(|m| m.name.clone()).unwrap_or_default(),
            "显示名称",
            window,
            cx,
        );
        let model = field(
            initial
                .as_ref()
                .map(|m| m.model.clone())
                .unwrap_or_default(),
            "API 模型 ID，例如 gpt-4.1",
            window,
            cx,
        );
        let url = field(
            initial
                .as_ref()
                .map(|m| m.base_url.clone())
                .unwrap_or_else(|| "https://api.openai.com/v1".into()),
            "API Base URL",
            window,
            cx,
        );
        let metadata = field(
            initial
                .as_ref()
                .and_then(|m| m.metadata.clone())
                .unwrap_or_default(),
            "可选，例如 openai/o3",
            window,
            cx,
        );
        let key = cx.new(|cx| {
            InputState::new(window, cx)
                .masked(true)
                .placeholder("可选；编辑时留空保留原 Key")
        });
        let environment = field(
            initial
                .as_ref()
                .and_then(|m| {
                    if let CredentialSource::Environment { variable } = &m.credential {
                        Some(variable.clone())
                    } else {
                        None
                    }
                })
                .unwrap_or_default(),
            "可选，例如 OPENAI_API_KEY",
            window,
            cx,
        );
        let protocol = cx.new(|cx| {
            SelectState::new(
                SearchableVec::new(vec![
                    SharedString::from("OpenAI Chat"),
                    "OpenAI Responses".into(),
                    "Anthropic Messages".into(),
                ]),
                None,
                window,
                cx,
            )
        });
        let selected = SharedString::from(
            match initial.as_ref().map(|m| m.protocol).unwrap_or_default() {
                Protocol::OpenaiChat => "OpenAI Chat",
                Protocol::OpenaiResponses => "OpenAI Responses",
                Protocol::AnthropicMessages => "Anthropic Messages",
            },
        );
        protocol.update(cx, |state, cx| {
            state.set_selected_value(&selected, window, cx)
        });
        Self {
            initial,
            name,
            model,
            url,
            metadata,
            key,
            environment,
            protocol,
        }
    }

    fn profile(&self, cx: &App) -> Result<ModelProfile, String> {
        let value = |input: &Entity<InputState>| input.read(cx).value().trim().to_owned();
        let environment = value(&self.environment);
        let key = value(&self.key);
        if !environment.is_empty() && !key.is_empty() {
            return Err("API Key 和环境变量请选择其中一种".into());
        }
        let id = self
            .initial
            .as_ref()
            .map(|m| m.id)
            .unwrap_or_else(agent::types::SessionId::new_v4);
        let credential = if !key.is_empty() {
            CredentialSource::Keychain {
                account: id.to_string(),
            }
        } else if !environment.is_empty() {
            CredentialSource::Environment {
                variable: environment,
            }
        } else {
            self.initial
                .as_ref()
                .map(|m| m.credential.clone())
                .filter(|c| matches!(c, CredentialSource::Keychain { .. }))
                .unwrap_or_default()
        };
        let metadata = value(&self.metadata);
        let profile = ModelProfile {
            id,
            name: value(&self.name),
            model: value(&self.model),
            base_url: value(&self.url),
            protocol: match self
                .protocol
                .read(cx)
                .selected_value()
                .map(SharedString::as_ref)
            {
                Some("OpenAI Responses") => Protocol::OpenaiResponses,
                Some("Anthropic Messages") => Protocol::AnthropicMessages,
                _ => Protocol::OpenaiChat,
            },
            metadata: (!metadata.is_empty()).then_some(metadata),
            credential,
        };
        profile.validate()?;
        Ok(profile)
    }

    fn render(&self, cx: &App) -> gpui::AnyElement {
        let mut fields = v_flex().gap_2();
        for (label, input) in [
            ("显示名称", &self.name),
            ("模型 ID", &self.model),
            ("API Base URL", &self.url),
        ] {
            fields = fields.child(
                v_flex()
                    .gap_1()
                    .child(div().text_xs().child(label))
                    .child(Input::new(input)),
            );
        }
        fields
            .child(
                v_flex()
                    .gap_1()
                    .child(div().text_xs().child("协议"))
                    .child(Select::new(&self.protocol).w_full()),
            )
            .child(
                v_flex()
                    .gap_1()
                    .child(div().text_xs().child("API Key（保存到系统凭据库）"))
                    .child(Input::new(&self.key)),
            )
            .child(
                v_flex()
                    .gap_1()
                    .child(div().text_xs().child("或使用环境变量"))
                    .child(Input::new(&self.environment)),
            )
            .child(
                v_flex()
                    .gap_1()
                    .child(div().text_xs().child("模型能力目录 ID（可选）"))
                    .child(Input::new(&self.metadata)),
            )
            .children(
                self.profile(cx)
                    .err()
                    .map(|error| div().text_xs().text_color(cx.theme().danger).child(error)),
            )
            .into_any_element()
    }
}

impl ChatPanel {
    pub(super) fn configure_model(
        &mut self,
        session: usize,
        edit: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(error) = &self.configuration_error {
            self.report_error(format!("请先修复配置：{error}"), cx);
            return;
        }
        let initial = edit
            .then(|| {
                self.sessions[session]
                    .options
                    .model
                    .and_then(|id| self.models.get(id))
                    .cloned()
            })
            .flatten();
        let form = ModelForm::new(initial, window, cx);
        let panel = cx.weak_entity();
        window.open_dialog(cx, move |dialog, _, cx| {
            let form = form.clone();
            let panel = panel.clone();
            let form_ok = form.clone();
            dialog
                .title("模型配置")
                .w(px(440.))
                .footer(
                    DialogFooter::new()
                        .child(DialogClose::new().child(Button::new("model-cancel").label("取消")))
                        .child(
                            DialogAction::new().child(
                                Button::new("model-save")
                                    .primary()
                                    .label("保存")
                                    .disabled(form.profile(cx).is_err()),
                            ),
                        ),
                )
                .on_ok(move |_, _, cx| {
                    let Ok(profile) = form_ok.profile(cx) else {
                        return false;
                    };
                    let key = form_ok.key.read(cx).value().trim().to_owned();
                    let task = if key.is_empty() {
                        None
                    } else {
                        Some(cx.write_credentials(
                            &crate::agent_host::credential_url(&profile.id.to_string()),
                            "api-key",
                            key.as_bytes(),
                        ))
                    };
                    let panel = panel.clone();
                    cx.spawn(async move |cx| {
                        let result = match task {
                            Some(task) => task.await.map_err(|_| {
                                "API Key 无法保存到系统凭据库，请检查钥匙串或改用环境变量"
                                    .to_owned()
                            }),
                            None => Ok(()),
                        };
                        let _ = panel.update(cx, |this, cx| {
                            if let Err(error) = result {
                                this.report_error(error, cx);
                                return;
                            }
                            let mut models = this.models.clone();
                            let index = if let Some(index) =
                                models.iter().position(|m| m.id == profile.id)
                            {
                                models[index] = profile.clone();
                                index
                            } else {
                                models.push(profile.clone());
                                models.len() - 1
                            };
                            if let Err(error) = crate::agent_host::save_profiles(&models) {
                                this.report_error(error.to_string(), cx);
                                return;
                            }
                            this.model_reasoning
                                .insert(profile.id, profile.reasoning_levels());
                            this.models = models;
                            if let Some(session) = this.sessions.get_mut(session) {
                                session.options.model = Some(index);
                                session.options.reasoning = None;
                            }
                            cx.notify();
                        });
                    })
                    .detach();
                    true
                })
                .child(form.render(cx))
        });
    }
}

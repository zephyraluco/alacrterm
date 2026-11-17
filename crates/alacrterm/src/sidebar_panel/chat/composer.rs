//! Conversation-local composer controls. Providers and external tools are not executed here.
use super::*;
use crate::dialog::name::open_name_dialog;
use gpui::{App, EntityId, PathPromptOptions};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use std::{io::Read as _, path::PathBuf};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum AgentMode {
    Ask,
    #[default]
    Agent,
}
impl AgentMode {
    fn label(self) -> &'static str {
        match self {
            Self::Ask => "Ask",
            Self::Agent => "Agent",
        }
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Reasoning {
    Low,
    #[default]
    Medium,
    High,
}
impl Reasoning {
    fn label(self) -> &'static str {
        match self {
            Self::Low => "低",
            Self::Medium => "中",
            Self::High => "高",
        }
    }
}

pub(super) struct ModelProfile {
    name: String,
    supports_reasoning: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ContextKind {
    Mcp,
    Skill,
}
#[derive(Clone)]
pub(super) struct ContextAttachment {
    path: PathBuf,
    kind: ContextKind,
    content: String,
}
impl ContextAttachment {
    pub(super) fn label(&self) -> String {
        format!(
            "{}: {}",
            if self.kind == ContextKind::Mcp {
                "MCP"
            } else {
                "Skill"
            },
            self.path.file_name().unwrap_or_default().to_string_lossy()
        )
    }
    pub(super) fn description(&self) -> String {
        format!("{} · {} 字节", self.path.display(), self.content.len())
    }
}
#[derive(Default)]
pub(super) struct ComposerOptions {
    model: Option<usize>,
    reasoning: Reasoning,
    mode: AgentMode,
    terminal: Option<EntityId>,
    pub attachments: Vec<ContextAttachment>,
}
impl ComposerOptions {
    pub(super) fn with_terminal(terminal: Option<EntityId>) -> Self {
        Self {
            terminal,
            ..Self::default()
        }
    }
    fn reasoning_for(&self, model: Option<&ModelProfile>) -> Option<Reasoning> {
        model
            .filter(|model| model.supports_reasoning)
            .map(|_| self.reasoning)
    }
}
struct TerminalChoice {
    id: EntityId,
    label: SharedString,
    connected: bool,
}

fn read_context(path: PathBuf, kind: ContextKind) -> Result<ContextAttachment, String> {
    const LIMIT: usize = 64 * 1024;
    let metadata = std::fs::metadata(&path).map_err(|error| error.to_string())?;
    if !metadata.is_file() || metadata.len() > LIMIT as u64 {
        return Err("请选择不超过 64 KiB 的文本文件".into());
    }
    let mut content = String::new();
    std::fs::File::open(&path)
        .map_err(|error| error.to_string())?
        .take((LIMIT + 1) as u64)
        .read_to_string(&mut content)
        .map_err(|error| error.to_string())?;
    if content.len() > LIMIT {
        return Err("文件过大".into());
    }
    if kind == ContextKind::Mcp {
        let config: serde_json::Value =
            serde_json::from_str(&content).map_err(|_| "MCP 配置必须是有效 JSON".to_owned())?;
        let servers = config
            .get("mcpServers")
            .or_else(|| config.get("servers"))
            .and_then(|value| value.as_object())
            .filter(|servers| !servers.is_empty())
            .ok_or("配置中未找到 MCP servers")?;
        // Keep server names only. Commands, credentials and environment values
        // from configuration files are never placed in conversation context.
        content = format!(
            "MCP 服务器（尚未连接）：{}",
            servers.keys().cloned().collect::<Vec<_>>().join(", ")
        );
    }
    Ok(ContextAttachment {
        path,
        kind,
        content,
    })
}

impl ChatPanel {
    fn terminal_choices(&self, cx: &App) -> Vec<TerminalChoice> {
        self.root
            .read_with(cx, |root, cx| {
                root.tabs
                    .iter()
                    .flat_map(|tab| {
                        let tab = tab.read(cx);
                        tab.views()
                            .iter()
                            .enumerate()
                            .map(|(index, view)| TerminalChoice {
                                id: view.entity_id(),
                                label: if tab.views().len() > 1 {
                                    format!("{} · {}", tab.name(cx), index + 1).into()
                                } else {
                                    tab.name(cx)
                                },
                                connected: view.read(cx).is_connected(cx),
                            })
                            .collect::<Vec<_>>()
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    pub(super) fn current_terminal(&self, cx: &App) -> Option<EntityId> {
        self.root
            .read_with(cx, |root, cx| {
                root.tabs
                    .get(root.active)
                    .map(|tab| tab.read(cx).focused_id())
            })
            .ok()
            .flatten()
    }

    fn configure_model(&mut self, session: usize, window: &mut Window, cx: &mut Context<Self>) {
        let panel = cx.weak_entity();
        open_name_dialog(
            window,
            cx,
            "添加模型",
            "输入模型 ID，例如 o3",
            "添加",
            None,
            move |name, cx| {
                let _ = panel.update(cx, |this, cx| {
                    let id = this
                        .models
                        .iter()
                        .position(|model| model.name == name)
                        .unwrap_or_else(|| {
                            this.models.push(ModelProfile {
                                name,
                                supports_reasoning: false,
                            });
                            this.models.len() - 1
                        });
                    if let Some(session) = this.sessions.get_mut(session) {
                        session.options.model = Some(id);
                    }
                    cx.notify();
                });
            },
        );
    }

    fn attach_context(&mut self, session: usize, kind: ContextKind, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some(
                if kind == ContextKind::Mcp {
                    "选择 MCP JSON 配置"
                } else {
                    "选择 SKILL.md"
                }
                .into(),
            ),
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            let results = cx
                .background_spawn(async move {
                    paths
                        .into_iter()
                        .map(|path| read_context(path, kind))
                        .collect::<Vec<_>>()
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                for result in results {
                    match result {
                        Ok(attachment) => {
                            if let Some(session) = this.sessions.get_mut(session) {
                                if let Some(existing) =
                                    session.options.attachments.iter_mut().find(|existing| {
                                        existing.path == attachment.path
                                            && existing.kind == attachment.kind
                                    })
                                {
                                    *existing = attachment;
                                } else {
                                    session.options.attachments.push(attachment);
                                }
                            }
                        }
                        Err(error) => {
                            let _ = this.root.update(cx, |root, cx| {
                                root.notify(
                                    gpui_kit::component::notification::Notification::error(error),
                                    cx,
                                )
                            });
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn render_composer(
        &self,
        session_index: usize,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let session = &self.sessions[session_index];
        let options = &session.options;
        let empty = session.composer.read(cx).value().trim().is_empty();
        let selected_model = options.model;
        let model = selected_model.and_then(|id| self.models.get(id));
        let model_label = model.map_or("选择模型", |model| model.name.as_str());
        let models = self
            .models
            .iter()
            .enumerate()
            .map(|(id, model)| (id, model.name.clone()))
            .collect::<Vec<_>>();
        let weak = cx.weak_entity();
        let add = Button::new("chat-add")
            .ghost()
            .xsmall()
            .icon(ui_icon(IconName::Plus))
            .tooltip("添加外部上下文")
            .dropdown_menu(move |menu, _, _| {
                let mcp = weak.clone();
                let skill = weak.clone();
                menu.item(
                    PopupMenuItem::new("添加 MCP 配置…").on_click(move |_, _, cx| {
                        let _ = mcp.update(cx, |this, cx| {
                            this.attach_context(session_index, ContextKind::Mcp, cx)
                        });
                    }),
                )
                .item(
                    PopupMenuItem::new("添加 Skills 文件…").on_click(move |_, _, cx| {
                        let _ = skill.update(cx, |this, cx| {
                            this.attach_context(session_index, ContextKind::Skill, cx)
                        });
                    }),
                )
            });
        let weak = cx.weak_entity();
        let model_button = Button::new("chat-model")
            .ghost()
            .xsmall()
            .label(model_label.to_owned())
            .max_w(px(110.))
            .overflow_hidden()
            .tooltip(model_label.to_owned())
            .dropdown_menu(move |mut menu, _, _| {
                for (id, name) in &models {
                    let id = *id;
                    let weak = weak.clone();
                    menu = menu.item(
                        PopupMenuItem::new(name.clone())
                            .checked(selected_model == Some(id))
                            .on_click(move |_, _, cx| {
                                let _ = weak.update(cx, |this, cx| {
                                    this.sessions[session_index].options.model = Some(id);
                                    cx.notify();
                                });
                            }),
                    );
                }
                let weak = weak.clone();
                menu.separator()
                    .item(
                        PopupMenuItem::new("添加模型…").on_click(move |_, window, cx| {
                            let _ = weak.update(cx, |this, cx| {
                                this.configure_model(session_index, window, cx)
                            });
                        }),
                    )
            });
        let reasoning = options.reasoning_for(model).map(|value| {
            let weak = cx.weak_entity();
            Button::new("chat-reasoning")
                .ghost()
                .xsmall()
                .label(value.label())
                .tooltip("思考强度")
                .dropdown_menu(move |mut menu, _, _| {
                    for next in [Reasoning::Low, Reasoning::Medium, Reasoning::High] {
                        let weak = weak.clone();
                        menu = menu.item(
                            PopupMenuItem::new(next.label())
                                .checked(value == next)
                                .on_click(move |_, _, cx| {
                                    let _ = weak.update(cx, |this, cx| {
                                        this.sessions[session_index].options.reasoning = next;
                                        cx.notify();
                                    });
                                }),
                        );
                    }
                    menu
                })
        });
        let weak = cx.weak_entity();
        let supports_reasoning = model.is_some_and(|model| model.supports_reasoning);
        let configure = Button::new("chat-configure")
            .ghost()
            .xsmall()
            .icon(ui_icon(IconName::Settings2))
            .tooltip("对话配置")
            .dropdown_menu(move |menu, _, _| {
                let add = weak.clone();
                let toggle = weak.clone();
                menu.item(
                    PopupMenuItem::new("添加模型…").on_click(move |_, window, cx| {
                        let _ = add.update(cx, |this, cx| {
                            this.configure_model(session_index, window, cx)
                        });
                    }),
                )
                .item(
                    PopupMenuItem::new("所选模型支持思考强度")
                        .checked(supports_reasoning)
                        .disabled(selected_model.is_none())
                        .on_click(move |_, _, cx| {
                            let _ = toggle.update(cx, |this, cx| {
                                if let Some(model) =
                                    selected_model.and_then(|id| this.models.get_mut(id))
                                {
                                    model.supports_reasoning = !model.supports_reasoning;
                                }
                                cx.notify();
                            });
                        }),
                )
                .separator()
                .item(PopupMenuItem::new("仅界面配置，AI / MCP 服务尚未接入").disabled(true))
            });
        let terminals = self.terminal_choices(cx);
        let terminal_id = options.terminal;
        let terminal_label = terminals
            .iter()
            .find(|terminal| Some(terminal.id) == terminal_id)
            .map_or_else(
                || {
                    if terminal_id.is_some() {
                        "终端已关闭".into()
                    } else {
                        "选择终端".into()
                    }
                },
                |terminal| terminal.label.clone(),
            );
        let weak = cx.weak_entity();
        let terminal_button = Button::new("chat-terminal")
            .ghost()
            .xsmall()
            .icon(ui_icon(IconName::SquareTerminal))
            .label(terminal_label.clone())
            .max_w(px(190.))
            .overflow_hidden()
            .tooltip(terminal_label)
            .dropdown_menu(move |mut menu, _, _| {
                if terminals.is_empty() {
                    return menu.item(PopupMenuItem::new("没有打开的终端").disabled(true));
                }
                for terminal in &terminals {
                    let id = terminal.id;
                    let weak = weak.clone();
                    menu = menu.item(
                        PopupMenuItem::new(terminal.label.clone())
                            .checked(terminal_id == Some(id))
                            .disabled(!terminal.connected)
                            .on_click(move |_, _, cx| {
                                let _ = weak.update(cx, |this, cx| {
                                    this.sessions[session_index].options.terminal = Some(id);
                                    cx.notify();
                                });
                            }),
                    );
                }
                menu
            });
        let mode = options.mode;
        let weak = cx.weak_entity();
        let mode_button = Button::new("chat-mode")
            .ghost()
            .xsmall()
            .icon(ui_icon(IconName::Bot))
            .label(mode.label())
            .tooltip("交互模式（尚未接入执行服务）")
            .dropdown_menu(move |mut menu, _, _| {
                for next in [AgentMode::Ask, AgentMode::Agent] {
                    let weak = weak.clone();
                    menu = menu.item(
                        PopupMenuItem::new(next.label())
                            .checked(next == mode)
                            .on_click(move |_, _, cx| {
                                let _ = weak.update(cx, |this, cx| {
                                    this.sessions[session_index].options.mode = next;
                                    cx.notify();
                                });
                            }),
                    );
                }
                menu
            });
        v_flex()
            .flex_none()
            .w_full()
            .p_3()
            .gap_1()
            .child(
                v_flex()
                    .w_full()
                    .p_2()
                    .gap_1()
                    .rounded_xl()
                    .bg(cx.theme().muted)
                    .children(
                        options
                            .attachments
                            .iter()
                            .enumerate()
                            .map(|(index, attachment)| {
                                let weak = cx.weak_entity();
                                h_flex()
                                    .w_full()
                                    .min_w_0()
                                    .items_center()
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .text_xs()
                                            .truncate()
                                            .child(attachment.label()),
                                    )
                                    .child(
                                        Button::new(("chat-context-remove", index))
                                            .ghost()
                                            .xsmall()
                                            .icon(ui_icon(IconName::CloseSmall))
                                            .tooltip(attachment.description())
                                            .accessibility_label("移除上下文")
                                            .on_click(move |_, _, cx| {
                                                let _ = weak.update(cx, |this, cx| {
                                                    this.sessions[session_index]
                                                        .options
                                                        .attachments
                                                        .remove(index);
                                                    cx.notify();
                                                });
                                            }),
                                    )
                            }),
                    )
                    .child(
                        Textarea::new(&session.composer)
                            .appearance(false)
                            .bordered(false)
                            .w_full(),
                    )
                    .child(
                        h_flex()
                            .w_full()
                            .min_w_0()
                            .items_center()
                            .gap_1()
                            .child(add)
                            .child(model_button)
                            .children(reasoning)
                            .child(div().flex_1())
                            .child(configure)
                            .child(
                                Button::new("chat-send")
                                    .ghost()
                                    .xsmall()
                                    .icon(ui_icon(IconName::ArrowUp))
                                    .tooltip("发送（Enter），换行（Shift+Enter）")
                                    .accessibility_label("发送消息")
                                    .disabled(empty)
                                    .on_click(
                                        cx.listener(|this, _, window, cx| this.send(window, cx)),
                                    ),
                            ),
                    ),
            )
            .child(
                h_flex()
                    .w_full()
                    .min_w_0()
                    .items_center()
                    .gap_1()
                    .child(terminal_button)
                    .child(mode_button),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reasoning_only_applies_to_capable_models_and_options_are_session_local() {
        let supported = ModelProfile {
            name: "reasoner".into(),
            supports_reasoning: true,
        };
        let unsupported = ModelProfile {
            name: "plain".into(),
            supports_reasoning: false,
        };
        let mut first = ComposerOptions::default();
        let second = ComposerOptions::default();
        first.reasoning = Reasoning::High;
        first.mode = AgentMode::Ask;
        assert_eq!(first.reasoning_for(Some(&supported)), Some(Reasoning::High));
        assert_eq!(first.reasoning_for(Some(&unsupported)), None);
        assert_eq!(first.reasoning_for(None), None);
        assert_eq!(second.mode, AgentMode::Agent);
        assert_eq!(second.reasoning, Reasoning::Medium);
    }
    #[test]
    fn mcp_context_only_contains_server_names_and_rejects_invalid_configs() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mcp.json");
        std::fs::write(&path, r#"{"mcpServers":{"docs":{"command":"secret-command","env":{"TOKEN":"secret-value"}}}}"#).unwrap();
        let context = read_context(path.clone(), ContextKind::Mcp).unwrap();
        assert!(context.content.contains("docs"));
        assert!(!context.content.contains("secret"));
        std::fs::write(&path, "not json").unwrap();
        assert!(read_context(path, ContextKind::Mcp).is_err());
        let path = dir.path().join("SKILL.md");
        std::fs::write(&path, "# Skill\nInstructions").unwrap();
        assert!(
            read_context(path, ContextKind::Skill)
                .unwrap()
                .content
                .contains("Instructions")
        );
    }
}

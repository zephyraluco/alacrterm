//! Conversation-local composer controls and explicit resource selection.
use super::*;
mod model;
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
    pub(super) fn core(self) -> agent::types::Mode {
        match self {
            Self::Ask => agent::types::Mode::Ask,
            Self::Agent => agent::types::Mode::Agent,
        }
    }
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
    pub(super) model: Option<usize>,
    reasoning: Option<String>,
    pub(super) mode: AgentMode,
    pub(super) terminal: Option<EntityId>,
    pub attachments: Vec<ContextAttachment>,
}
impl ComposerOptions {
    pub(super) fn with_terminal(terminal: Option<EntityId>) -> Self {
        Self {
            terminal,
            ..Self::default()
        }
    }
    pub(super) fn reasoning_for(&self, levels: &[String]) -> Option<ReasoningChoice> {
        let value = self
            .reasoning
            .as_ref()
            .filter(|v| levels.contains(v))
            .cloned()
            .or_else(|| levels.iter().find(|v| v.as_str() == "medium").cloned())
            .or_else(|| levels.first().cloned())?;
        Some(ReasoningChoice(value))
    }
}
pub(super) struct ReasoningChoice(String);
impl ReasoningChoice {
    pub(super) fn wire(&self) -> &str {
        &self.0
    }
    fn label(&self) -> String {
        reasoning_label(&self.0)
    }
}
fn reasoning_label(value: &str) -> String {
    match value {
        "low" => "低".into(),
        "medium" => "中".into(),
        "high" => "高".into(),
        other => other.into(),
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
        agent::mcp::parse_config(&content)?;
    }

    Ok(ContextAttachment {
        path,
        kind,
        content,
    })
}

pub(super) fn resources(
    attachments: &[ContextAttachment],
) -> Result<(String, Vec<agent::mcp::McpResource>), String> {
    if attachments.len() > 8
        || attachments.iter().map(|a| a.content.len()).sum::<usize>() > 128 * 1024
    {
        return Err("最多添加 8 个资源，总大小不超过 128 KiB".into());
    }
    let mut instructions = String::new();
    let mut mcp = vec![];
    for attachment in attachments {
        match attachment.kind {
            ContextKind::Skill => {
                instructions.push_str(&format!(
                    "\n--- Skill: {} ---\n{}\n--- End skill ---\n",
                    attachment.path.display(),
                    attachment.content
                ));
            }
            ContextKind::Mcp => mcp.push(agent::mcp::McpResource {
                label: attachment
                    .path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned(),
                config: agent::mcp::parse_config(&attachment.content)?,
                directory: attachment
                    .path
                    .parent()
                    .map(PathBuf::from)
                    .unwrap_or_else(crate::agent_host::default_resource_directory),
            }),
        }
    }
    Ok((instructions, mcp))
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
        let busy = session.busy();
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
            .disabled(busy)
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
            .disabled(busy)
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
                                this.configure_model(session_index, false, window, cx)
                            });
                        }),
                    )
            });
        let reasoning_levels = model
            .and_then(|m| self.model_reasoning.get(&m.id))
            .cloned()
            .unwrap_or_default();
        let reasoning = options.reasoning_for(&reasoning_levels).map(|value| {
            let weak = cx.weak_entity();
            Button::new("chat-reasoning")
                .ghost()
                .xsmall()
                .label(value.label())
                .disabled(session.busy())
                .tooltip("思考强度")
                .dropdown_menu(move |mut menu, _, _| {
                    for next in &reasoning_levels {
                        let weak = weak.clone();
                        let next = next.clone();
                        menu = menu.item(
                            PopupMenuItem::new(reasoning_label(&next))
                                .checked(value.wire() == next)
                                .on_click(move |_, _, cx| {
                                    let _ = weak.update(cx, |this, cx| {
                                        this.sessions[session_index].options.reasoning =
                                            Some(next.clone());
                                        cx.notify();
                                    });
                                }),
                        );
                    }
                    menu
                })
        });
        let weak = cx.weak_entity();
        let configure = Button::new("chat-configure")
            .ghost()
            .xsmall()
            .disabled(session.busy())
            .icon(ui_icon(IconName::Settings2))
            .tooltip("模型配置")
            .dropdown_menu(move |menu, _, _| {
                let edit = weak.clone();
                let add = weak.clone();
                menu.item(
                    PopupMenuItem::new("编辑所选模型…")
                        .disabled(selected_model.is_none())
                        .on_click(move |_, window, cx| {
                            let _ = edit.update(cx, |this, cx| {
                                this.configure_model(session_index, true, window, cx)
                            });
                        }),
                )
                .item(
                    PopupMenuItem::new("添加模型…").on_click(move |_, window, cx| {
                        let _ = add.update(cx, |this, cx| {
                            this.configure_model(session_index, false, window, cx)
                        });
                    }),
                )
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
            .disabled(busy)
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
            .disabled(busy)
            .ghost()
            .xsmall()
            .icon(ui_icon(IconName::Bot))
            .label(mode.label())
            .tooltip("Ask：只读；Agent：允许审批后执行工具")
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
                                    .icon(ui_icon(if busy {
                                        IconName::Stop
                                    } else {
                                        IconName::ArrowUp
                                    }))
                                    .tooltip(if busy {
                                        "停止生成"
                                    } else {
                                        "发送（Enter），换行（Shift+Enter）"
                                    })
                                    .accessibility_label(if busy {
                                        "停止生成"
                                    } else {
                                        "发送消息"
                                    })
                                    .disabled(if busy {
                                        session.preparing
                                    } else {
                                        empty || model.is_none()
                                    })
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        if busy {
                                            this.stop(session_index, cx);
                                        } else {
                                            this.send(window, cx);
                                        }
                                    })),
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
    fn selected_resources_are_bounded_text_and_mcp_is_not_injected_into_prompt() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("mcp.json");
        std::fs::write(&path, r#"{"mcpServers":{"docs":{"command":"mcp-docs"}}}"#).unwrap();
        let mcp = read_context(path, ContextKind::Mcp).unwrap();
        let path = directory.path().join("SKILL.md");
        std::fs::write(&path, "# Skill\nUse a concise answer").unwrap();
        let skill = read_context(path, ContextKind::Skill).unwrap();
        let (instructions, servers) = resources(&[mcp, skill]).unwrap();
        assert!(instructions.contains("concise"));
        assert!(!instructions.contains("mcp-docs"));
        assert_eq!(servers.len(), 1);
    }
}

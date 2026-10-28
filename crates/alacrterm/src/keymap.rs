//! 按键绑定：从 Zed 风格的 keymap JSON 加载绑定并注册进 gpui 的 keymap。
//!
//! 数据来源两层，后者覆盖前者：
//!
//! 1. 内嵌资产 `assets/keymaps/default-{windows,macos,linux}.json`（随二进制发布）；
//! 2. 当前构建模式配置目录中的 `keymap.json`（路径见 [`config::candidates`]）。
//!
//! 语法与 Zed 一致：顶层数组，每项一节
//! `{ "context"?, "bindings": { 键位: action | [action, 参数] | "null" } }`；
//! 键位里用空格分隔即多键序列（chord，如 `"ctrl-k ctrl-t"`）；
//! `"null"`（JSON 空值 `null` 同义）= 解绑（压掉更浅 context 上的同键位绑定）。
//! 支持 `//`、`/* */` 注释与尾随逗号。
//!
//! ⚠️ **只追加绑定，不清空**（不调 [`gpui::App::clear_key_bindings`]）：gpui-kit 的组件在
//! `init` 时注册了自己的绑定（`Root`：tab / shift-tab / ctrl-c；`Dialog`：escape / enter），
//! 清空会一并干掉，对话框确认与焦点遍历都会坏掉。
//! gpui 的键位解析先比 context 深度（深者优先）、再比注册顺序（大者优先），
//! 所以这里在 `gpui_kit::init` 之后追加即可获得更高优先级。
//!
//! ⚠️ `use_key_equivalents`（macOS 菜单用的键位等价）不支持：本模块与 `KeyBinding::new`
//! 一样用 [`DummyKeyboardMapper`]，该开关在它上面无效果。

use settings::keymap::{BindingAction, parse_sections};
use std::rc::Rc;
#[cfg(test)]
use util::strip_json_comments as strip_jsonc;

use gpui::{Action, App, DummyKeyboardMapper, KeyBinding, KeyBindingContextPredicate, NoAction};

use crate::assets::KeymapAssets;
use crate::config;

/// 用户 keymap 文件的相对路径。
const USER_KEYMAP_RELATIVE_PATH: &str = "config/keymap.json";
/// 「标签栏 / 面板」的 key context（打在 [`crate::tab_bar`] 的标签栏上）。
pub(crate) const DOCK_CONTEXT: &str = "Dock";

/// 「侧边栏」的 key context（打在 侧边栏面板根节点上）。
pub(crate) const SIDEBAR_CONTEXT: &str = "Sidebar";

/// 「预览窗口」的 key context（打在 `preview_window` 的窗口根节点上）。
pub(crate) const PREVIEW_CONTEXT: &str = "Preview";

/// 当前平台的默认 keymap 资产路径。
fn default_keymap_asset() -> &'static str {
    if cfg!(target_os = "macos") {
        "keymaps/default-macos.json"
    } else if cfg!(target_os = "windows") {
        "keymaps/default-windows.json"
    } else {
        "keymaps/default-linux.json"
    }
}

/// 读两层 keymap 并注册。在 `gpui_kit::init` 之后调用一次。
pub(crate) fn install(cx: &mut App) {
    let asset = default_keymap_asset();
    let mut sections = match KeymapAssets::get(asset) {
        Some(file) => parse_sections(&String::from_utf8_lossy(&file.data), asset),
        None => {
            eprintln!("[keymap] 未找到内嵌默认 keymap：{asset}");
            Vec::new()
        }
    };
    if let Some((path, text)) = config::read_config_file(USER_KEYMAP_RELATIVE_PATH) {
        let shown = config::shown_path(&path);
        eprintln!("[keymap] 用户绑定已加载：{shown}");
        sections.extend(parse_sections(&text, &shown));
    }

    let mut bindings = Vec::new();
    let mut skipped = 0usize;
    for section in &sections {
        let predicate = match section.context.as_deref() {
            Some(source) => match KeyBindingContextPredicate::parse(source) {
                Ok(predicate) => Some(Rc::new(predicate)),
                Err(error) => {
                    eprintln!("[keymap] context `{source}` 解析失败，跳过该节：{error}");
                    skipped += section.bindings.len();
                    continue;
                }
            },
            None => None,
        };
        for (keystrokes, action) in &section.bindings {
            match build_binding(cx, keystrokes, action, predicate.clone()) {
                Ok(binding) => bindings.push(binding),
                Err(error) => {
                    eprintln!("[keymap] 跳过键位 `{keystrokes}`：{error}");
                    skipped += 1;
                }
            }
        }
    }

    let bound = bindings.len();
    cx.bind_keys(bindings);
    eprintln!("[keymap] 已绑定 {bound} 条按键（跳过 {skipped} 条）");
}

/// 动作 → [`KeyBinding`]；动作名或键位坏掉时返回错误文案。
fn build_binding(
    cx: &App,
    keystrokes: &str,
    action: &BindingAction,
    predicate: Option<Rc<KeyBindingContextPredicate>>,
) -> Result<KeyBinding, String> {
    let action: Box<dyn Action> = match action {
        BindingAction::Unbound => Box::new(NoAction),
        BindingAction::Named(name) => cx
            .build_action(name, None)
            .map_err(|error| error.to_string())?,
        BindingAction::WithData(name, data) => cx
            .build_action(name, Some(data.clone()))
            .map_err(|error| error.to_string())?,
    };

    KeyBinding::load(
        keystrokes,
        action,
        predicate,
        false,
        None,
        &DummyKeyboardMapper,
    )
    .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::Keystroke;

    /// 三个平台的内嵌默认 keymap 都要能解析出来（macOS 的资产本机跑不到，
    /// 只能靠这条测试兜底：键位 / context 必须合法，动作名必须带命名空间）。
    #[test]
    fn embedded_default_keymaps_are_valid() {
        let assets = [
            "keymaps/default-windows.json",
            "keymaps/default-macos.json",
            "keymaps/default-linux.json",
        ];
        let mut action_names: Vec<(String, Vec<String>)> = Vec::new();

        for asset in assets {
            let file = KeymapAssets::get(asset).unwrap_or_else(|| panic!("缺少内嵌资产 {asset}"));
            let text = String::from_utf8_lossy(&file.data);
            let sections = parse_sections(&text, asset);
            assert!(!sections.is_empty(), "{asset} 一节都没解析出来");

            let mut names = Vec::new();
            for section in &sections {
                if let Some(context) = section.context.as_deref() {
                    if let Err(error) = KeyBindingContextPredicate::parse(context) {
                        panic!("{asset} 的 context `{context}` 非法：{error}");
                    }
                }
                for (keystrokes, action) in &section.bindings {
                    for part in keystrokes.split_whitespace() {
                        if let Err(error) = Keystroke::parse(part) {
                            panic!("{asset} 的键位 `{keystrokes}` 非法：{error}");
                        }
                    }
                    match action {
                        BindingAction::Unbound => {}
                        BindingAction::Named(name) | BindingAction::WithData(name, _) => {
                            assert!(
                                name.starts_with("alacrterm::"),
                                "{asset} 的动作 `{name}` 没带命名空间"
                            );
                            names.push(name.clone());
                        }
                    }
                }
            }
            assert!(!names.is_empty(), "{asset} 没有解析出任何动作");
            names.sort();
            action_names.push((asset.to_string(), names));
        }

        // 三份资产的动作集合必须一致 —— 平台差异只允许出现在键位 / context 上。
        let (first_name, first) = &action_names[0];
        for (asset, names) in &action_names[1..] {
            assert_eq!(
                first, names,
                "{asset} 与 {first_name} 的动作集合不一致（键位可以不同，动作不行）"
            );
        }
    }

    #[test]
    fn strips_line_and_block_comments() {
        let source = r#"
        [
          // 行注释
          { "bindings": { "ctrl-a": "alacrterm::X" /* 行内注释 */ } },
        ]
        "#;
        let value: serde_json::Value =
            serde_json::from_str(&strip_jsonc(source)).expect("注释应被去掉");
        assert_eq!(value[0]["bindings"]["ctrl-a"], "alacrterm::X");
    }

    #[test]
    fn keeps_comment_like_text_inside_strings() {
        let source = r#"["a//b", "c/*d*/e"]"#;
        let value: serde_json::Value =
            serde_json::from_str(&strip_jsonc(source)).expect("字符串里的斜杠要保留");
        assert_eq!(value[0], "a//b");
        assert_eq!(value[1], "c/*d*/e");
    }

    #[test]
    fn escapes_do_not_end_strings() {
        let source = r#"["a\"//b"]"#;
        let value: serde_json::Value =
            serde_json::from_str(&strip_jsonc(source)).expect("转义引号不该结束字符串");
        assert_eq!(value[0], "a\"//b");
    }

    #[test]
    fn empty_or_comment_only_source_yields_no_sections() {
        assert!(parse_sections("", "test").is_empty());
        assert!(parse_sections("// 只有注释\n", "test").is_empty());
    }

    #[test]
    fn parses_bindings_actions_and_context() {
        let source = r#"
        [
          {
            "bindings": {
              "ctrl-shift-n": "alacrterm::NewLocalTerminal",
              "alt-1": ["alacrterm::ActivateSession", 1],
              "tab": null,
            },
          },
          { "context": "Terminal", "bindings": { "shift-pageup": "alacrterm::ScrollPageUp" } },
        ]
        "#;
        let sections = parse_sections(source, "test");
        assert_eq!(sections.len(), 2);

        assert_eq!(sections[0].context, None);
        assert_eq!(
            sections[0].bindings,
            vec![
                (
                    "ctrl-shift-n".to_string(),
                    BindingAction::Named("alacrterm::NewLocalTerminal".to_string())
                ),
                (
                    "alt-1".to_string(),
                    BindingAction::WithData(
                        "alacrterm::ActivateSession".to_string(),
                        serde_json::json!(1)
                    )
                ),
                ("tab".to_string(), BindingAction::Unbound),
            ]
        );

        assert_eq!(sections[1].context.as_deref(), Some("Terminal"));
        assert_eq!(sections[1].bindings.len(), 1);
    }

    #[test]
    fn quoted_null_unbinds_like_json_null() {
        let source = r#"[{ "bindings": { "tab": "null", "shift-tab": null } }]"#;
        let sections = parse_sections(source, "test");
        assert_eq!(sections[0].bindings.len(), 2);
        for (_, action) in &sections[0].bindings {
            assert_eq!(*action, BindingAction::Unbound);
        }
    }

    #[test]
    fn skips_malformed_entries_without_failing_the_file() {
        let source = r#"
        [
          {
            "bindings": {
              "ctrl-a": "alacrterm::Ok",
              "ctrl-b": 42,
              "ctrl-c": ["alacrterm::TooMany", 1, 2],
            },
          },
          { "context": 7, "bindings": {} },
          "不是对象",
        ]
        "#;
        let sections = parse_sections(source, "test");
        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].bindings.len(), 1);
        assert_eq!(
            sections[0].bindings[0].1,
            BindingAction::Named("alacrterm::Ok".to_string())
        );
    }

    #[test]
    fn broken_json_is_ignored() {
        assert!(parse_sections("[{ \"bindings\": ", "test").is_empty());
    }
}

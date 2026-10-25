//! Keymap file models and tolerant parsing, without GUI action registration.
/// 一条绑定要派发的动作。
#[derive(Debug, PartialEq)]
pub enum BindingAction {
    /// 不带参数的动作名。
    Named(String),
    /// 带 JSON 参数的动作。
    WithData(String, serde_json::Value),
    /// `null`：解绑。
    Unbound,
}

/// keymap 文件里的一节。
#[derive(Debug)]
pub struct KeymapSection {
    /// key context 谓词（如 `"Terminal"`、`"Terminal && !vi_mode"`）；`None` = 不限定。
    pub context: Option<String>,
    /// 键位 → 动作，**保持文件里的先后顺序**（同 context 下先注册者有优先权）。
    pub bindings: Vec<(String, BindingAction)>,
}

/// 解析一份 keymap 文件；坏掉的条目只跳过（不 panic）。
pub fn parse_sections(source: &str, label: &str) -> Vec<KeymapSection> {
    let stripped = util::strip_json_comments(source);
    if stripped.trim().is_empty() {
        return Vec::new();
    }

    let value: serde_json::Value = match serde_json::from_str(&stripped) {
        Ok(value) => value,
        Err(error) => {
            eprintln!("[keymap] {label} 解析失败（该文件被忽略）：{error}");
            return Vec::new();
        }
    };

    let Some(entries) = value.as_array() else {
        eprintln!("[keymap] {label} 顶层必须是数组");
        return Vec::new();
    };

    let mut sections = Vec::with_capacity(entries.len());
    for (index, entry) in entries.iter().enumerate() {
        let Some(fields) = entry.as_object() else {
            eprintln!("[keymap] {label} 第 {} 节不是对象，跳过", index + 1);
            continue;
        };
        let context = match fields.get("context") {
            Some(serde_json::Value::String(context)) => Some(context.clone()),
            Some(_) => {
                eprintln!(
                    "[keymap] {label} 第 {} 节的 context 不是字符串，跳过该节",
                    index + 1
                );
                continue;
            }
            None => None,
        };
        let Some(serde_json::Value::Object(bindings)) = fields.get("bindings") else {
            eprintln!(
                "[keymap] {label} 第 {} 节缺少 bindings 对象，跳过该节",
                index + 1
            );
            continue;
        };

        let mut parsed = Vec::with_capacity(bindings.len());
        for (keystrokes, value) in bindings {
            match parse_action(value) {
                Ok(action) => parsed.push((keystrokes.clone(), action)),
                Err(error) => {
                    eprintln!("[keymap] {label} 跳过 `{keystrokes}`：{error}");
                }
            }
        }
        sections.push(KeymapSection {
            context,
            bindings: parsed,
        });
    }
    sections
}

/// 一个绑定值 → 动作。
///
/// `null`（JSON 空值）与字符串 `"null"` 都算解绑：前者是 Zed 的写法，后者更好写、也更显眼。
fn parse_action(value: &serde_json::Value) -> Result<BindingAction, String> {
    match value {
        serde_json::Value::Null => Ok(BindingAction::Unbound),
        serde_json::Value::String(name) if name.eq_ignore_ascii_case("null") => {
            Ok(BindingAction::Unbound)
        }
        serde_json::Value::String(name) => Ok(BindingAction::Named(name.clone())),
        serde_json::Value::Array(items) => match items.as_slice() {
            [serde_json::Value::String(name), data] => {
                Ok(BindingAction::WithData(name.clone(), data.clone()))
            }
            _ => Err("数组必须是 [\"动作名\", 参数] 两个元素".to_string()),
        },
        _ => Err("动作必须是字符串、[\"动作名\", 参数]、\"null\" 或 null".to_string()),
    }
}

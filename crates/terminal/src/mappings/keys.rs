use alacritty_terminal::term::TermMode;
use gpui::Keystroke;
use std::borrow::Cow;

/// 将 GPUI 键盘事件转换为终端转义序列
/// 参考 Zed 的 mappings/keys.rs 实现
pub fn to_esc_str(
    keystroke: &Keystroke,
    mode: &TermMode,
    option_as_meta: bool,
) -> Option<Cow<'static, str>> {
    let modifiers = &keystroke.modifiers;
    let key = keystroke.key.as_str();

    // 处理功能键
    let fkey = match key {
        "f1" => Some("\x1bOP"),
        "f2" => Some("\x1bOQ"),
        "f3" => Some("\x1bOR"),
        "f4" => Some("\x1bOS"),
        "f5" => Some("\x1b[15~"),
        "f6" => Some("\x1b[17~"),
        "f7" => Some("\x1b[18~"),
        "f8" => Some("\x1b[19~"),
        "f9" => Some("\x1b[20~"),
        "f10" => Some("\x1b[21~"),
        "f11" => Some("\x1b[23~"),
        "f12" => Some("\x1b[24~"),
        _ => None,
    };
    if let Some(seq) = fkey {
        return Some(Cow::Borrowed(seq));
    }

    // 光标键（受 APP_CURSOR 模式影响）
    let app_cursor = mode.contains(TermMode::APP_CURSOR);
    match key {
        "up" => return Some(Cow::Borrowed(if app_cursor { "\x1bOA" } else { "\x1b[A" })),
        "down" => return Some(Cow::Borrowed(if app_cursor { "\x1bOB" } else { "\x1b[B" })),
        "right" => return Some(Cow::Borrowed(if app_cursor { "\x1bOC" } else { "\x1b[C" })),
        "left" => return Some(Cow::Borrowed(if app_cursor { "\x1bOD" } else { "\x1b[D" })),
        "home" => return Some(Cow::Borrowed(if app_cursor { "\x1bOH" } else { "\x1b[H" })),
        "end" => return Some(Cow::Borrowed(if app_cursor { "\x1bOF" } else { "\x1b[F" })),
        "pageup" => return Some(Cow::Borrowed("\x1b[5~")),
        "pagedown" => return Some(Cow::Borrowed("\x1b[6~")),
        "delete" => return Some(Cow::Borrowed("\x1b[3~")),
        "insert" => return Some(Cow::Borrowed("\x1b[2~")),
        "backspace" => return Some(Cow::Borrowed("\x7f")),
        "enter" => return Some(Cow::Borrowed("\r")),
        "tab" => {
            return Some(if modifiers.shift {
                Cow::Borrowed("\x1b[Z")
            } else {
                Cow::Borrowed("\t")
            });
        }
        "escape" => return Some(Cow::Borrowed("\x1b")),
        "space" => {
            // Ctrl+Space = 0x00
            if modifiers.control {
                return Some(Cow::Borrowed("\x00"));
            }
            return Some(Cow::Borrowed(" "));
        }
        _ => {}
    }

    // Ctrl 组合键（不含 Alt）
    if modifiers.control && !modifiers.alt {
        let ctrl_str = match key {
            "a" => Some("\x01"),
            "b" => Some("\x02"),
            "c" => Some("\x03"),
            "d" => Some("\x04"),
            "e" => Some("\x05"),
            "f" => Some("\x06"),
            "g" => Some("\x07"),
            "h" => Some("\x08"),
            "i" => Some("\x09"),
            "j" => Some("\x0a"),
            "k" => Some("\x0b"),
            "l" => Some("\x0c"),
            "m" => Some("\x0d"),
            "n" => Some("\x0e"),
            "o" => Some("\x0f"),
            "p" => Some("\x10"),
            "q" => Some("\x11"),
            "r" => Some("\x12"),
            "s" => Some("\x13"),
            "t" => Some("\x14"),
            "u" => Some("\x15"),
            "v" => Some("\x16"),
            "w" => Some("\x17"),
            "x" => Some("\x18"),
            "y" => Some("\x19"),
            "z" => Some("\x1a"),
            "[" => Some("\x1b"),
            "\\" => Some("\x1c"),
            "]" => Some("\x1d"),
            "^" => Some("\x1e"),
            "_" => Some("\x1f"),
            "@" => Some("\x00"),
            _ => None,
        };
        if let Some(seq) = ctrl_str {
            return Some(Cow::Borrowed(seq));
        }
    }

    // Alt / Option 组合键（发送 ESC 前缀）
    let use_alt = modifiers.alt || (option_as_meta && modifiers.alt);
    if use_alt && !modifiers.control {
        if key.len() == 1 && key.is_ascii() {
            return Some(Cow::Owned(format!("\x1b{}", key)));
        }
        // Alt + 多字节 unicode
        if key.chars().count() == 1 {
            return Some(Cow::Owned(format!("\x1b{}", key)));
        }
    }

    // 普通字符（无 Ctrl/Meta 修饰）
    if !modifiers.control && !modifiers.platform {
        if key.len() >= 1 {
            // 过滤掉命名键（已在上方处理）
            let ch = key.chars().next().unwrap();
            if !ch.is_control() || ch == '\t' {
                return Some(Cow::Owned(key.to_string()));
            }
        }
    }

    None
}

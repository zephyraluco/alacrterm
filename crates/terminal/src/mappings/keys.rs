use alacritty_terminal::term::TermMode;
use gpui::Keystroke;
use std::borrow::Cow;

#[derive(Debug, PartialEq, Eq)]
enum TerminalModifiers {
    None,
    Alt,
    Ctrl,
    Shift,
    CtrlShift,
    Other,
}

impl TerminalModifiers {
    fn new(ks: &Keystroke) -> Self {
        match (
            ks.modifiers.alt,
            ks.modifiers.control,
            ks.modifiers.shift,
            ks.modifiers.platform,
        ) {
            (false, false, false, false) => TerminalModifiers::None,
            (true, false, false, false) => TerminalModifiers::Alt,
            (false, true, false, false) => TerminalModifiers::Ctrl,
            (false, false, true, false) => TerminalModifiers::Shift,
            (false, true, true, false) => TerminalModifiers::CtrlShift,
            _ => TerminalModifiers::Other,
        }
    }

    fn any(&self) -> bool {
        !matches!(self, TerminalModifiers::None)
    }
}

/// Convert GPUI keystrokes into xterm-compatible terminal escape sequences.
/// This tracks Zed's mapping behavior for modified navigation/function keys.
pub fn to_esc_str(
    keystroke: &Keystroke,
    mode: &TermMode,
    option_as_meta: bool,
) -> Option<Cow<'static, str>> {
    let modifiers = TerminalModifiers::new(keystroke);
    let key = keystroke.key.as_str();

    let manual_esc_str: Option<&'static str> = match (key, &modifiers) {
        ("tab", TerminalModifiers::None) => Some("\x09"),
        ("escape", TerminalModifiers::None) => Some("\x1b"),
        ("enter", TerminalModifiers::None) => Some("\x0d"),
        ("enter", TerminalModifiers::Shift) => Some("\x0a"),
        ("enter", TerminalModifiers::Alt) => Some("\x1b\x0d"),
        ("backspace", TerminalModifiers::None) => Some("\x7f"),
        ("tab", TerminalModifiers::Shift) => Some("\x1b[Z"),
        ("backspace", TerminalModifiers::Ctrl) => Some("\x08"),
        ("backspace", TerminalModifiers::Alt) => Some("\x1b\x7f"),
        ("backspace", TerminalModifiers::Shift) => Some("\x7f"),
        ("space", TerminalModifiers::Ctrl) => Some("\x00"),
        ("home", TerminalModifiers::None) if mode.contains(TermMode::APP_CURSOR) => Some("\x1bOH"),
        ("home", TerminalModifiers::None) => Some("\x1b[H"),
        ("end", TerminalModifiers::None) if mode.contains(TermMode::APP_CURSOR) => Some("\x1bOF"),
        ("end", TerminalModifiers::None) => Some("\x1b[F"),
        ("up", TerminalModifiers::None) if mode.contains(TermMode::APP_CURSOR) => Some("\x1bOA"),
        ("up", TerminalModifiers::None) => Some("\x1b[A"),
        ("down", TerminalModifiers::None) if mode.contains(TermMode::APP_CURSOR) => Some("\x1bOB"),
        ("down", TerminalModifiers::None) => Some("\x1b[B"),
        ("right", TerminalModifiers::None) if mode.contains(TermMode::APP_CURSOR) => Some("\x1bOC"),
        ("right", TerminalModifiers::None) => Some("\x1b[C"),
        ("left", TerminalModifiers::None) if mode.contains(TermMode::APP_CURSOR) => Some("\x1bOD"),
        ("left", TerminalModifiers::None) => Some("\x1b[D"),
        ("back", TerminalModifiers::None) => Some("\x7f"),
        ("insert", TerminalModifiers::None) => Some("\x1b[2~"),
        ("delete", TerminalModifiers::None) => Some("\x1b[3~"),
        ("pageup", TerminalModifiers::None) => Some("\x1b[5~"),
        ("pagedown", TerminalModifiers::None) => Some("\x1b[6~"),
        ("f1", TerminalModifiers::None) => Some("\x1bOP"),
        ("f2", TerminalModifiers::None) => Some("\x1bOQ"),
        ("f3", TerminalModifiers::None) => Some("\x1bOR"),
        ("f4", TerminalModifiers::None) => Some("\x1bOS"),
        ("f5", TerminalModifiers::None) => Some("\x1b[15~"),
        ("f6", TerminalModifiers::None) => Some("\x1b[17~"),
        ("f7", TerminalModifiers::None) => Some("\x1b[18~"),
        ("f8", TerminalModifiers::None) => Some("\x1b[19~"),
        ("f9", TerminalModifiers::None) => Some("\x1b[20~"),
        ("f10", TerminalModifiers::None) => Some("\x1b[21~"),
        ("f11", TerminalModifiers::None) => Some("\x1b[23~"),
        ("f12", TerminalModifiers::None) => Some("\x1b[24~"),
        ("f13", TerminalModifiers::None) => Some("\x1b[25~"),
        ("f14", TerminalModifiers::None) => Some("\x1b[26~"),
        ("f15", TerminalModifiers::None) => Some("\x1b[28~"),
        ("f16", TerminalModifiers::None) => Some("\x1b[29~"),
        ("f17", TerminalModifiers::None) => Some("\x1b[31~"),
        ("f18", TerminalModifiers::None) => Some("\x1b[32~"),
        ("f19", TerminalModifiers::None) => Some("\x1b[33~"),
        ("f20", TerminalModifiers::None) => Some("\x1b[34~"),
        ("a" | "A", TerminalModifiers::Ctrl | TerminalModifiers::CtrlShift) => Some("\x01"),
        ("b" | "B", TerminalModifiers::Ctrl | TerminalModifiers::CtrlShift) => Some("\x02"),
        ("c" | "C", TerminalModifiers::Ctrl | TerminalModifiers::CtrlShift) => Some("\x03"),
        ("d" | "D", TerminalModifiers::Ctrl | TerminalModifiers::CtrlShift) => Some("\x04"),
        ("e" | "E", TerminalModifiers::Ctrl | TerminalModifiers::CtrlShift) => Some("\x05"),
        ("f" | "F", TerminalModifiers::Ctrl | TerminalModifiers::CtrlShift) => Some("\x06"),
        ("g" | "G", TerminalModifiers::Ctrl | TerminalModifiers::CtrlShift) => Some("\x07"),
        ("h" | "H", TerminalModifiers::Ctrl | TerminalModifiers::CtrlShift) => Some("\x08"),
        ("i" | "I", TerminalModifiers::Ctrl | TerminalModifiers::CtrlShift) => Some("\x09"),
        ("j" | "J", TerminalModifiers::Ctrl | TerminalModifiers::CtrlShift) => Some("\x0a"),
        ("k" | "K", TerminalModifiers::Ctrl | TerminalModifiers::CtrlShift) => Some("\x0b"),
        ("l" | "L", TerminalModifiers::Ctrl | TerminalModifiers::CtrlShift) => Some("\x0c"),
        ("m" | "M", TerminalModifiers::Ctrl | TerminalModifiers::CtrlShift) => Some("\x0d"),
        ("n" | "N", TerminalModifiers::Ctrl | TerminalModifiers::CtrlShift) => Some("\x0e"),
        ("o" | "O", TerminalModifiers::Ctrl | TerminalModifiers::CtrlShift) => Some("\x0f"),
        ("p" | "P", TerminalModifiers::Ctrl | TerminalModifiers::CtrlShift) => Some("\x10"),
        ("q" | "Q", TerminalModifiers::Ctrl | TerminalModifiers::CtrlShift) => Some("\x11"),
        ("r" | "R", TerminalModifiers::Ctrl | TerminalModifiers::CtrlShift) => Some("\x12"),
        ("s" | "S", TerminalModifiers::Ctrl | TerminalModifiers::CtrlShift) => Some("\x13"),
        ("t" | "T", TerminalModifiers::Ctrl | TerminalModifiers::CtrlShift) => Some("\x14"),
        ("u" | "U", TerminalModifiers::Ctrl | TerminalModifiers::CtrlShift) => Some("\x15"),
        ("v" | "V", TerminalModifiers::Ctrl | TerminalModifiers::CtrlShift) => Some("\x16"),
        ("w" | "W", TerminalModifiers::Ctrl | TerminalModifiers::CtrlShift) => Some("\x17"),
        ("x" | "X", TerminalModifiers::Ctrl | TerminalModifiers::CtrlShift) => Some("\x18"),
        ("y" | "Y", TerminalModifiers::Ctrl | TerminalModifiers::CtrlShift) => Some("\x19"),
        ("z" | "Z", TerminalModifiers::Ctrl | TerminalModifiers::CtrlShift) => Some("\x1a"),
        ("@", TerminalModifiers::Ctrl) => Some("\x00"),
        ("[", TerminalModifiers::Ctrl) => Some("\x1b"),
        ("\\", TerminalModifiers::Ctrl) => Some("\x1c"),
        ("]", TerminalModifiers::Ctrl) => Some("\x1d"),
        ("^", TerminalModifiers::Ctrl) => Some("\x1e"),
        ("_", TerminalModifiers::Ctrl) => Some("\x1f"),
        ("?", TerminalModifiers::Ctrl) => Some("\x7f"),
        _ => None,
    };
    if let Some(esc_str) = manual_esc_str {
        return Some(Cow::Borrowed(esc_str));
    }

    if modifiers.any() {
        let modifier_code = modifier_code(keystroke);
        let modified = match key {
            "up" => Some(format!("\x1b[1;{}A", modifier_code)),
            "down" => Some(format!("\x1b[1;{}B", modifier_code)),
            "right" => Some(format!("\x1b[1;{}C", modifier_code)),
            "left" => Some(format!("\x1b[1;{}D", modifier_code)),
            "f1" => Some(format!("\x1b[1;{}P", modifier_code)),
            "f2" => Some(format!("\x1b[1;{}Q", modifier_code)),
            "f3" => Some(format!("\x1b[1;{}R", modifier_code)),
            "f4" => Some(format!("\x1b[1;{}S", modifier_code)),
            "f5" => Some(format!("\x1b[15;{}~", modifier_code)),
            "f6" => Some(format!("\x1b[17;{}~", modifier_code)),
            "f7" => Some(format!("\x1b[18;{}~", modifier_code)),
            "f8" => Some(format!("\x1b[19;{}~", modifier_code)),
            "f9" => Some(format!("\x1b[20;{}~", modifier_code)),
            "f10" => Some(format!("\x1b[21;{}~", modifier_code)),
            "f11" => Some(format!("\x1b[23;{}~", modifier_code)),
            "f12" => Some(format!("\x1b[24;{}~", modifier_code)),
            "f13" => Some(format!("\x1b[25;{}~", modifier_code)),
            "f14" => Some(format!("\x1b[26;{}~", modifier_code)),
            "f15" => Some(format!("\x1b[28;{}~", modifier_code)),
            "f16" => Some(format!("\x1b[29;{}~", modifier_code)),
            "f17" => Some(format!("\x1b[31;{}~", modifier_code)),
            "f18" => Some(format!("\x1b[32;{}~", modifier_code)),
            "f19" => Some(format!("\x1b[33;{}~", modifier_code)),
            "f20" => Some(format!("\x1b[34;{}~", modifier_code)),
            "insert" => Some(format!("\x1b[2;{}~", modifier_code)),
            "delete" => Some(format!("\x1b[3;{}~", modifier_code)),
            "pageup" => Some(format!("\x1b[5;{}~", modifier_code)),
            "pagedown" => Some(format!("\x1b[6;{}~", modifier_code)),
            "end" => Some(format!("\x1b[1;{}F", modifier_code)),
            "home" => Some(format!("\x1b[1;{}H", modifier_code)),
            _ => None,
        };
        if let Some(esc_str) = modified {
            return Some(Cow::Owned(esc_str));
        }
    }

    let use_alt_as_meta = !cfg!(target_os = "macos") || option_as_meta;
    if use_alt_as_meta {
        let is_alt_lowercase_ascii = modifiers == TerminalModifiers::Alt && key.is_ascii();
        let is_alt_uppercase_ascii =
            keystroke.modifiers.alt && keystroke.modifiers.shift && key.is_ascii();
        if is_alt_lowercase_ascii || is_alt_uppercase_ascii {
            let key = if is_alt_uppercase_ascii {
                key.to_ascii_uppercase()
            } else {
                key.to_string()
            };
            return Some(Cow::Owned(format!("\x1b{}", key)));
        }
    }

    if !keystroke.modifiers.control && !keystroke.modifiers.platform && !keystroke.modifiers.alt {
        if key.chars().count() == 1 {
            let ch = key.chars().next().unwrap();
            if !ch.is_control() || ch == '\t' {
                return Some(Cow::Owned(key.to_string()));
            }
        }
    }

    None
}

fn modifier_code(keystroke: &Keystroke) -> u32 {
    let mut modifier_code = 0;
    if keystroke.modifiers.shift {
        modifier_code |= 1;
    }
    if keystroke.modifiers.alt {
        modifier_code |= 1 << 1;
    }
    if keystroke.modifiers.control {
        modifier_code |= 1 << 2;
    }
    modifier_code + 1
}

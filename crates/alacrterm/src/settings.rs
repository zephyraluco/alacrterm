use alacritty_terminal::vte::ansi::CursorShape;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(dead_code)]
pub enum BlinkMode {
    Off,
    On,
    TerminalControlled,
}

#[derive(Clone, Debug)]
pub struct TerminalSettings {
    pub font_family: String,
    pub font_size: f32,
    /// 基础字体粗细（0.0–900.0，400.0 = Normal，700.0 = Bold）
    pub font_weight: f32,
    pub line_height_ratio: f32,
    pub cursor_shape: Option<CursorShape>,
    pub blinking: BlinkMode,
    pub scroll_multiplier: f32,
    /// APCA 最小对比度（Lc 值，约 45 ≈ WCAG 4.5）
    pub minimum_contrast: f32,
    pub option_as_meta: bool,
}

impl Default for TerminalSettings {
    fn default() -> Self {
        Self {
            font_family: "JetBrainsMono Nerd Font".to_string(),
            font_size: 14.0,
            font_weight: 400.0,
            line_height_ratio: 20.0 / 14.0,
            cursor_shape: None,
            blinking: BlinkMode::TerminalControlled,
            scroll_multiplier: 1.0,
            minimum_contrast: 45.0,
            option_as_meta: false,
        }
    }
}

impl TerminalSettings {
    pub fn line_height(&self) -> f32 {
        self.font_size * self.line_height_ratio
    }
}

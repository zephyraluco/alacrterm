use alacritty_terminal::vte::ansi::{
    CursorShape as AlacCursorShape, CursorStyle as AlacCursorStyle,
};
use std::collections::HashMap;
use gpui::{FontFallbacks, FontFeatures, FontWeight, Pixels, px};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

// pub use settings::AlternateScroll;

// use settings::{
//     PathHyperlinkRegex, RegisterSetting, ShowScrollbar, TerminalBlink, TerminalDockPosition,
//     TerminalLineHeight, VenvSettings, WorkingDirectory, merge_from::MergeFrom,
// };
// use task::Shell;
// use theme::FontFamilyName;

#[derive(Copy, Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct Toolbar {
    pub breadcrumbs: bool,
}

#[derive(Clone, Debug, Deserialize)]
pub struct TerminalSettings {
    // pub shell: Shell,
    // pub working_directory: WorkingDirectory,
    pub font_size: Option<Pixels>, // todo(settings_refactor) can be non-optional...
    // pub font_family: Option<FontFamilyName>,
    pub font_fallbacks: Option<FontFallbacks>,
    pub font_features: Option<FontFeatures>,
    pub font_weight: Option<FontWeight>,
    // pub line_height: TerminalLineHeight,
    pub env: HashMap<String, String>,
    pub cursor_shape: CursorShape,
    // pub blinking: TerminalBlink,
    // pub alternate_scroll: AlternateScroll,
    pub option_as_meta: bool,
    pub copy_on_select: bool,
    pub keep_selection_on_copy: bool,
    pub button: bool,
    // pub dock: TerminalDockPosition,
    pub default_width: Pixels,
    pub default_height: Pixels,
    // pub detect_venv: VenvSettings,
    pub max_scroll_history_lines: Option<usize>,
    pub scroll_multiplier: f32,
    pub toolbar: Toolbar,
    pub scrollbar: ScrollbarSettings,
    pub minimum_contrast: f32,
    pub path_hyperlink_regexes: Vec<String>,
    pub path_hyperlink_timeout_ms: u64,
}

#[derive(Copy, Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct ScrollbarSettings {
    /// When to show the scrollbar in the terminal.
    ///
    /// Default: inherits editor scrollbar settings
    pub show: Option<bool>,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CursorShape {
    /// Cursor is a block like `█`.
    #[default]
    Block,
    /// Cursor is an underscore like `_`.
    Underline,
    /// Cursor is a vertical bar like `⎸`.
    Bar,
    /// Cursor is a hollow box like `▯`.
    Hollow,
}

// impl From<settings::CursorShapeContent> for CursorShape {
//     fn from(value: settings::CursorShapeContent) -> Self {
//         match value {
//             settings::CursorShapeContent::Block => CursorShape::Block,
//             settings::CursorShapeContent::Underline => CursorShape::Underline,
//             settings::CursorShapeContent::Bar => CursorShape::Bar,
//             settings::CursorShapeContent::Hollow => CursorShape::Hollow,
//         }
//     }
// }

impl From<CursorShape> for AlacCursorShape {
    fn from(value: CursorShape) -> Self {
        match value {
            CursorShape::Block => AlacCursorShape::Block,
            CursorShape::Underline => AlacCursorShape::Underline,
            CursorShape::Bar => AlacCursorShape::Beam,
            CursorShape::Hollow => AlacCursorShape::HollowBlock,
        }
    }
}

impl From<CursorShape> for AlacCursorStyle {
    fn from(value: CursorShape) -> Self {
        AlacCursorStyle {
            shape: value.into(),
            blinking: false,
        }
    }
}

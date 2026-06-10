use std::collections::HashMap;

use gpui::WindowButtonLayout;
use schemars::{json_schema, JsonSchema, Schema, SchemaGenerator};
use serde::{Deserialize, Serialize};

use crate::{TerminalSettingsContent, ThemeSettingsContent};

/// The state of the modifier keys at some point in time.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModifiersContent {
    /// The control key.
    #[serde(default)]
    pub control: bool,
    /// The alt key, sometimes also known as the meta key.
    #[serde(default)]
    pub alt: bool,
    /// The shift key.
    #[serde(default)]
    pub shift: bool,
    /// The command key on macOS, windows key on Windows, super key on Linux.
    #[serde(default)]
    pub platform: bool,
    /// The function key.
    #[serde(default)]
    pub function: bool,
}

/// When to show the scrollbar.
///
/// Default: auto
#[derive(
    Copy,
    Clone,
    Debug,
    Default,
    Serialize,
    Deserialize,
    PartialEq,
    Eq,
    JsonSchema,
    strum::VariantArray,
    strum::VariantNames,
)]
#[serde(rename_all = "snake_case")]
pub enum ShowScrollbar {
    /// Show the scrollbar if there's important information or
    /// follow the system's configured behavior.
    #[default]
    Auto,
    /// Match the system's configured behavior.
    System,
    /// Always show the scrollbar.
    Always,
    /// Never show the scrollbar.
    Never,
}

#[derive(
    Clone, PartialEq, Debug, Serialize, Deserialize, JsonSchema, Default, strum::EnumDiscriminants,
)]
#[strum_discriminants(derive(strum::VariantArray, strum::VariantNames, strum::FromRepr))]
#[schemars(schema_with = "window_button_layout_schema")]
#[serde(from = "String", into = "String")]
pub enum WindowButtonLayoutContent {
    /// Follow the system/desktop configuration.
    #[default]
    PlatformDefault,
    /// Use Zed's built-in standard layout, regardless of system config.
    Standard,
    /// A raw GNOME-style layout string.
    Custom(String),
}

impl WindowButtonLayoutContent {
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    pub fn into_layout(self) -> Option<WindowButtonLayout> {
        use util::ResultExt;

        match self {
            Self::PlatformDefault => None,
            Self::Standard => Some(WindowButtonLayout::linux_default()),
            Self::Custom(layout) => WindowButtonLayout::parse(&layout).log_err(),
        }
    }

    #[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
    pub fn into_layout(self) -> Option<WindowButtonLayout> {
        None
    }
}

fn window_button_layout_schema(_: &mut SchemaGenerator) -> Schema {
    json_schema!({
        "anyOf": [
            { "enum": ["platform_default", "standard"] },
            { "type": "string" }
        ]
    })
}

impl From<WindowButtonLayoutContent> for String {
    fn from(value: WindowButtonLayoutContent) -> Self {
        match value {
            WindowButtonLayoutContent::PlatformDefault => "platform_default".to_string(),
            WindowButtonLayoutContent::Standard => "standard".to_string(),
            WindowButtonLayoutContent::Custom(s) => s,
        }
    }
}

impl From<String> for WindowButtonLayoutContent {
    fn from(layout_string: String) -> Self {
        match layout_string.as_str() {
            "platform_default" => Self::PlatformDefault,
            "standard" => Self::Standard,
            _ => Self::Custom(layout_string),
        }
    }
}

#[derive(Clone, Default, Serialize, Deserialize, JsonSchema, Debug, PartialEq, Eq)]
pub struct StatusBarSettingsContent {
    /// Whether to show the status bar.
    ///
    /// Default: true
    #[serde(rename = "experimental.show")]
    pub show: Option<bool>,
    /// Whether to show the name of the active file in the status bar.
    ///
    /// Default: false
    pub show_active_file: Option<bool>,
    /// Whether to display the active language button in the status bar.
    ///
    /// Default: true
    pub active_language_button: Option<bool>,
    /// Whether to show the cursor position button in the status bar.
    ///
    /// Default: true
    pub cursor_position_button: Option<bool>,
    /// Whether to show active line endings button in the status bar.
    ///
    /// Default: false
    pub line_endings_button: Option<bool>,
}

#[derive(Clone, PartialEq, Default, Serialize, Deserialize, JsonSchema, Debug)]
pub struct TitleBarSettingsContent {
    /// Whether to show git status indicators on the branch icon in the title bar.
    pub show_branch_status_icon: Option<bool>,
    /// Whether to show onboarding banners in the title bar.
    pub show_onboarding_banner: Option<bool>,
    /// Whether to show user avatar in the title bar.
    pub show_user_picture: Option<bool>,
    /// Whether to show the branch name button in the titlebar.
    pub show_branch_name: Option<bool>,
    /// Whether to show the project host and name in the titlebar.
    pub show_project_items: Option<bool>,
    /// Whether to show the sign in button in the title bar.
    pub show_sign_in: Option<bool>,
    /// Whether to show the user menu button in the title bar.
    pub show_user_menu: Option<bool>,
    /// Whether to show the menus in the title bar.
    pub show_menus: Option<bool>,
    /// The layout of window control buttons in the title bar (Linux only).
    pub button_layout: Option<WindowButtonLayoutContent>,
}

#[derive(Debug, PartialEq, Default, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SettingsContent {
    #[serde(flatten)]
    pub theme: Box<ThemeSettingsContent>,

    #[serde(flatten)]
    // pub extension: ExtensionSettingsContent,
    pub status_bar: Option<StatusBarSettingsContent>,

    /// Configuration of audio in Zed.
    // pub audio: Option<AudioSettingsContent>,

    /// A map of log scopes to the desired log level.
    /// Useful for filtering out noisy logs or enabling more verbose logging.
    ///
    /// Example: {"log": {"client": "warn"}}
    pub log: Option<HashMap<String, String>>,

    pub proxy: Option<String>,

    /// Configuration for session-related features.
    // pub session: Option<SessionSettingsContent>,

    /// Configuration of the terminal in Zed.
    pub terminal: Option<TerminalSettingsContent>,

    pub title_bar: Option<TitleBarSettingsContent>,

    /// Whether or not to enable Vim mode.
    ///
    /// Default: false
    pub vim_mode: Option<bool>,
}

use std::{borrow::Cow, collections::HashMap, fmt::Display, sync::Arc};

use gpui::WindowButtonLayout;
use indexmap::IndexMap;
use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Deserializer, Serialize};

use crate::{TerminalSettingsContent, serialize_f32_with_two_decimal_places};

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
// serde(transparent) 序列化和反序列化时直接使用内部字段的表示，而不是把包装结构体本身序列化出来
#[serde(transparent)]
pub struct FontFamilyName(pub Arc<str>);

impl AsRef<str> for FontFamilyName {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl From<String> for FontFamilyName {
    fn from(value: String) -> Self {
        Self(Arc::from(value))
    }
}

impl From<FontFamilyName> for String {
    fn from(value: FontFamilyName) -> Self {
        value.0.to_string()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct FontFeaturesContent(pub IndexMap<String, u32>);

impl FontFeaturesContent {
    pub fn new() -> Self {
        Self(IndexMap::default())
    }
}

#[derive(Debug, serde::Deserialize)]
#[serde(untagged)]
enum FeatureValue {
    Bool(bool),
    Number(serde_json::Number),
}

fn is_valid_feature_tag(tag: &str) -> bool {
    tag.len() == 4 && tag.chars().all(|c| c.is_ascii_alphanumeric())
}

impl<'de> Deserialize<'de> for FontFeaturesContent {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        use serde::de::{MapAccess, Visitor};
        use std::fmt;

        struct FontFeaturesVisitor;

        impl<'de> Visitor<'de> for FontFeaturesVisitor {
            type Value = FontFeaturesContent;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("a map of font features")
            }

            fn visit_map<M>(self, mut access: M) -> Result<Self::Value, M::Error>
            where
                M: MapAccess<'de>,
            {
                let mut feature_map = IndexMap::default();

                while let Some((key, value)) =
                    access.next_entry::<String, Option<FeatureValue>>()?
                {
                    if !is_valid_feature_tag(&key) {
                        log::error!("Incorrect font feature tag: {}", key);
                        continue;
                    }
                    if let Some(value) = value {
                        match value {
                            FeatureValue::Bool(enable) => {
                                feature_map.insert(key, if enable { 1 } else { 0 });
                            }
                            FeatureValue::Number(value) => {
                                if value.is_u64() {
                                    feature_map.insert(key, value.as_u64().unwrap() as u32);
                                } else {
                                    log::error!(
                                        "Incorrect font feature value {} for feature tag {}",
                                        value,
                                        key
                                    );
                                    continue;
                                }
                            }
                        }
                    }
                }

                Ok(FontFeaturesContent(feature_map))
            }
        }

        deserializer.deserialize_map(FontFeaturesVisitor)
    }
}

impl JsonSchema for FontFeaturesContent {
    fn schema_name() -> Cow<'static, str> {
        "FontFeaturesContent".into()
    }

    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        json_schema!({
            "type": "object",
            "patternProperties": {
                "[0-9a-zA-Z]{4}$": {
                    "type": ["boolean", "integer"],
                    "minimum": 0,
                    "multipleOf": 1
                }
            },
            "additionalProperties": false
        })
    }
}

/// A font size value in pixels, wrapping around `f32` for custom settings UI rendering.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, PartialOrd)]
#[serde(transparent)]
pub struct FontSize(#[serde(serialize_with = "serialize_f32_with_two_decimal_places")] pub f32);

impl Display for FontSize {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:.2}", self.0)
    }
}

impl From<f32> for FontSize {
    fn from(value: f32) -> Self {
        Self(value)
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum FontStyleContent {
    Normal,
    Italic,
    Oblique,
}

#[derive(Clone, Copy, Debug, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct FontWeightContent(pub f32);

impl Display for FontWeightContent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<f32> for FontWeightContent {
    fn from(weight: f32) -> Self {
        FontWeightContent(weight)
    }
}

impl Default for FontWeightContent {
    fn default() -> Self {
        Self::NORMAL
    }
}

impl FontWeightContent {
    pub const THIN: FontWeightContent = FontWeightContent(100.0);
    pub const EXTRA_LIGHT: FontWeightContent = FontWeightContent(200.0);
    pub const LIGHT: FontWeightContent = FontWeightContent(300.0);
    pub const NORMAL: FontWeightContent = FontWeightContent(400.0);
    pub const MEDIUM: FontWeightContent = FontWeightContent(500.0);
    pub const SEMIBOLD: FontWeightContent = FontWeightContent(600.0);
    pub const BOLD: FontWeightContent = FontWeightContent(700.0);
    pub const EXTRA_BOLD: FontWeightContent = FontWeightContent(800.0);
    pub const BLACK: FontWeightContent = FontWeightContent(900.0);
}

impl schemars::JsonSchema for FontWeightContent {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "FontWeightContent".into()
    }

    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        json_schema!({
            "type": "number",
            "minimum": Self::THIN.0,
            "maximum": Self::BLACK.0,
            "default": Self::NORMAL.0,
            "description": "Font weight value between 100 (thin) and 900 (black)"
        })
    }
}

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

/// The background appearance of the window.
#[derive(Debug, PartialEq, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WindowBackgroundContent {
    Opaque,
    Transparent,
    Blurred,
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

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(transparent)]
pub struct ThemeName(pub Arc<str>);

#[derive(
    Debug,
    PartialEq,
    Eq,
    Clone,
    Copy,
    Default,
    Serialize,
    Deserialize,
    JsonSchema,
    strum::VariantArray,
    strum::VariantNames,
)]
#[serde(rename_all = "snake_case")]
pub enum ThemeAppearanceMode {
    /// Use the specified `light` theme.
    Light,
    /// Use the specified `dark` theme.
    Dark,
    /// Use the theme based on the system's appearance.
    #[default]
    System,
}

/// Represents the selection of a theme, which can be either static or dynamic.
#[derive(
    Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq, strum::EnumDiscriminants,
)]
#[strum_discriminants(derive(strum::VariantArray, strum::VariantNames, strum::FromRepr))]
#[serde(untagged)]
pub enum ThemeSelection {
    /// A static theme selection, represented by a single theme name.
    Static(ThemeName),
    /// A dynamic theme selection, which can change based the [ThemeMode].
    Dynamic {
        /// The mode used to determine which theme to use.
        #[serde(default)]
        mode: ThemeAppearanceMode,
        /// The theme to use for light mode.
        light: ThemeName,
        /// The theme to use for dark mode.
        dark: ThemeName,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(transparent)]
pub struct IconThemeName(pub Arc<str>);

#[derive(Clone, PartialEq, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct ThemeSettingsContent {
    /// The default font size for text in the UI.
    pub ui_font_size: Option<FontSize>,
    /// The name of a font to use for rendering in the UI.
    pub ui_font_family: Option<FontFamilyName>,
    /// The font fallbacks to use for rendering in the UI.
    #[schemars(extend("uniqueItems" = true))]
    pub ui_font_fallbacks: Option<Vec<FontFamilyName>>,
    /// The OpenType features to enable for text in the UI.
    pub ui_font_features: Option<FontFeaturesContent>,
    /// The weight of the UI font in CSS units from 100 to 900.
    pub ui_font_weight: Option<FontWeightContent>,
    /// The default font size for text in the terminal.
    pub theme: Option<ThemeSelection>,
    /// The name of the icon theme to use.
    pub icon_theme: Option<IconThemeName>,
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

mod content_into_gpui;
mod serde_helper;
use std::{borrow::Cow, collections::{BTreeMap, HashMap}, fmt::Display, path::PathBuf, rc::Rc, sync::Arc};
use indexmap::IndexMap;
use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Deserializer, Serialize, de::DeserializeOwned};
use gpui::{SharedString, WindowButtonLayout};
pub use content_into_gpui::*;
use serde_helper::{serialize_f32_with_two_decimal_places, serialize_optional_f32_with_two_decimal_places};
use util::asset_str;
use rust_embed::RustEmbed;

#[derive(Clone, Debug, Serialize, Deserialize,JsonSchema,PartialEq, Eq)]
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


//1
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
        use schemars::json_schema;
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
#[derive(
    Clone,
    Copy,
    Debug,
    Serialize,
    Deserialize,
    JsonSchema,
    PartialEq,
    PartialOrd,
)]
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

#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    PartialOrd,
    Serialize,
    Deserialize,
)]
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
        use schemars::json_schema;
        json_schema!({
            "type": "number",
            "minimum": Self::THIN.0,
            "maximum": Self::BLACK.0,
            "default": Self::NORMAL.0,
            "description": "Font weight value between 100 (thin) and 900 (black)"
        })
    }
}

/// The state of the modifier keys at some point in time
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModifiersContent {
    /// The control key
    #[serde(default)]
    pub control: bool,
    /// The alt key
    /// Sometimes also known as the 'meta' key
    #[serde(default)]
    pub alt: bool,
    /// The shift key
    #[serde(default)]
    pub shift: bool,
    /// The command key, on macos
    /// the windows key, on windows
    /// the super key, on linux
    #[serde(default)]
    pub platform: bool,
    /// The function key
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

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, JsonSchema)]
#[serde(untagged)]
pub enum PathHyperlinkRegex {
    SingleLine(String),
    MultiLine(Vec<String>),
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
    Copy,
    Clone,
    Debug,
    Default,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    JsonSchema,
    strum::VariantArray,
    strum::VariantNames,
)]
#[serde(rename_all = "snake_case")]
pub enum TerminalBell {
    /// Play an OS-specific alert sound.
    #[default]
    System,
    /// Do not play any sound.
    Off,
}

#[derive(
    Copy,
    Clone,
    Debug,
    Serialize,
    Deserialize,
    PartialEq,
    Eq,
    JsonSchema,
    strum::VariantArray,
    strum::VariantNames,
)]
#[serde(rename_all = "snake_case")]
pub enum TerminalBlink {
    /// Never blink the cursor, ignoring the terminal mode.
    Off,
    /// Default the cursor blink to off, but allow the terminal to
    /// set blinking.
    TerminalControlled,
    /// Always blink the cursor, ignoring the terminal mode.
    On,
}

#[derive(
    Copy,
    Clone,
    Debug,
    Serialize,
    Deserialize,
    PartialEq,
    Eq,
    JsonSchema,
    strum::VariantArray,
    strum::VariantNames,
)]
#[serde(rename_all = "snake_case")]
pub enum TerminalDockPosition {
    Left,
    Bottom,
    Right,
}

#[derive(Clone, Debug, Serialize, Deserialize,JsonSchema, PartialEq, Default)]
#[serde(rename_all = "snake_case")]
pub enum TerminalLineHeight {
    /// Use a line height that's comfortable for reading, 1.618
    #[default]
    Comfortable,
    /// Use a standard line height, 1.3. This option is useful for TUIs,
    /// particularly if they use box characters
    Standard,
    /// Use a custom line height.
    Custom(#[serde(serialize_with = "crate::serialize_f32_with_two_decimal_places")] f32),
}

impl TerminalLineHeight {
    pub fn value(&self) -> f32 {
        match self {
            TerminalLineHeight::Comfortable => 1.618,
            TerminalLineHeight::Standard => 1.3,
            TerminalLineHeight::Custom(line_height) => f32::max(*line_height, 1.),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ActivateScript {
    #[default]
    Default,
    Csh,
    Fish,
    Nushell,
    PowerShell,
    Pyenv,
}
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum CondaManager {
    /// Automatically detect the conda manager
    #[default]
    Auto,
    /// Use conda
    Conda,
    /// Use mamba
    Mamba,
    /// Use micromamba
    Micromamba,
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum VenvSettings {
    #[default]
    Off,
    On {
        /// Default directories to search for virtual environments, relative
        /// to the current working directory. We recommend overriding this
        /// in your project's settings, rather than globally.
        activate_script: Option<ActivateScript>,
        venv_name: Option<String>,
        directories: Option<Vec<PathBuf>>,
        /// Preferred Conda manager to use when activating Conda environments.
        ///
        /// Default: auto
        conda_manager: Option<CondaManager>,
    },
}


#[derive(
    Clone,
    Debug,
    Serialize,
    Deserialize,
    PartialEq,
    Eq,
    JsonSchema,
    strum::EnumDiscriminants,
)]
#[strum_discriminants(derive(strum::VariantArray, strum::VariantNames, strum::FromRepr))]
#[serde(rename_all = "snake_case")]
pub enum WorkingDirectory {
    /// Use the current file's directory, falling back to the project directory,
    /// then the first project in the workspace.
    CurrentFileDirectory,
    /// Use the current file's project directory. Fallback to the
    /// first project directory strategy if unsuccessful.
    CurrentProjectDirectory,
    /// Use the first project in this workspace's directory. Fallback to using
    /// this platform's home directory.
    FirstProjectDirectory,
    /// Always use this platform's home directory (if it can be found).
    AlwaysHome,
    /// Always use a specific directory. This value will be shell expanded.
    /// If this path is not a valid directory the terminal will default to
    /// this platform's home directory  (if it can be found).
    Always { directory: String },
}

#[derive(
    Clone,
    Copy,
    Debug,
    Serialize,
    Deserialize,
    PartialEq,
    Eq,
    JsonSchema,
    strum::VariantArray,
    strum::VariantNames,
)]
#[serde(rename_all = "snake_case")]
pub enum AlternateScroll {
    On,
    Off,
}

// Shell configuration to open the terminal with.
#[derive(
    Clone,
    Debug,
    Default,
    Serialize,
    Deserialize,
    PartialEq,
    Eq,
    JsonSchema,
    strum::EnumDiscriminants,
)]
#[strum_discriminants(derive(strum::VariantArray, strum::VariantNames, strum::FromRepr))]
#[serde(rename_all = "snake_case")]
pub enum Shell {
    /// Use the system's default terminal configuration in /etc/passwd
    #[default]
    System,
    /// Use a specific program with no arguments.
    Program(String),
    /// Use a specific program with arguments.
    WithArguments {
        /// The program to run.
        program: String,
        /// The arguments to pass to the program.
        args: Vec<String>,
        /// An optional string to override the title of the terminal tab
        title_override: Option<String>,
    },
}


#[derive(
    Clone,
    Copy,
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
// todo() -> combine with CursorShape
pub enum CursorShapeContent {
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

#[derive(
    Clone,
    PartialEq,
    Debug,
    Serialize,
    Deserialize,
    JsonSchema,
    Default,
    strum::EnumDiscriminants,
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


#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct TerminalToolbarContent {
    /// Whether to display the terminal title in breadcrumbs inside the terminal pane.
    /// Only shown if the terminal title is not empty.
    ///
    /// The shell running in the terminal needs to be configured to emit the title.
    /// Example: `echo -e "\e]2;New Title\007";`
    ///
    /// Default: true
    pub breadcrumbs: Option<bool>,
}

#[derive(
    Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq, Default,
)]
pub struct ScrollbarSettingsContent {
    /// When to show the scrollbar in the terminal.
    ///
    /// Default: inherits editor scrollbar settings
    pub show: Option<ShowScrollbar>,
}

#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize, JsonSchema)]
pub struct ProjectTerminalSettingsContent {
    /// What shell to use when opening a terminal.
    ///
    /// Default: system
    pub shell: Option<Shell>,
    /// What working directory to use when launching the terminal
    ///
    /// Default: current_project_directory
    pub working_directory: Option<WorkingDirectory>,
    /// Any key-value pairs added to this list will be added to the terminal's
    /// environment. Use `:` to separate multiple values.
    ///
    /// Default: {}
    pub env: Option<HashMap<String, String>>,
    /// Activates the python virtual environment, if one is found, in the
    /// terminal's working directory (as resolved by the working_directory
    /// setting). Set this to "off" to disable this behavior.
    ///
    /// Default: on
    pub detect_venv: Option<VenvSettings>,
    /// Regexes used to identify paths for hyperlink navigation.
    ///
    /// Default: [
    ///   // Python-style diagnostics
    ///   "File \"(?<path>[^\"]+)\", line (?<line>[0-9]+)",
    ///   // Common path syntax with optional line, column, description, trailing punctuation, or
    ///   // surrounding symbols or quotes
    ///   [
    ///     "(?x)",
    ///     "# optionally starts with 0-2 opening prefix symbols",
    ///     "[({\\[<]{0,2}",
    ///     "# which may be followed by an opening quote",
    ///     "(?<quote>[\"'`])?",
    ///     "# `path` is the shortest sequence of any non-space character",
    ///     "(?<link>(?<path>[^ ]+?",
    ///     "    # which may end with a line and optionally a column,",
    ///     "    (?<line_column>:+[0-9]+(:[0-9]+)?|:?\\([0-9]+([,:][0-9]+)?\\))?",
    ///     "))",
    ///     "# which must be followed by a matching quote",
    ///     "(?(<quote>)\\k<quote>)",
    ///     "# and optionally a single closing symbol",
    ///     "[)}\\]>]?",
    ///     "# if line/column matched, may be followed by a description",
    ///     "(?(<line_column>):[^ 0-9][^ ]*)?",
    ///     "# which may be followed by trailing punctuation",
    ///     "[.,:)}\\]>]*",
    ///     "# and always includes trailing whitespace or end of line",
    ///     "([ ]+|$)"
    ///   ]
    /// ]
    pub path_hyperlink_regexes: Option<Vec<PathHyperlinkRegex>>,
    /// Timeout for hover and Cmd-click path hyperlink discovery in milliseconds.
    ///
    /// Default: 1
    pub path_hyperlink_timeout_ms: Option<u64>,
}
#[derive(Clone, Debug, PartialEq, Default, Serialize,Deserialize, JsonSchema)]
pub struct TerminalSettingsContent {
    #[serde(flatten)]
    pub project: ProjectTerminalSettingsContent,
    /// Sets the terminal's font size.
    ///
    /// If this option is not included,
    /// the terminal will default to matching the buffer's font size.
    pub font_size: Option<FontSize>,
    /// Sets the terminal's font family.
    ///
    /// If this option is not included,
    /// the terminal will default to matching the buffer's font family.
    pub font_family: Option<FontFamilyName>,

    /// Sets the terminal's font fallbacks.
    ///
    /// If this option is not included,
    /// the terminal will default to matching the buffer's font fallbacks.
    #[schemars(extend("uniqueItems" = true))]
    pub font_fallbacks: Option<Vec<FontFamilyName>>,

    /// Sets the terminal's line height.
    ///
    /// Default: comfortable
    pub line_height: Option<TerminalLineHeight>,
    pub font_features: Option<FontFeaturesContent>,
    /// Sets the terminal's font weight in CSS weight units 0-900.
    pub font_weight: Option<FontWeightContent>,
    /// Default cursor shape for the terminal.
    /// Can be "bar", "block", "underline", or "hollow".
    ///
    /// Default: "block"
    pub cursor_shape: Option<CursorShapeContent>,
    /// Sets the cursor blinking behavior in the terminal.
    ///
    /// Default: terminal_controlled
    pub blinking: Option<TerminalBlink>,
    /// Sets whether Alternate Scroll mode (code: ?1007) is active by default.
    /// Alternate Scroll mode converts mouse scroll events into up / down key
    /// presses when in the alternate screen (e.g. when running applications
    /// like vim or  less). The terminal can still set and unset this mode.
    ///
    /// Default: on
    pub alternate_scroll: Option<AlternateScroll>,
    /// Sets whether the option key behaves as the meta key.
    ///
    /// Default: false
    pub option_as_meta: Option<bool>,
    /// Whether or not selecting text in the terminal will automatically
    /// copy to the system clipboard.
    ///
    /// Default: false
    pub copy_on_select: Option<bool>,
    /// Whether to keep the text selection after copying it to the clipboard.
    ///
    /// Default: true
    pub keep_selection_on_copy: Option<bool>,
    /// Whether to show the terminal button in the status bar.
    ///
    /// Default: true
    pub button: Option<bool>,
    pub dock: Option<TerminalDockPosition>,
    /// Whether the terminal panel should use flexible (proportional) sizing.
    ///
    /// Default: true
    pub flexible: Option<bool>,
    /// Default width when the terminal is docked to the left or right.
    ///
    /// Default: 640
    #[serde(serialize_with = "crate::serialize_optional_f32_with_two_decimal_places")]
    pub default_width: Option<f32>,
    /// Default height when the terminal is docked to the bottom.
    ///
    /// Default: 320
    #[serde(serialize_with = "crate::serialize_optional_f32_with_two_decimal_places")]
    pub default_height: Option<f32>,
    /// The maximum number of lines to keep in the scrollback history.
    /// Maximum allowed value is 100_000, all values above that will be treated as 100_000.
    /// 0 disables the scrolling.
    /// Existing terminals will not pick up this change until they are recreated.
    /// See <a href="https://github.com/alacritty/alacritty/blob/cb3a79dbf6472740daca8440d5166c1d4af5029e/extra/man/alacritty.5.scd?plain=1#L207-L213">Alacritty documentation</a> for more information.
    ///
    /// Default: 10_000
    pub max_scroll_history_lines: Option<usize>,
    /// The multiplier for scrolling with the mouse wheel.
    ///
    /// Default: 1.0
    pub scroll_multiplier: Option<f32>,
    /// Toolbar related settings
    pub toolbar: Option<TerminalToolbarContent>,
    /// Scrollbar-related settings
    pub scrollbar: Option<ScrollbarSettingsContent>,
    /// The minimum APCA perceptual contrast between foreground and background colors.
    ///
    /// APCA (Accessible Perceptual Contrast Algorithm) is more accurate than WCAG 2.x,
    /// especially for dark mode. Values range from 0 to 106.
    ///
    /// Based on APCA Readability Criterion (ARC) Bronze Simple Mode:
    /// https://readtech.org/ARC/tests/bronze-simple-mode/
    /// - 0: No contrast adjustment
    /// - 45: Minimum for large fluent text (36px+)
    /// - 60: Minimum for other content text
    /// - 75: Minimum for body text
    /// - 90: Preferred for body text
    ///
    /// Default: 45
    #[serde(serialize_with = "crate::serialize_optional_f32_with_two_decimal_places")]
    pub minimum_contrast: Option<f32>,
    /// Whether to show a badge on the terminal panel icon with the count of open terminals.
    ///
    /// Default: false
    pub show_count_badge: Option<bool>,
    /// What to do when the `BEL` character (`\a`) is printed to terminal.
    ///
    /// Default: "system"
    pub bell: Option<TerminalBell>,
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
    /// When enabled, the branch icon changes to reflect the current repository
    /// status (e.g. modified, added, deleted, or conflict).
    ///
    /// Default: false
    pub show_branch_status_icon: Option<bool>,
    /// Whether to show onboarding banners in the title bar.
    ///
    /// Default: true
    pub show_onboarding_banner: Option<bool>,
    /// Whether to show user avatar in the title bar.
    ///
    /// Default: true
    pub show_user_picture: Option<bool>,
    /// Whether to show the branch name button in the titlebar.
    ///
    /// Default: true
    pub show_branch_name: Option<bool>,
    /// Whether to show the project host and name in the titlebar.
    ///
    /// Default: true
    pub show_project_items: Option<bool>,
    /// Whether to show the sign in button in the title bar.
    ///
    /// Default: true
    pub show_sign_in: Option<bool>,
    /// Whether to show the user menu button in the title bar.
    ///
    /// Default: true
    pub show_user_menu: Option<bool>,
    /// Whether to show the menus in the title bar.
    ///
    /// Default: false
    pub show_menus: Option<bool>,
    /// The layout of window control buttons in the title bar (Linux only).
    ///
    /// This can be set to "platform_default" to follow the system configuration, or
    /// "standard" to use Zed's built-in layout. For custom layouts, use a
    /// GNOME-style layout string like "close:minimize,maximize".
    ///
    /// Default: "platform_default"
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
    Clone,
    Debug,
    Serialize,
    Deserialize,
    JsonSchema,
    PartialEq,
    Eq,
    strum::EnumDiscriminants,
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
    // #[schemars(default = "default_font_fallbacks")]
    #[schemars(extend("uniqueItems" = true))]
    pub ui_font_fallbacks: Option<Vec<FontFamilyName>>,
    /// The OpenType features to enable for text in the UI.
    // #[schemars(default = "default_font_features")]
    pub ui_font_features: Option<FontFeaturesContent>,
    /// The weight of the UI font in CSS units from 100 to 900.
    // #[schemars(default = "default_buffer_font_weight")]
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

    /// Configuration for session-related features
    // pub session: Option<SessionSettingsContent>,

    /// Configuration of the terminal in Zed.
    pub terminal: Option<TerminalSettingsContent>,

    pub title_bar: Option<TitleBarSettingsContent>,

    /// Whether or not to enable Vim mode.
    ///
    /// Default: false
    pub vim_mode: Option<bool>,

}

pub struct SettingsStore {
    // setting_values: TypeIdHashMap<Box<dyn AnySettingValue>>,
    default_settings: Rc<SettingsContent>,
    user_settings: Option<SettingsContent>,
    global_settings: Option<Box<SettingsContent>>,

    extension_settings: Option<Box<SettingsContent>>,
    server_settings: Option<Box<SettingsContent>>,

    merged_settings: Rc<SettingsContent>,

    last_user_settings_content: Option<String>,
    last_global_settings_content: Option<String>,
}

#[derive(RustEmbed)]
#[folder = "../../assets"]
#[include = "settings/*"]
#[include = "keymaps/*"]
#[exclude = "*.DS_Store"]
pub struct SettingsAssets;

pub fn default_settings() -> Cow<'static, str> {
    asset_str::<SettingsAssets>("settings/default.json")
}

pub fn parse_json_with_comments<T: DeserializeOwned>(content: &str) -> serde_json::Result<T> {
    serde_json::from_str(content)
}

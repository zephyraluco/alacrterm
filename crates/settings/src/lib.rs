mod content_into_gpui;
mod serde_helper;
mod settings_content;
mod terminal;

use std::{borrow::Cow, rc::Rc};

use gpui::Global;
use rust_embed::RustEmbed;
use serde::de::DeserializeOwned;
use util::asset_str;

pub use content_into_gpui::*;
pub use serde_helper::*;
pub use settings_content::*;
pub use terminal::*;

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

impl Global for SettingsStore {}

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

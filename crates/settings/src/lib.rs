mod content_into_gpui;
mod serde_helper;
mod settings_content;
mod terminal;

use std::{borrow::Cow, rc::Rc};

use gpui::{App, Global};
use rust_embed::RustEmbed;
use serde::de::DeserializeOwned;
use util::{asset_str, strip_json_comments};

pub use content_into_gpui::*;
pub use serde_helper::*;
pub use settings_content::*;
pub use terminal::*;

pub struct SettingsStore {
    // setting_values: TypeIdHashMap<Box<dyn AnySettingValue>>,
    default_settings: Rc<SettingsContent>,
    user_settings: Option<SettingsContent>,
    // global_settings: Option<Box<SettingsContent>>,
    // extension_settings: Option<Box<SettingsContent>>,
    // server_settings: Option<Box<SettingsContent>>,

    // merged_settings: Rc<SettingsContent>,

    // last_user_settings_content: Option<String>,
    // last_global_settings_content: Option<String>,
}

impl Global for SettingsStore {}

impl SettingsStore {
    pub fn new() -> serde_json::Result<Self> {
        let default_settings = parse_json_with_comments(&default_settings())?;

        Ok(Self {
            default_settings: Rc::new(default_settings),
            user_settings: None,
        })
    }

    pub fn default_settings(&self) -> &SettingsContent {
        &self.default_settings
    }

    pub fn user_settings(&self) -> Option<&SettingsContent> {
        self.user_settings.as_ref()
    }
}
pub trait Settings: 'static + Send + Sync + Sized {
    fn from_settings(content: &SettingsContent) -> Self;
    fn get_global(cx: &mut App) -> Self {
        let store = cx.global::<SettingsStore>();
        Self::from_settings(store.user_settings().unwrap_or_else(|| store.default_settings()))
    }
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
    serde_json::from_str(&strip_json_comments(content))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_print_default_settings() {
        let settings = default_settings();

        println!("{settings}");

        assert!(!settings.trim().is_empty());
    }

    #[test]
    fn test_print_parse_json_with_comments_result() {
        let settings = parse_json_with_comments::<SettingsContent>(
            r#"{
                // line comments are allowed
                "vim_mode": true,
                "proxy": "http://127.0.0.1:10809", /* block comments are allowed */
            }"#,
        )
        .unwrap();

        println!("{settings:#?}");

        assert_eq!(settings.vim_mode, Some(true));
        assert_eq!(settings.proxy.as_deref(), Some("http://127.0.0.1:10809"));
    }

    #[test]
    fn test_parse_json_with_comments_keeps_comment_markers_in_strings() {
        let value = parse_json_with_comments::<serde_json::Value>(
            r#"{
                "url": "http://127.0.0.1:10809/*not-a-comment*/",
                "items": ["//not-a-comment",],
            }"#,
        )
        .unwrap();

        println!("{value:#?}");

        assert_eq!(value["url"], "http://127.0.0.1:10809/*not-a-comment*/");
        assert_eq!(value["items"][0], "//not-a-comment");
    }

    #[test]
    fn test_print_parse_json_with_comments_default() {
        let settings = parse_json_with_comments::<SettingsContent>(&default_settings()).unwrap();

        println!("{settings:#?}");

        assert_ne!(settings, SettingsContent::default());
    }
}

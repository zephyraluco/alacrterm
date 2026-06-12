mod content_into_gpui;
mod serde_helper;
mod settings_content;
mod terminal;
mod theme;

use std::{
    borrow::Cow,
    error::Error,
    fs,
    path::Path,
    rc::Rc,
};

use gpui::{App, Global};
use rust_embed::RustEmbed;
use serde::de::DeserializeOwned;
use serde_json::Value;
use util::{asset_str, strip_json_comments};

pub use content_into_gpui::*;
pub use serde_helper::*;
pub use settings_content::*;
pub use terminal::*;
pub use theme::*;

pub struct SettingsStore {
    // setting_values: TypeIdHashMap<Box<dyn AnySettingValue>>,
    default_settings: Rc<SettingsContent>,
    user_settings: Option<SettingsContent>,
    global_settings: Rc<SettingsContent>,
    // extension_settings: Option<Box<SettingsContent>>,
    // server_settings: Option<Box<SettingsContent>>,

    // merged_settings: Rc<SettingsContent>,

    // last_user_settings_content: Option<String>,
    // last_global_settings_content: Option<String>,
}

impl Global for SettingsStore {}

impl SettingsStore {
    pub fn new() -> serde_json::Result<Self> {
        let default_settings: SettingsContent = parse_json_with_comments(&default_settings())?;
        let default_settings = Rc::new(default_settings);

        Ok(Self {
            default_settings: default_settings.clone(),
            user_settings: None,
            global_settings: default_settings,
        })
    }

    pub fn default_settings(&self) -> &SettingsContent {
        &self.default_settings
    }

    pub fn user_settings(&self) -> Option<&SettingsContent> {
        self.user_settings.as_ref()
    }

    pub fn global_settings(&self) -> &SettingsContent {
        &self.global_settings
    }

    pub fn load_user_settings(
        &mut self,
        path: impl AsRef<Path>,
    ) -> Result<(), Box<dyn Error + Send + Sync>> {
        let content = fs::read_to_string(path)?;
        let user_settings = parse_json_with_comments(&content)?;
        self.global_settings = Rc::new(merge_settings(&self.default_settings, &user_settings)?);
        self.user_settings = Some(user_settings);
        Ok(())
    }

    pub fn update_user_settings(
        &mut self,
        _cx: &mut App,
        update: impl FnOnce(&mut SettingsContent),
    ) {
        // let settings = self
        //     .user_settings
        //     .get_or_insert_with(|| self.default_settings.as_ref().clone());
        if let Some(settings) = self.user_settings.as_mut() {
            update(settings);
            self.global_settings = Rc::new(
                merge_settings(&self.default_settings, settings).expect("failed to merge settings"),
            );
        }
    }
}
pub trait Settings: 'static + Send + Sync + Sized {
    fn from_settings(content: &SettingsContent) -> Self;
    fn get_global(cx: &App) -> Self {
        let store = cx.global::<SettingsStore>();
        Self::from_settings(store.global_settings())
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

fn merge_settings(
    default_settings: &SettingsContent,
    user_settings: &SettingsContent,
) -> serde_json::Result<SettingsContent> {
    let mut settings = serde_json::to_value(default_settings)?;
    merge_json(&mut settings, serde_json::to_value(user_settings)?);
    serde_json::from_value(settings)
}

fn merge_json(default: &mut Value, user: Value) {
    match (default, user) {
        (_, Value::Null) => {}
        (Value::Object(default), Value::Object(user)) => {
            for (key, value) in user {
                if value.is_null() {
                    continue;
                }

                match default.get_mut(&key) {
                    Some(default_value) => merge_json(default_value, value),
                    None => {
                        default.insert(key, value);
                    }
                }
            }
        }
        (default, user) => *default = user,
    }
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

    #[test]
    fn test_set_user_settings_from_path() {
        use std::fs;

        let dir = std::env::current_exe()
            .unwrap()
            .parent()
            .unwrap()
            .to_path_buf();
        let path = dir.join("settings.json");
        fs::write(
            &path,
            r#"{
                // user settings support comments
                "vim_mode": true,
            }"#,
        )
        .unwrap();

        let mut store = SettingsStore::new().unwrap();
        store.load_user_settings(&path).unwrap();

        assert_eq!(store.user_settings().unwrap().vim_mode, Some(true));
        assert_eq!(store.global_settings().vim_mode, Some(true));
        assert_eq!(
            store.global_settings().terminal,
            store.default_settings().terminal
        );

        fs::remove_file(path).unwrap();
    }
}

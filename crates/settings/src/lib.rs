//! UI-independent application configuration and persistence.
pub mod color_schemes;
pub mod document;
pub mod keymap;
mod suggestions;
pub use document::{SettingsFile, SettingsPatch, SettingsSnapshot, commit, load_snapshot};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};
pub use suggestions::SuggestionSettings;

const SETTINGS_CONFIG_RELATIVE_PATH: &str = "config/settings.json";

const SESSIONS_CONFIG_RELATIVE_PATH: &str = "config/sessions.json";

/// 开发构建使用未跟踪的源码 config；发布构建使用系统用户配置目录。
pub fn candidates(relative: &str) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if Path::new(relative).starts_with("config") {
        #[cfg(debug_assertions)]
        return vec![
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../..")
                .join(relative),
        ];
        #[cfg(not(debug_assertions))]
        return user_config_path(relative).into_iter().collect();
    }
    if let Some(dir) = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(PathBuf::from))
    {
        paths.push(dir.join(relative));
    }
    #[cfg(any(not(target_os = "windows"), debug_assertions))]
    paths.push(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join(relative),
    );
    paths
}

/// 读第一个命中的配置文件。
pub fn read_config_file(relative: &str) -> Option<(PathBuf, String)> {
    candidates(relative)
        .into_iter()
        .find_map(|path| std::fs::read_to_string(&path).ok().map(|text| (path, text)))
}

#[cfg(not(debug_assertions))]
fn user_config_path(relative: &str) -> Option<PathBuf> {
    configuration_path(&dirs::config_dir()?, relative)
}

#[cfg(any(not(debug_assertions), test))]
fn configuration_path(base: &Path, relative: &str) -> Option<PathBuf> {
    let filename = Path::new(relative).strip_prefix("config").ok()?;
    let application = if cfg!(target_os = "linux") {
        "alacrterm"
    } else {
        "Alacrterm"
    };
    Some(base.join(application).join(filename))
}

/// 配置写入当前构建模式的专属目录，不在开发配置和用户配置之间回退。
pub fn write_target(relative: &str) -> PathBuf {
    select_write_target(&candidates(relative))
}

fn select_write_target(paths: &[PathBuf]) -> PathBuf {
    if let Some(existing) = paths.iter().find(|path| path.exists()) {
        return existing.clone();
    }
    paths
        .iter()
        .find(|path| path.parent().is_some_and(Path::is_dir))
        .or_else(|| paths.first())
        .cloned()
        .expect("candidates() 至少给一条路径")
}

pub fn sessions_config_path() -> PathBuf {
    write_target(SESSIONS_CONFIG_RELATIVE_PATH)
}

/// 去掉 Windows 路径的 `\\?\` 前缀。
pub fn shown_path(path: &Path) -> String {
    path.canonicalize()
        .unwrap_or_else(|_| path.to_path_buf())
        .to_string_lossy()
        .trim_start_matches(r"\\?\")
        .to_string()
}

/// `config/settings.json` 的 `terminal` 子项；字段全可选。
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct TerminalFile {
    pub color_scheme: Option<String>,
    pub font_family: Option<String>,
    pub font_size: Option<f32>,
    pub font_weight: Option<f32>,
    pub line_height_multiplier: Option<f32>,
    pub minimum_contrast: Option<f32>,
    pub cursor_shape: Option<String>,
    pub cursor_blinks: Option<bool>,
    pub suggestions: Option<SuggestionSettings>,
    /// ANSI 调色板：键名 = `TerminalColors` 的字段名。
    pub colors: HashMap<String, String>,
    pub selection_color: Option<String>,
    pub search_match_color: Option<String>,
    pub link_color: Option<String>,
}

/// Persisted UI mode. Older configurations and unknown mode names default to dark.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AppThemeMode {
    Light,
    #[default]
    #[serde(other)]
    Dark,
}

/// `config/settings.json` 顶层的应用级偏好。
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct AppFile {
    pub theme_mode: AppThemeMode,
    /// 深色 / 浅色槽位用的主题名；`null` = 跟随内置主题。
    pub dark_theme: Option<String>,
    pub light_theme: Option<String>,
}

/// A configuration document with an explicit write destination.
/// Parse failures are returned to the caller and never replaced with an empty object.
pub struct SettingsStore {
    path: PathBuf,
}
impl SettingsStore {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }
    pub fn read(&self) -> anyhow::Result<serde_json::Value> {
        match std::fs::read_to_string(&self.path) {
            Ok(text) => Ok(serde_json::from_str(&text)?),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(json!({})),
            Err(error) => Err(error.into()),
        }
    }
    pub fn update(
        &self,
        preferences: serde_json::Map<String, serde_json::Value>,
    ) -> anyhow::Result<()> {
        let mut document = self.read()?;
        let map = document.as_object_mut().ok_or_else(|| {
            anyhow::anyhow!(
                "configuration must be a JSON object: {}",
                self.path.display()
            )
        })?;
        map.extend(preferences);
        let mut text = serde_json::to_string_pretty(&document)?;
        text.push('\n');
        util::atomic_file::write(&self.path, text.as_bytes())?;
        Ok(())
    }
}

/// Migrate once without deleting the originals. An existing settings.json is
/// authoritative, including omitted keys; legacy values must not reappear later.
fn unified_store_at(path: &Path) -> anyhow::Result<SettingsStore> {
    let store = SettingsStore::new(path.to_owned());
    if path.try_exists()? {
        return Ok(store);
    }
    let directory = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("missing settings directory"))?;
    let read_legacy = |name: &str| -> anyhow::Result<Option<serde_json::Value>> {
        let file = directory.join(name);
        match std::fs::read_to_string(&file) {
            Ok(text) => {
                let value: serde_json::Value = serde_json::from_str(&text)?;
                anyhow::ensure!(
                    value.is_object(),
                    "{} must contain a JSON object",
                    file.display()
                );
                Ok(Some(value))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.into()),
        }
    };
    let app = read_legacy("app.json")?;
    let terminal = read_legacy("terminal.json")?;
    if app.is_some() || terminal.is_some() {
        let mut document = app.unwrap_or_else(|| json!({}));
        if let Some(terminal) = terminal {
            document["terminal"] = terminal;
        }
        SettingsSnapshot::parse(document.clone())?;
        store.update(document.as_object().unwrap().clone())?;
    }
    Ok(store)
}

fn unified_store() -> anyhow::Result<SettingsStore> {
    unified_store_at(&write_target(SETTINGS_CONFIG_RELATIVE_PATH))
}

#[cfg(test)]
fn save_terminal_preferences_at(
    path: &Path,
    preferences: serde_json::Map<String, serde_json::Value>,
) -> anyhow::Result<()> {
    document::commit_at(
        &SettingsStore::new(path.to_owned()),
        SettingsPatch::Terminal(preferences),
    )?;
    Ok(())
}

impl TerminalFile {
    /// Drop invalid numeric overrides, allowing renderer defaults to take effect.
    /// Returns whether any invalid values were found.
    pub fn validate(&mut self) -> bool {
        let mut invalid = false;
        for (value, minimum, maximum) in [
            (
                &mut self.font_size,
                document::FONT_SIZE.0 as f32,
                document::FONT_SIZE.1 as f32,
            ),
            (
                &mut self.font_weight,
                document::FONT_WEIGHT.0 as f32,
                document::FONT_WEIGHT.1 as f32,
            ),
            (
                &mut self.line_height_multiplier,
                document::LINE_HEIGHT.0 as f32,
                document::LINE_HEIGHT.1 as f32,
            ),
            (
                &mut self.minimum_contrast,
                document::CONTRAST.0 as f32,
                document::CONTRAST.1 as f32,
            ),
        ] {
            if value.is_some_and(|value| !value.is_finite() || value < minimum || value > maximum) {
                *value = None;
                invalid = true;
            }
        }
        invalid
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(super) fn migration_fixture() -> (util::file_cache::FileCache, util::file_cache::CacheLease)
    {
        let cache = util::file_cache::FileCache::new(
            &std::env::temp_dir().join(format!("alacrterm-migrate-{}", std::process::id())),
        );
        let lease = cache.allocate("settings.json");
        (cache, lease)
    }

    #[test]
    fn migration_keeps_legacy_values_and_unknown_fields_and_does_not_repeat() {
        let (_cache, lease) = migration_fixture();
        let directory = lease.path().parent().unwrap();
        let app = r#"{"theme_mode":"light","dark_theme":"Nord","future_app":true}"#;
        let terminal = r##"{"font_size":17,"colors":{"terminal_background":"#123456"},"future_terminal":true}"##;
        std::fs::write(directory.join("app.json"), app).unwrap();
        std::fs::write(directory.join("terminal.json"), terminal).unwrap();
        let store = unified_store_at(lease.path()).unwrap();
        let saved = store.read().unwrap();
        assert_eq!(saved["theme_mode"], "light");
        assert_eq!(saved["dark_theme"], "Nord");
        assert_eq!(saved["future_app"], true);
        assert_eq!(saved["terminal"]["font_size"], 17);
        assert_eq!(saved["terminal"]["future_terminal"], true);
        assert_eq!(
            saved["terminal"]["colors"]["terminal_background"],
            "#123456"
        );
        assert_eq!(
            std::fs::read_to_string(directory.join("app.json")).unwrap(),
            app
        );
        assert_eq!(
            std::fs::read_to_string(directory.join("terminal.json")).unwrap(),
            terminal
        );
        // Existing new settings are authoritative even when legacy values change.
        std::fs::write(directory.join("app.json"), r#"{"theme_mode":"dark"}"#).unwrap();
        assert_eq!(
            unified_store_at(lease.path()).unwrap().read().unwrap(),
            saved
        );
        store
            .update(serde_json::Map::from_iter([(
                "theme_mode".into(),
                json!("dark"),
            )]))
            .unwrap();
        assert_eq!(store.read().unwrap()["terminal"], saved["terminal"]);
    }

    #[test]
    fn migration_accepts_either_legacy_file_and_no_legacy_files() {
        for legacy in [Some("app.json"), Some("terminal.json"), None] {
            let (_cache, lease) = migration_fixture();
            if let Some(name) = legacy {
                std::fs::write(
                    lease.path().parent().unwrap().join(name),
                    r#"{"future":true}"#,
                )
                .unwrap();
            }
            let document = unified_store_at(lease.path()).unwrap().read().unwrap();
            match legacy {
                Some("app.json") => assert_eq!(document["future"], true),
                Some(_) => assert_eq!(document["terminal"]["future"], true),
                None => assert!(!lease.path().exists()),
            }
        }
    }

    #[test]
    fn invalid_legacy_or_terminal_section_is_never_overwritten() {
        let (_cache, lease) = migration_fixture();
        let legacy = lease.path().parent().unwrap().join("app.json");
        std::fs::write(&legacy, "{").unwrap();
        assert!(unified_store_at(lease.path()).is_err());
        assert!(!lease.path().exists());
        assert_eq!(std::fs::read_to_string(&legacy).unwrap(), "{");
        let invalid = r#"{"theme_mode":"light","terminal":[]}"#;
        std::fs::write(lease.path(), invalid).unwrap();
        assert!(save_terminal_preferences_at(lease.path(), serde_json::Map::new()).is_err());
        assert_eq!(std::fs::read_to_string(lease.path()).unwrap(), invalid);
    }

    #[test]
    fn theme_mode_defaults_preserve_legacy_theme_choices() {
        for document in [
            r#"{"dark_theme":"Nord","light_theme":"One Light"}"#,
            r#"{"theme_mode":"unknown","dark_theme":"Nord","light_theme":"One Light"}"#,
        ] {
            let config: AppFile = serde_json::from_str(document).unwrap();
            assert_eq!(config.theme_mode, AppThemeMode::Dark);
            assert_eq!(config.dark_theme.as_deref(), Some("Nord"));
            assert_eq!(config.light_theme.as_deref(), Some("One Light"));
        }
    }

    #[test]
    fn theme_mode_round_trips_both_choices() {
        for (text, expected) in [("dark", AppThemeMode::Dark), ("light", AppThemeMode::Light)] {
            let config: AppFile = serde_json::from_value(json!({"theme_mode": text})).unwrap();
            assert_eq!(config.theme_mode, expected);
            assert_eq!(serde_json::to_value(&config).unwrap()["theme_mode"], text);
        }
    }
    #[cfg(not(debug_assertions))]
    #[test]
    fn configuration_has_only_the_user_directory_candidate() {
        for name in ["settings.json", "sessions.json", "keymap.json"] {
            let relative = format!("config/{name}");
            assert_eq!(
                candidates(&relative),
                vec![configuration_path(&dirs::config_dir().unwrap(), &relative).unwrap()]
            );
        }
    }

    #[cfg(debug_assertions)]
    #[test]
    fn development_configuration_stays_in_the_source_directory() {
        for name in ["settings.json", "sessions.json", "keymap.json"] {
            let relative = format!("config/{name}");
            assert_eq!(
                candidates(&relative),
                vec![
                    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                        .join("../..")
                        .join(relative)
                ]
            );
        }
    }

    #[test]
    fn user_paths_separate_configuration_from_bundled_resources() {
        let base = Path::new("AppData/Roaming");
        for name in ["settings.json", "sessions.json", "keymap.json"] {
            assert_eq!(
                configuration_path(base, &format!("config/{name}")),
                Some(
                    base.join(if cfg!(target_os = "linux") {
                        "alacrterm"
                    } else {
                        "Alacrterm"
                    })
                    .join(name)
                )
            );
        }
        assert_eq!(configuration_path(base, "themes/global"), None);
        assert_eq!(configuration_path(base, "themes/terminal"), None);
    }

    #[test]
    fn first_save_creates_user_configuration_directory() {
        let root =
            std::env::temp_dir().join(format!("alacrterm-first-config-{}", std::process::id()));
        let cache = util::file_cache::FileCache::new(&root);
        let lease = cache.allocate("installation");
        let installed = configuration_path(lease.path(), "config/app.json").unwrap();
        let build_machine = lease.path().join("build-machine/config/app.json");
        let target = select_write_target(&[installed.clone(), build_machine.clone()]);
        assert_eq!(target, installed);
        SettingsStore::new(target.clone())
            .update(serde_json::Map::from_iter([(
                "dark_theme".into(),
                json!("Nord"),
            )]))
            .unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&std::fs::read_to_string(target).unwrap())
                .unwrap()["dark_theme"],
            "Nord"
        );
        assert!(!build_machine.exists());
        std::fs::remove_dir_all(lease.path()).unwrap();
    }
    #[test]
    fn corrupt_configuration_is_not_overwritten() {
        let root =
            std::env::temp_dir().join(format!("alacrterm-corrupt-settings-{}", std::process::id()));
        let cache = util::file_cache::FileCache::new(&root);
        let lease = cache.allocate("settings.json");
        std::fs::write(lease.path(), "{broken").unwrap();
        let store = SettingsStore::new(lease.path().to_owned());
        assert!(store.update(serde_json::Map::new()).is_err());
        assert_eq!(std::fs::read_to_string(lease.path()).unwrap(), "{broken");
    }
    #[test]
    fn external_numeric_values_use_the_same_bounds_as_the_ui() {
        let mut file: TerminalFile = serde_json::from_str(r#"{"font_size":-1,"font_weight":950,"line_height_multiplier":0,"minimum_contrast":107}"#).unwrap();
        assert!(file.validate());
        assert!(
            file.font_size.is_none()
                && file.font_weight.is_none()
                && file.line_height_multiplier.is_none()
                && file.minimum_contrast.is_none()
        );
        assert!(!file.validate());
    }
    #[test]
    fn saving_preferences_preserves_palette_and_unknown_fields() {
        let root = std::env::temp_dir().join(format!("alacrterm-settings-{}", std::process::id()));
        let cache = util::file_cache::FileCache::new(&root);
        let lease = cache.allocate("terminal.json");
        std::fs::write(lease.path(), r##"{"theme_mode":"light","terminal":{"font_size":15,"colors":{"terminal_background":"#010203"},"future":{"keep":true}}}"##).unwrap();
        save_terminal_preferences_at(
            lease.path(),
            serde_json::Map::from_iter([("font_size".into(), json!(18))]),
        )
        .unwrap();
        let saved = SettingsStore::new(lease.path().to_owned()).read().unwrap();
        assert_eq!(saved["terminal"]["font_size"], 18);
        assert_eq!(saved["theme_mode"], "light");
        assert_eq!(
            saved["terminal"]["colors"]["terminal_background"],
            "#010203"
        );
        assert_eq!(saved["terminal"]["future"]["keep"], true);
    }
    #[test]
    fn saving_non_object_configuration_does_not_overwrite_it() {
        let root =
            std::env::temp_dir().join(format!("alacrterm-settings-invalid-{}", std::process::id()));
        let cache = util::file_cache::FileCache::new(&root);
        let lease = cache.allocate("terminal.json");
        std::fs::write(lease.path(), "[]").unwrap();
        assert!(save_terminal_preferences_at(lease.path(), serde_json::Map::new()).is_err());
        assert_eq!(std::fs::read_to_string(lease.path()).unwrap(), "[]");
    }
    #[test]
    fn legacy_terminal_fields_remain_optional() {
        let file: TerminalFile = serde_json::from_str(
            r##"{"font_size":17,"colors":{"terminal_background":"#000000"},"future":true}"##,
        )
        .unwrap();
        assert_eq!(file.font_size, Some(17.));
        assert!(file.suggestions.is_none());
        assert_eq!(file.colors["terminal_background"], "#000000");
    }
    #[test]
    fn bundled_resources_keep_executable_relative_candidates() {
        let executable = std::env::current_exe().unwrap();
        assert_eq!(
            candidates("themes/global")[0],
            executable.parent().unwrap().join("themes/global")
        );
    }
}

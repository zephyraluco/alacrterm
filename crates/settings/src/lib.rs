//! UI-independent application configuration and persistence.
mod suggestions;
pub use suggestions::SuggestionSettings;
use std::{collections::HashMap, path::{Path, PathBuf}};
use serde::{Deserialize, Serialize};
use serde_json::json;

const TERMINAL_CONFIG_RELATIVE_PATH: &str = "config/terminal.json";

const APP_CONFIG_RELATIVE_PATH: &str = "config/app.json";


const SESSIONS_CONFIG_RELATIVE_PATH: &str = "config/sessions.json";

/// 候选路径：可执行文件同级 → 仓库根。
pub fn candidates(relative: &str) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(dir) = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(PathBuf::from))
    {
        paths.push(dir.join(relative));
    }
    paths.push(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join(relative),
    );
    paths
}

/// 读第一个命中的配置文件。
pub fn read_config_file(relative: &str) -> Option<(PathBuf, String)> {
    candidates(relative).into_iter().find_map(|path| {
        std::fs::read_to_string(&path)
            .ok()
            .map(|text| (path, text))
    })
}

/// 写回目标：优先已存在的那份。
pub fn write_target(relative: &str) -> PathBuf {
    let paths = candidates(relative);
    if let Some(existing) = paths.iter().find(|path| path.exists()) {
        return existing.clone();
    }
    paths
        .iter()
        .find(|path| path.parent().is_some_and(Path::is_dir))
        .or_else(|| paths.last())
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


/// `config/terminal.json` 的结构；字段全可选。
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct TerminalFile {
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

/// `config/app.json`：应用级偏好。
#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct AppFile {
    /// 深色 / 浅色槽位用的主题名；`null` = 跟随内置主题。
    pub dark_theme: Option<String>,
    pub light_theme: Option<String>,
}

/// 读 JSON 文档；失败时给空对象。
fn read_json_value(path: &Path) -> serde_json::Value {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_else(|| json!({}))
}

fn write_json_doc(path: &Path, doc: &serde_json::Value, what: &str) {
    if let Some(parent) = path.parent()
        && let Err(error) = std::fs::create_dir_all(parent)
    {
        eprintln!("[config] {what}保存失败（建目录 {}）：{error}", parent.display());
        return;
    }

    match serde_json::to_string_pretty(doc) {
        Ok(mut text) => {
            text.push('\n');
            match std::fs::write(path, text) {
                Ok(()) => eprintln!("[config] {what}已保存：{}", shown_path(path)),
                Err(error) => eprintln!("[config] {what}保存失败：{error}"),
            }
        }
        Err(error) => eprintln!("[config] {what}序列化失败：{error}"),
    }
}

pub fn load_app_config() -> AppFile {
    let Some((path, text)) = read_config_file(APP_CONFIG_RELATIVE_PATH) else {
        return AppFile::default();
    };

    match serde_json::from_str(&text) {
        Ok(file) => {
            eprintln!("[config] 应用偏好已加载：{}", shown_path(&path));
            file
        }
        Err(error) => {
            eprintln!(
                "[config] 应用偏好 {} 解析失败（改用默认）：{error}",
                path.display()
            );
            AppFile::default()
        }
    }
}

/// 把主题选择写回 `config/app.json`。
pub fn save_theme_choice(dark: Option<&str>, light: Option<&str>) {
    let path = write_target(APP_CONFIG_RELATIVE_PATH);
    let doc = json!({ "dark_theme": dark, "light_theme": light });
    write_json_doc(&path, &doc, "应用偏好");
}

/// Load terminal preferences, preserving renderer defaults for omitted fields.
pub fn load_terminal_config() -> TerminalFile {
    let Some((path, text)) = read_config_file(TERMINAL_CONFIG_RELATIVE_PATH) else {
        return TerminalFile::default();
    };
    match serde_json::from_str(&text) {
        Ok(file) => file,
        Err(error) => {
            eprintln!("[config] {}: {error}", path.display());
            TerminalFile::default()
        }
    }
}

/// Save only UI-owned preferences, preserving colors and unknown keys.
pub fn save_terminal_preferences(preferences: serde_json::Map<String, serde_json::Value>) {
    let path = write_target(TERMINAL_CONFIG_RELATIVE_PATH);
    let mut doc = read_json_value(&path);
    if let Some(map) = doc.as_object_mut() {
        map.extend(preferences);
        write_json_doc(&path, &doc, "终端渲染参数");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn legacy_terminal_fields_remain_optional() {
        let file: TerminalFile = serde_json::from_str(r##"{"font_size":17,"colors":{"terminal_background":"#000000"},"future":true}"##).unwrap();
        assert_eq!(file.font_size, Some(17.));
        assert!(file.suggestions.is_none());
        assert_eq!(file.colors["terminal_background"], "#000000");
    }
    #[test]
    fn repository_fallback_keeps_existing_configuration_root() {
        assert_eq!(candidates("config/app.json").last().unwrap().canonicalize().unwrap(),
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config/app.json").canonicalize().unwrap());
    }
}

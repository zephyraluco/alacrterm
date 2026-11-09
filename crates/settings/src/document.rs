use crate::{AppFile, SettingsStore, TerminalFile};
use anyhow::{Context as _, Result, ensure};
use serde::Deserialize;
use serde_json::{Map, Value, json};

/// File schema. Omitted terminal fields are resolved by the renderer's defaults.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct SettingsFile {
    #[serde(flatten)]
    pub app: AppFile,
    pub terminal: TerminalFile,
}

#[derive(Clone, Debug)]
pub struct SettingsSnapshot {
    pub document: Value,
    pub values: SettingsFile,
}

pub enum SettingsPatch {
    Application(Map<String, Value>),
    Terminal(Map<String, Value>),
}

pub const FONT_SIZE: (f64, f64) = (6., 48.);
pub const FONT_WEIGHT: (f64, f64) = (100., 900.);
pub const LINE_HEIGHT: (f64, f64) = (0.8, 2.5);
pub const CONTRAST: (f64, f64) = (0., 106.);

impl SettingsSnapshot {
    pub fn parse(document: Value) -> Result<Self> {
        ensure!(document.is_object(), "settings must be a JSON object");
        if let Some(terminal) = document.get("terminal") {
            ensure!(terminal.is_object(), "terminal must be a JSON object");
        }
        if let Some(mode) = document.get("theme_mode") {
            ensure!(
                matches!(mode.as_str(), Some("dark" | "light")),
                "theme_mode must be dark or light"
            );
        }
        let values: SettingsFile =
            serde_json::from_value(document.clone()).context("invalid settings fields")?;
        for (name, value, (min, max)) in [
            ("font_size", values.terminal.font_size, FONT_SIZE),
            ("font_weight", values.terminal.font_weight, FONT_WEIGHT),
            (
                "line_height_multiplier",
                values.terminal.line_height_multiplier,
                LINE_HEIGHT,
            ),
            (
                "minimum_contrast",
                values.terminal.minimum_contrast,
                CONTRAST,
            ),
        ] {
            ensure!(
                value.is_none_or(|v| v.is_finite() && v >= min as f32 && v <= max as f32),
                "terminal.{name} must be between {min} and {max}"
            );
        }
        if let Some(name) = &values.terminal.font_family {
            ensure!(
                !name.trim().is_empty(),
                "terminal.font_family must not be empty"
            );
        }
        if let Some(shape) = &values.terminal.cursor_shape {
            ensure!(
                ["block", "underline", "bar", "hollowblock", "hidden"]
                    .contains(&shape.to_ascii_lowercase().as_str()),
                "invalid terminal.cursor_shape"
            );
        }
        for (name, color) in values
            .terminal
            .colors
            .iter()
            .map(|(k, v)| (k.as_str(), Some(v)))
            .chain([
                ("selection_color", values.terminal.selection_color.as_ref()),
                (
                    "search_match_color",
                    values.terminal.search_match_color.as_ref(),
                ),
                ("link_color", values.terminal.link_color.as_ref()),
            ])
        {
            if let Some(color) = color {
                ensure!(
                    crate::color_schemes::parse_hex(color).is_some(),
                    "invalid terminal.{name} color"
                );
            }
        }
        Ok(Self { document, values })
    }

    pub fn patched(&self, patch: SettingsPatch) -> Result<Self> {
        let mut document = self.document.clone();
        match patch {
            SettingsPatch::Application(fields) => document.as_object_mut().unwrap().extend(fields),
            SettingsPatch::Terminal(fields) => {
                let terminal = document
                    .as_object_mut()
                    .unwrap()
                    .entry("terminal")
                    .or_insert_with(|| json!({}));
                terminal
                    .as_object_mut()
                    .context("terminal must be an object")?
                    .extend(fields);
            }
        }
        Self::parse(document)
    }
}

pub fn load_snapshot() -> Result<SettingsSnapshot> {
    SettingsSnapshot::parse(crate::unified_store()?.read()?)
}

pub(super) fn commit_at(store: &SettingsStore, patch: SettingsPatch) -> Result<SettingsSnapshot> {
    let next = SettingsSnapshot::parse(store.read()?)?.patched(patch)?;
    store.update(next.document.as_object().unwrap().clone())?;
    Ok(next)
}

pub fn commit(patch: SettingsPatch) -> Result<SettingsSnapshot> {
    commit_at(&crate::unified_store()?, patch)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn default_snapshot_and_scoped_changes_preserve_unknown_fields() {
        assert_eq!(
            SettingsSnapshot::parse(json!({}))
                .unwrap()
                .values
                .app
                .theme_mode,
            crate::AppThemeMode::Dark
        );
        let snapshot =
            SettingsSnapshot::parse(json!({"future":true,"terminal":{"future":7,"font_size":15}}))
                .unwrap();
        let snapshot = snapshot
            .patched(SettingsPatch::Application(Map::from_iter([(
                "theme_mode".into(),
                json!("light"),
            )])))
            .unwrap();
        let snapshot = snapshot
            .patched(SettingsPatch::Terminal(Map::from_iter([(
                "font_size".into(),
                json!(18),
            )])))
            .unwrap();
        assert_eq!(
            snapshot.document,
            json!({"future":true,"theme_mode":"light","terminal":{"future":7,"font_size":18}})
        );
    }
    #[test]
    fn invalid_fields_are_rejected_without_losing_the_last_snapshot() {
        for invalid in [
            json!({"terminal":{"font_size":1}}),
            json!({"theme_mode":"system"}),
            json!({"terminal":[]}),
            json!({"terminal":{"cursor_shape":"wrong"}}),
            json!({"terminal":{"selection_color":"wrong"}}),
        ] {
            assert!(SettingsSnapshot::parse(invalid).is_err());
        }
    }
    #[test]
    fn committed_snapshot_matches_disk_and_keeps_unrelated_preferences() {
        let (_cache, lease) = crate::tests::migration_fixture();
        std::fs::write(
            lease.path(),
            r#"{"theme_mode":"light","future":42,"terminal":{"font_size":15}}"#,
        )
        .unwrap();
        let next = commit_at(
            &SettingsStore::new(lease.path().to_owned()),
            SettingsPatch::Terminal(Map::from_iter([("font_size".into(), json!(19))])),
        )
        .unwrap();
        assert_eq!(
            next.document,
            SettingsStore::new(lease.path().to_owned()).read().unwrap()
        );
        assert_eq!(next.values.app.theme_mode, crate::AppThemeMode::Light);
        assert_eq!(next.values.terminal.font_size, Some(19.));
        assert_eq!(next.document["future"], 42);
    }

    #[test]
    fn invalid_or_failed_save_leaves_disk_unchanged() {
        let (_cache, lease) = crate::tests::migration_fixture();
        let store = SettingsStore::new(lease.path().to_owned());
        let original = "{\"theme_mode\":\"light\"}";
        std::fs::write(lease.path(), original).unwrap();
        assert!(
            commit_at(
                &store,
                SettingsPatch::Terminal(Map::from_iter([("font_size".into(), json!(0))]))
            )
            .is_err()
        );
        assert_eq!(std::fs::read_to_string(lease.path()).unwrap(), original);
        std::fs::remove_file(lease.path()).unwrap();
        std::fs::create_dir(lease.path()).unwrap();
        assert!(commit_at(&store, SettingsPatch::Application(Map::new())).is_err());
    }
}

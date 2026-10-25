//! User-facing completion policy, independent of shell integration and rendering.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct SuggestionSettings {
    pub enabled: bool,
    pub automatic: bool,
    pub history: bool,
    pub commands: bool,
    pub paths: bool,
    pub specifications: bool,
    pub dynamic: bool,
    pub fuzzy: bool,
    pub insert_trailing_space: bool,
    pub remote_integration: bool,
}
impl Default for SuggestionSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            automatic: true,
            history: true,
            commands: true,
            paths: true,
            specifications: true,
            dynamic: true,
            fuzzy: true,
            insert_trailing_space: false,
            remote_integration: true,
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn partial_configuration_preserves_defaults() {
        let settings: SuggestionSettings = serde_json::from_str(r#"{"automatic":false}"#).unwrap();
        assert!(!settings.automatic);
        assert!(settings.enabled && settings.paths && settings.remote_integration);
        assert_eq!(
            serde_json::from_str::<SuggestionSettings>(&serde_json::to_string(&settings).unwrap())
                .unwrap(),
            settings
        );
    }
}

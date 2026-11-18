use crate::backend::Backend;
use rig_core::completion::Effort;
use rig_core::{
    catalog::{Catalog, ModelSpec},
    providers::registry::ProviderId,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use url::Url;

/// Only references are persisted. API keys are resolved by the GUI credential
/// service or an explicitly selected environment variable when starting a run.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(tag = "source", rename_all = "snake_case", deny_unknown_fields)]
pub enum CredentialSource {
    #[default]
    None,
    Keychain {
        account: String,
    },
    Environment {
        variable: String,
    },
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Protocol {
    #[default]
    OpenaiChat,
    OpenaiResponses,
    AnthropicMessages,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelProfile {
    pub id: uuid::Uuid,
    pub name: String,
    pub model: String,
    pub base_url: String,
    #[serde(default)]
    pub protocol: Protocol,
    /// Optional official catalog identity, independent of the API model alias.
    #[serde(default)]
    pub metadata: Option<String>,
    #[serde(default)]
    pub credential: CredentialSource,
}

impl ModelProfile {
    pub fn validate(&self) -> Result<(), String> {
        if self.name.trim().is_empty() || self.model.trim().is_empty() || self.model.len() > 256 {
            return Err("model name and API model ID are required".into());
        }
        let url = Url::parse(&self.base_url).map_err(|_| "invalid API base URL")?;
        if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
            return Err("API base URL must use HTTP or HTTPS".into());
        }
        if !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err("put API credentials in the credential store, not the URL".into());
        }
        if let CredentialSource::Environment { variable } = &self.credential
            && (variable.is_empty()
                || !variable
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_'))
        {
            return Err("invalid credential environment variable".into());
        }
        Ok(())
    }

    fn parameters(&self) -> Result<ModelSpec, String> {
        self.validate()?;
        let catalog = Catalog::builtin();
        let lookup = |vendor, model| catalog.get(ProviderId::catalog(vendor)?, model);
        let found = if let Some(reference) = &self.metadata {
            reference
                .split_once('/')
                .and_then(|(vendor, model)| lookup(vendor, model))
        } else {
            let mut matches = [
                "openai",
                "anthropic",
                "google",
                "deepseek",
                "xai",
                "mistral",
                "meta",
                "moonshotai",
                "zhipuai",
                "minimax",
            ]
            .into_iter()
            .filter_map(|vendor| lookup(vendor, &self.model));
            let first = matches.next();
            if matches.next().is_none() {
                first
            } else {
                None
            }
        };
        let mut spec = found
            .map(|resolved| resolved.spec.clone())
            .unwrap_or_else(|| {
                ModelSpec::new(
                    ProviderId::declare("alacrterm").expect("valid provider"),
                    &self.model,
                )
                .with_tools(true)
            });
        spec.id = self.model.clone();
        Ok(spec)
    }

    /// Hide the control when the catalog doesn't list supported effort levels.
    /// A model's name is never treated as proof of support for a wire option.
    pub fn reasoning_levels(&self) -> Vec<String> {
        if self.protocol == Protocol::AnthropicMessages {
            return vec![];
        }
        self.parameters()
            .ok()
            .and_then(|p| p.reasoning.levels().map(|levels| levels.to_vec()))
            .unwrap_or_default()
            .into_iter()
            .filter_map(|effort| {
                serde_json::to_value(effort)
                    .ok()?
                    .as_str()
                    .map(str::to_owned)
            })
            .collect()
    }

    pub fn reasoning_params(&self, effort: Option<&str>) -> Result<Value, String> {
        let Some(effort) = effort else {
            return Ok(json!({}));
        };
        let requested: Effort =
            serde_json::from_value(json!(effort)).map_err(|_| "unknown reasoning effort")?;
        let parameters = self.parameters()?;
        let supported = parameters
            .reasoning
            .levels()
            .is_some_and(|levels| levels.contains(&requested));
        if !supported {
            return Err("this model does not list the selected reasoning effort".into());
        }
        Ok(match self.protocol {
            Protocol::OpenaiChat => json!({"reasoning_effort":effort}),
            Protocol::OpenaiResponses => json!({"reasoning":{"effort":effort}}),
            // Anthropic's budget/adaptive controls need their own catalog-backed
            // request mapping. Never send OpenAI fields to this protocol.
            Protocol::AnthropicMessages => {
                return Err("effort selection is not available for this protocol".into());
            }
        })
    }

    pub(crate) fn backend(&self, secret: Option<String>) -> Result<Backend, String> {
        Backend::new(
            self.protocol,
            self.base_url.clone(),
            secret,
            self.parameters()?,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    pub fn profile() -> ModelProfile {
        ModelProfile {
            id: uuid::Uuid::new_v4(),
            name: "Local model".into(),
            model: "unknown-local-model".into(),
            base_url: "http://localhost:11434/v1".into(),
            protocol: Protocol::OpenaiChat,
            metadata: None,
            credential: CredentialSource::None,
        }
    }
    #[test]
    fn unknown_models_do_not_invent_reasoning_controls() {
        let p = profile();
        assert!(p.reasoning_levels().is_empty());
        assert!(p.reasoning_params(Some("high")).is_err());
        assert_eq!(p.reasoning_params(None).unwrap(), json!({}));
    }
    #[test]
    fn catalog_metadata_is_separate_from_model_alias() {
        let mut p = profile();
        p.metadata = Some("openai/o3".into());
        assert!(p.reasoning_levels().contains(&"high".into()));
        assert_eq!(p.parameters().unwrap().id, "unknown-local-model");
        assert_eq!(
            p.reasoning_params(Some("high")).unwrap(),
            json!({"reasoning_effort":"high"})
        );
        p.protocol = Protocol::OpenaiResponses;
        assert_eq!(
            p.reasoning_params(Some("high")).unwrap(),
            json!({"reasoning":{"effort":"high"}})
        );
    }
    #[test]
    fn credentials_cannot_be_embedded_in_endpoint() {
        for url in [
            "https://user:secret@example.com/v1",
            "https://example.com/v1?key=secret",
            "file:///tmp/api",
        ] {
            let mut p = profile();
            p.base_url = url.into();
            assert!(p.validate().is_err());
        }
    }
}

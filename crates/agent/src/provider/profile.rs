use crate::backend::{Backend, Endpoint, TurnPlan, Wire};
use rig_core::completion::Effort;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use solaris_backend::ModelParameters;
use solaris_provider::ProviderConfig;
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
impl Protocol {
    fn wire(self) -> Wire {
        match self {
            Self::OpenaiChat => Wire::OpenAiChat,
            Self::OpenaiResponses => Wire::OpenAiResponses,
            Self::AnthropicMessages => Wire::AnthropicMessages,
        }
    }
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

    pub fn parameters(&self) -> Result<ModelParameters, String> {
        self.validate()?;
        let config = ProviderConfig::from_json(&json!({
            "providers":{"alacrterm":{"protocol":self.protocol,"base_url":self.base_url}},
            "models":{"selected":{"provider":"alacrterm","request_id":self.model,"metadata":self.metadata}}
        }).to_string())?;
        Ok(config.resolve("alacrterm", "selected").parameters)
    }

    /// Hide the control when the catalog doesn't list supported effort levels.
    /// A model's name is never treated as proof of support for a wire option.
    pub fn reasoning_levels(&self) -> Vec<String> {
        self.parameters()
            .ok()
            .and_then(|p| {
                p.facts()
                    .spec()?
                    .reasoning
                    .levels()
                    .map(|levels| levels.to_vec())
            })
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
            .facts()
            .spec()
            .and_then(|s| s.reasoning.levels())
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

    pub fn backend(&self, secret: Option<String>) -> Result<Backend, String> {
        Backend::new(TurnPlan {
            endpoint: Endpoint {
                wire: self.protocol.wire(),
                session_header: None,
                base_url: self.base_url.clone(),
                secret,
                name: "alacrterm".into(),
                label: self.name.clone(),
                env_keys: &[],
            },
            model: self.parameters()?,
            prompt_cache_key: None,
        })
        .map_err(|e| e.to_string())
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
        assert_eq!(p.parameters().unwrap().model(), "unknown-local-model");
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

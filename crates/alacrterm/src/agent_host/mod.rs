//! GPUI, credentials and connection adapters stay outside the agent core.
mod filesystem;
mod terminal;
use agent::{agent::runtime::AgentService, provider::ModelProfile};
use gpui::{App, Global};
use std::{path::PathBuf, sync::Arc};
pub(crate) use terminal::bind_terminal;

struct Service(Result<Arc<AgentService>, String>);
impl Global for Service {}

pub(crate) fn service(cx: &mut App) -> Result<Arc<AgentService>, String> {
    if !cx.has_global::<Service>() {
        let directory = settings::write_target("config/settings.json")
            .parent()
            .ok_or("missing settings directory")?
            .join("agent");
        cx.set_global(Service(
            AgentService::open(&directory)
                .map(Arc::new)
                .map_err(|e| e.to_string()),
        ));
    }
    cx.global::<Service>().0.clone()
}

fn settings_store() -> settings::SettingsStore {
    settings::SettingsStore::new(settings::write_target("config/settings.json"))
}

pub(crate) fn profiles() -> anyhow::Result<Vec<ModelProfile>> {
    let document = settings_store().read()?;
    let Some(value) = document.get("agent").and_then(|v| v.get("models")) else {
        return Ok(vec![]);
    };
    let models: Vec<ModelProfile> = serde_json::from_value(value.clone())?;
    for model in &models {
        model.validate().map_err(anyhow::Error::msg)?;
    }
    Ok(models)
}

pub(crate) fn save_profiles(models: &[ModelProfile]) -> anyhow::Result<()> {
    let store = settings_store();
    let document = store.read()?;
    let mut agent = document
        .get("agent")
        .cloned()
        .unwrap_or_else(|| serde_json::json!({}));
    anyhow::ensure!(agent.is_object(), "agent settings must be an object");
    agent["models"] = serde_json::to_value(models)?;
    store.update(serde_json::Map::from_iter([("agent".into(), agent)]))
}

pub(crate) fn credential_url(account: &str) -> String {
    format!("alacrterm-agent://{account}")
}

pub(crate) fn default_resource_directory() -> PathBuf {
    settings::write_target("config/settings.json")
        .parent()
        .unwrap_or(std::path::Path::new("."))
        .to_path_buf()
}

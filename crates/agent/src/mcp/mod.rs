//! MCP connections discover adapters; the application explicitly registers them.
pub use solaris_mcp::{McpConfig, McpManager, ServerConfig, ServerStatus};

pub struct McpResource {
    pub label: String,
    pub config: McpConfig,
    pub directory: std::path::PathBuf,
}

/// Accept the SDK configuration and the usual mcpServers wrapper. Environment
/// values follow Solaris semantics: they name variables, not literal secrets.
pub fn parse_config(text: &str) -> Result<McpConfig, String> {
    if text.len() > 64 * 1024 {
        return Err("MCP configuration exceeds 64 KiB".into());
    }
    let mut value: serde_json::Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
    let object = value
        .as_object_mut()
        .ok_or("MCP configuration must be an object")?;
    if let Some(servers) = object.remove("mcpServers") {
        if object.contains_key("servers") {
            return Err("choose either servers or mcpServers".into());
        }
        object.insert("servers".into(), servers);
    }
    let servers = object
        .get_mut("servers")
        .and_then(|v| v.as_object_mut())
        .ok_or("MCP servers are required")?;
    if servers.is_empty() || servers.len() > 16 {
        return Err("select between 1 and 16 MCP servers".into());
    }
    for server in servers.values_mut() {
        if server.get("transport").is_none() {
            let kind = if server.get("command").is_some() {
                "stdio"
            } else {
                "http"
            };
            server
                .as_object_mut()
                .ok_or("MCP server must be an object")?
                .insert("transport".into(), kind.into());
        }
    }
    serde_json::from_value(value).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn conventional_config_is_validated_without_launching_processes() {
        let config =
            parse_config(r#"{"mcpServers":{"docs":{"command":"mcp-docs","args":[]}}}"#).unwrap();
        assert!(matches!(config.servers["docs"], ServerConfig::Stdio { .. }));
        assert!(parse_config(r#"{"servers":{}}"#).is_err());
        assert!(parse_config(r#"{"servers":{"docs":{"command":"x","unexpected":true}}}"#).is_err());
    }
}

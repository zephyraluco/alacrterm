//! Alacrterm's shell scripts, activation policy, installation and OSC extensions.
mod local;
mod protocol;
mod remote;
use std::sync::Arc;
use terminal::{LaunchOptions, LocalSessionSetup, PreparedShell};

/// Bump when remote scripts or startup entry semantics change.
const VERSION: &str = "1.0.0";

#[derive(Clone, Copy)]
struct Scripts {
    bash: &'static str,
    zsh: &'static str,
}
fn scripts() -> Scripts {
    Scripts {
        bash: include_str!("../../../../scripts/shell_integration/bash.sh"),
        zsh: include_str!("../../../../scripts/shell_integration/zsh.zsh"),
    }
}

struct LocalIntegration;
impl LocalSessionSetup for LocalIntegration {
    fn prepare(
        &self,
        program: &str,
        args: &[String],
        env: &mut std::collections::HashMap<String, String>,
    ) -> anyhow::Result<Option<PreparedShell>> {
        Ok(
            local::prepare(program, args, env)?.map(|(args, files)| PreparedShell {
                args,
                resources: Box::new(files),
            }),
        )
    }
}

pub(crate) fn launch_options(remote_enabled: bool) -> LaunchOptions {
    let enabled = std::env::var_os("ALACRTERM_SHELL_INTEGRATION").as_deref()
        != Some(std::ffi::OsStr::new("0"));
    LaunchOptions {
        ssh_x11: Some(Arc::new(crate::x11::WayxTarget)),
        local_setup: enabled.then(|| Arc::new(LocalIntegration) as Arc<dyn LocalSessionSetup>),
        ssh_setup: (enabled && remote_enabled)
            .then(|| Arc::new(remote::RemoteIntegration) as Arc<dyn ssh::SessionSetup>),
        osc_decoder: Some(protocol::decode),
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod resource_tests {
    use super::*;
    #[test]
    fn local_startup_resources_live_until_the_host_lease_is_dropped() {
        let mut env = std::collections::HashMap::new();
        let prepared = LocalIntegration
            .prepare("bash", &[], &mut env)
            .unwrap()
            .unwrap();
        let script = std::path::PathBuf::from(&env["ALACRTERM_INTEGRATION_SCRIPT"]);
        assert!(script.is_file());
        assert!(!prepared.args.is_empty());
        drop(prepared);
        assert!(!script.exists());
    }
}

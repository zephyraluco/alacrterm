//! Optional host hooks and resources; terminal code owns no shell scripts.
use anyhow::Result;
use std::{collections::HashMap, sync::Arc};

pub type SessionResources = Box<dyn Send + Sync>;

pub struct PreparedShell {
    pub args: Vec<String>,
    /// Kept alive until the PTY event loop exits, including detached cleanup.
    pub resources: SessionResources,
}

pub trait LocalSessionSetup: Send + Sync {
    fn prepare(
        &self,
        program: &str,
        args: &[String],
        env: &mut HashMap<String, String>,
    ) -> Result<Option<PreparedShell>>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ShellMetadata {
    Command(String),
    Context(&'static str, String),
}
pub type OscDecoder = fn(&[u8]) -> Option<ShellMetadata>;

#[derive(Clone, Default)]
pub struct LaunchOptions {
    pub local_setup: Option<Arc<dyn LocalSessionSetup>>,
    pub ssh_setup: Option<Arc<dyn ssh::SessionSetup>>,
    pub osc_decoder: Option<OscDecoder>,
}

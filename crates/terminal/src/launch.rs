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
    History(HistoryEvent),
}
/// History transport is scoped to one shell invocation, not its working directory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HistoryEvent {
    Session(String),
    Begin {
        session: String,
        batch: u64,
        reset: bool,
    },
    Entry {
        session: String,
        batch: u64,
        index: u64,
        command: String,
    },
    End {
        session: String,
        batch: u64,
        count: usize,
    },
}
pub type OscDecoder = fn(&[u8]) -> Option<ShellMetadata>;

#[derive(Clone, Default)]
pub struct LaunchOptions {
    pub local_setup: Option<Arc<dyn LocalSessionSetup>>,
    pub ssh_x11: Option<Arc<dyn ssh::X11Target>>,
    pub ssh_setup: Option<Arc<dyn ssh::SessionSetup>>,
    pub osc_decoder: Option<OscDecoder>,
}

impl LaunchOptions {
    pub(crate) fn ssh_options(&self) -> ssh::SessionOptions {
        ssh::SessionOptions {
            setup: self.ssh_setup.clone(),
            x11: self.ssh_x11.clone(),
        }
    }
}

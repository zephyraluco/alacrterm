//! Host-provided preparation of a native SSH shell. No application policy here.
use crate::{HostKeyPolicy, SshParams, auth, handler::ClientHandler, session::EventSink};
use anyhow::{Result, bail};
use futures::future::BoxFuture;
use russh::{Channel, ChannelMsg, client};
use std::{sync::Arc, time::Duration};

pub trait SessionSetup: Send + Sync {
    fn prepare<'a>(&'a self, session: SessionContext<'a>) -> BoxFuture<'a, ()>;
}

pub struct SessionContext<'a> {
    params: &'a SshParams,
    channel: &'a mut Channel<client::Msg>,
    events: &'a EventSink,
}
impl<'a> SessionContext<'a> {
    pub(crate) fn new(
        params: &'a SshParams,
        channel: &'a mut Channel<client::Msg>,
        events: &'a EventSink,
    ) -> Self {
        Self {
            params,
            channel,
            events,
        }
    }

    /// Ask the server to pass an environment variable to the future shell.
    pub async fn set_env(&mut self, name: &str, value: &str, timeout: Duration) -> Result<bool> {
        tokio::time::timeout(timeout, async {
            self.channel.set_env(true, name, value).await?;
            loop {
                match self.channel.wait().await {
                    Some(ChannelMsg::Success) => return Ok(true),
                    Some(ChannelMsg::Failure) => return Ok(false),
                    Some(ChannelMsg::Close) | None => {
                        bail!("SSH session closed during environment request")
                    }
                    _ => {}
                }
            }
        })
        .await?
    }

    /// Reauthenticate on a separate transport, trusting only recorded host keys.
    /// Running commands here preserves the native login on the main transport.
    /// The caller supplies its overall timeout and fallback policy.
    pub async fn connect_auxiliary(&self) -> Result<SshCommandConnection> {
        let params = self.params;
        let handler = ClientHandler::new(
            params.user.clone(),
            params.host.clone(),
            params.port,
            HostKeyPolicy::Strict,
            params.known_hosts.clone(),
            None,
        );
        let config = Arc::new(client::Config {
            preferred: crate::compression::preferred(),
            nodelay: true,
            ..Default::default()
        });
        let mut handle =
            client::connect(config, (params.host.as_str(), params.port), handler).await?;
        auth::authenticate(&mut handle, params, self.events).await?;
        Ok(SshCommandConnection { handle })
    }
}

pub struct CommandOutput {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub exit_status: Option<u32>,
}

pub struct SshCommandConnection {
    handle: client::Handle<ClientHandler>,
}
impl SshCommandConnection {
    /// Run a command with bounded combined output and a caller-chosen timeout.
    pub async fn exec(
        &self,
        command: &str,
        timeout: Duration,
        output_limit: usize,
    ) -> Result<CommandOutput> {
        tokio::time::timeout(timeout, async {
            let mut channel = self.handle.channel_open_session().await?;
            channel.exec(true, command).await?;
            let mut output = CommandOutput {
                stdout: Vec::new(),
                stderr: Vec::new(),
                exit_status: None,
            };
            while let Some(message) = channel.wait().await {
                match message {
                    ChannelMsg::Data { data } => output.stdout.extend_from_slice(&data),
                    ChannelMsg::ExtendedData { data, .. } => output.stderr.extend_from_slice(&data),
                    ChannelMsg::ExitStatus { exit_status } => {
                        output.exit_status = Some(exit_status)
                    }
                    ChannelMsg::Failure => bail!("SSH command rejected"),
                    ChannelMsg::Close => break,
                    _ => {}
                }
                if output.stdout.len() + output.stderr.len() > output_limit {
                    let _ = channel.close().await;
                    bail!("SSH command output exceeded {output_limit} bytes");
                }
            }
            Ok(output)
        })
        .await?
    }

    pub async fn disconnect(self) -> Result<()> {
        self.handle
            .disconnect(russh::Disconnect::ByApplication, "", "")
            .await?;
        Ok(())
    }
}

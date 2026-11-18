use agent::{
    tools::{Cancel, CommandResult, TerminalContext, TerminalHost},
    types::{TargetBinding, TerminalId},
};
use async_trait::async_trait;
use futures::{
    SinkExt as _, StreamExt as _,
    channel::{mpsc, oneshot},
};
use gpui::{Context, Entity};
use std::{sync::Arc, time::Duration};
use terminal_view::TerminalView;

type ExecStream = (ssh::SshExecChannel, mpsc::UnboundedReceiver<ssh::SshEvent>);
enum Request {
    Validate(TargetBinding, oneshot::Sender<Result<(), String>>),
    Context(
        TargetBinding,
        usize,
        oneshot::Sender<Result<TerminalContext, String>>,
    ),
    Exec(
        TargetBinding,
        String,
        oneshot::Sender<Result<ExecStream, String>>,
    ),
    Insert(TargetBinding, String, oneshot::Sender<Result<(), String>>),
}
pub(super) struct Host {
    requests: mpsc::Sender<Request>,
    pub(super) files: Option<super::filesystem::Filesystem>,
}
pub(crate) struct BoundTerminal {
    pub target: TargetBinding,
    pub tools: Vec<Arc<dyn agent::tools::Tool>>,
}
impl Host {
    pub(super) async fn validate(&self, target: &TargetBinding) -> Result<(), String> {
        let (tx, rx) = oneshot::channel();
        self.requests
            .clone()
            .send(Request::Validate(target.clone(), tx))
            .await
            .map_err(|_| "terminal host unavailable")?;
        rx.await
            .map_err(|_| "terminal host unavailable".into())
            .and_then(|v| v)
    }
}

fn check(
    view: &TerminalView,
    target: &TargetBinding,
    expected: TerminalId,
    cx: &gpui::App,
) -> Result<(), String> {
    if target.terminal != expected
        || target.generation != view.connection_generation()
        || !view.is_connected(cx)
    {
        return Err("The selected terminal is closed, disconnected or has reconnected. Select it again before starting a new run.".into());
    }
    Ok(())
}

pub(crate) fn bind_terminal<T: 'static>(
    view: &Entity<TerminalView>,
    id: TerminalId,
    label: String,
    mode: agent::types::Mode,
    cx: &mut Context<T>,
) -> Result<BoundTerminal, String> {
    let generation = view.read(cx).connection_generation();
    let target = TargetBinding {
        terminal: id,
        generation,
        label,
    };
    check(view.read(cx), &target, id, cx)?;
    let files = if let Some(fs) = view.read(cx).remote_fs(cx) {
        Some(super::filesystem::Filesystem::Remote {
            fs,
            root: view.read(cx).remote_directory(cx),
        })
    } else {
        view.read(cx)
            .working_directory(cx)
            .map(super::filesystem::Filesystem::Local)
    };
    let weak = view.downgrade();
    let (sender, mut requests) = mpsc::channel(8);
    cx.spawn(async move |_, cx| {
        while let Some(request) = requests.next().await {
            match request {
                Request::Validate(target, reply) => {
                    let result = weak
                        .read_with(cx, |view, cx| check(view, &target, id, cx))
                        .map_err(|_| "terminal was closed".to_owned())
                        .and_then(|v| v);
                    let _ = reply.send(result);
                }
                Request::Context(target, lines, reply) => {
                    let result = weak
                        .read_with(cx, |view, cx| {
                            check(view, &target, id, cx)?;
                            let (mut output, mut selection) = view
                                .context_text(lines, cx)
                                .ok_or("terminal content unavailable")?;
                            let mut truncated = output.lines().count() >= lines;
                            truncated |= truncate(&mut output, 64 * 1024, true);
                            if let Some(selection) = selection.as_mut() {
                                truncated |= truncate(selection, 64 * 1024, false);
                            }
                            Ok(TerminalContext {
                                cwd: view.remote_directory(cx).or_else(|| {
                                    view.working_directory(cx)
                                        .map(|p| p.to_string_lossy().into_owned())
                                }),
                                selection,
                                output,
                                truncated,
                            })
                        })
                        .map_err(|_| "terminal was closed".to_owned())
                        .and_then(|v| v);
                    let _ = reply.send(result);
                }
                Request::Exec(target, command, reply) => {
                    let result = weak
                        .read_with(cx, |view, cx| {
                            check(view, &target, id, cx)?;
                            let exec = view
                                .remote_exec(cx)
                                .ok_or("SSH exec is available only for a selected SSH terminal")?;
                            let (sender, events) = mpsc::unbounded();
                            let guard = exec.exec(command, sender).map_err(|e| e.to_string())?;
                            Ok((guard, events))
                        })
                        .map_err(|_| "terminal was closed".to_owned())
                        .and_then(|v| v);
                    let _ = reply.send(result);
                }
                Request::Insert(target, text, reply) => {
                    let result = weak
                        .update(cx, |view, cx| {
                            check(view, &target, id, cx)?;
                            view.insert_at_shell(&text, cx)
                        })
                        .map_err(|_| "terminal was closed".to_owned())
                        .and_then(|v| v);
                    let _ = reply.send(result);
                }
            }
        }
    })
    .detach();
    let host = Arc::new(Host {
        requests: sender,
        files,
    });
    let mut tools = agent::tools::terminal_tools(host.clone(), target.clone(), mode);
    if host.files.is_some() {
        tools.extend(agent::tools::filesystem_tools(host, target.clone(), mode));
    }
    Ok(BoundTerminal { target, tools })
}

fn truncate(value: &mut String, limit: usize, tail: bool) -> bool {
    if value.len() <= limit {
        return false;
    }
    if tail {
        let mut start = value.len() - limit;
        while !value.is_char_boundary(start) {
            start += 1;
        }
        value.drain(..start);
    } else {
        let mut end = limit;
        while !value.is_char_boundary(end) {
            end -= 1;
        }
        value.truncate(end);
    }
    true
}

#[async_trait]
impl TerminalHost for Host {
    async fn context(
        &self,
        target: &TargetBinding,
        lines: usize,
    ) -> Result<TerminalContext, String> {
        let (tx, rx) = oneshot::channel();
        self.requests
            .clone()
            .send(Request::Context(target.clone(), lines, tx))
            .await
            .map_err(|_| "terminal host unavailable")?;
        rx.await
            .map_err(|_| "terminal host unavailable".into())
            .and_then(|v| v)
    }
    async fn insert_text(&self, target: &TargetBinding, text: &str) -> Result<(), String> {
        let (tx, rx) = oneshot::channel();
        self.requests
            .clone()
            .send(Request::Insert(target.clone(), text.into(), tx))
            .await
            .map_err(|_| "terminal host unavailable")?;
        rx.await
            .map_err(|_| "terminal host unavailable".into())
            .and_then(|v| v)
    }
    async fn ssh_exec(
        &self,
        target: &TargetBinding,
        command: &str,
        timeout_seconds: u64,
        cancel: Cancel,
    ) -> Result<CommandResult, String> {
        let (tx, rx) = oneshot::channel();
        self.requests
            .clone()
            .send(Request::Exec(target.clone(), command.into(), tx))
            .await
            .map_err(|_| "terminal host unavailable")?;
        let (_guard, mut events) = rx.await.map_err(|_| "terminal host unavailable")??;
        let collect = async {
            let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
            let mut truncated = false;
            while let Some(event) = events.next().await {
                match event {
                    ssh::SshEvent::Data(bytes) => {
                        let remaining =
                            (64 * 1024usize).saturating_sub(stdout.len() + stderr.len());
                        let count = remaining.min(bytes.len());
                        truncated |= count < bytes.len();
                        if bytes.is_stderr() {
                            stderr.extend_from_slice(&bytes[..count]);
                        } else {
                            stdout.extend_from_slice(&bytes[..count]);
                        }
                    }
                    ssh::SshEvent::Closed {
                        exit_status,
                        reason,
                    } => {
                        if let Some(reason) = reason {
                            return Err(reason);
                        }
                        return Ok(CommandResult {
                            stdout: String::from_utf8_lossy(&stdout).into_owned(),
                            stderr: String::from_utf8_lossy(&stderr).into_owned(),
                            exit_code: exit_status.and_then(|code| i32::try_from(code).ok()),
                            truncated,
                        });
                    }
                    _ => {}
                }
            }
            Err("SSH exec channel closed without an exit result".into())
        };
        tokio::select! {
            biased;
            _ = async { while !cancel.is_cancelled() {tokio::time::sleep(Duration::from_millis(20)).await;} } => Err("SSH channel closed after cancellation; remote process termination is not guaranteed".into()),
            result = tokio::time::timeout(Duration::from_secs(timeout_seconds),collect) => result.map_err(|_|"SSH exec timed out; channel closed, but the remote process may still be running".to_owned()).and_then(|v|v),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn context_limits_preserve_utf8_for_head_and_tail() {
        for tail in [false, true] {
            let mut text = "abc世界".repeat(20);
            assert!(truncate(&mut text, 17, tail));
            assert!(text.len() <= 17);
            assert!(std::str::from_utf8(text.as_bytes()).is_ok());
        }
    }
}

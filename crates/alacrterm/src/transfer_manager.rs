use futures::StreamExt as _;
use gpui::{AsyncApp, Context, EventEmitter, SharedString, WeakEntity};
use std::{collections::HashMap, path::PathBuf};
use terminal::{SessionId, SshError, SshFs, Transfer, TransferEvent, TransferProgress};

use crate::preview_window::PreviewRequest;

pub(crate) enum TransferDone {
    Preview(PreviewRequest),
    OpenExternally(PathBuf),
    Nothing,
}

pub(crate) struct TransferCompleted {
    pub session: SessionId,
    pub label: SharedString,
    pub result: Result<(), SshError>,
    pub refresh: Option<String>,
    pub done: TransferDone,
}

pub(crate) use transfer::TransferState;
#[derive(Default)]
pub(crate) struct TransferManager {
    book: transfer::TransferBook,
    tasks: HashMap<u64, gpui::Task<()>>,
}
impl TransferManager {
    pub(crate) fn active_count(&self) -> usize {
        self.book.active_count()
    }
    pub(crate) fn transfers(&self) -> &[TransferState] {
        self.book.transfers()
    }
    pub(crate) fn transfer(&self, id: u64) -> Option<TransferState> {
        self.book.transfer(id)
    }
    pub(crate) fn cancel_all(&mut self, cx: &mut Context<Self>) {
        self.book.cancel_all();
        cx.notify();
    }
    pub(crate) fn cancel(&mut self, id: u64, cx: &mut Context<Self>) {
        self.book.cancel(id);
        cx.notify();
    }
    pub(crate) fn start(
        &mut self,
        source: SshFs,
        label: SharedString,
        refresh: Option<String>,
        transfer: Transfer,
        done: TransferDone,
        cx: &mut Context<Self>,
    ) {
        let Transfer { mut events, cancel } = transfer;
        let id = self.book.start(source.clone(), label.to_string(), cancel);
        cx.notify();

        let task = cx.spawn(move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let mut cx = cx.clone();
            async move {
                while let Some(event) = events.next().await {
                    match event {
                        TransferEvent::Progress(progress) => {
                            if this
                                .update(&mut cx, |manager, cx| manager.progress(id, progress, cx))
                                .is_err()
                            {
                                return;
                            }
                        }
                        TransferEvent::Done(result) => {
                            let _ = this.update(&mut cx, |manager, cx| {
                                manager.complete(
                                    id,
                                    TransferCompleted {
                                        session: source.session_id(),
                                        label,
                                        result,
                                        refresh,
                                        done,
                                    },
                                    cx,
                                );
                            });
                            return;
                        }
                    }
                }
                let _ = this.update(&mut cx, |manager, cx| {
                    manager.complete(
                        id,
                        TransferCompleted {
                            session: source.session_id(),
                            label,
                            refresh,
                            done,
                            result: Err(SshError::Sftp("传输中断:远端文件系统线程已退出".into())),
                        },
                        cx,
                    );
                });
            }
        });
        self.tasks.insert(id, task);
    }

    fn progress(&mut self, id: u64, progress: TransferProgress, cx: &mut Context<Self>) {
        self.book.progress(id, progress);
        cx.notify();
    }
    fn complete(&mut self, id: u64, completion: TransferCompleted, cx: &mut Context<Self>) {
        // The task is delivering its final result. Release its handle without
        // cancelling the update that is currently dispatching completion effects.
        if let Some(task) = self.tasks.remove(&id) { task.detach(); }
        self.book.remove(id);
        cx.emit(completion);
        cx.notify();
    }
}
impl EventEmitter<TransferCompleted> for TransferManager {}

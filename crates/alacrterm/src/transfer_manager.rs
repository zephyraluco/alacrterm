use futures::StreamExt as _;
use gpui::{AsyncApp, Context, SharedString, WeakEntity};
use terminal::{SshFs, Transfer, TransferEvent, TransferProgress};

use crate::sidebar_panel::files::{FilesState, TransferDone};

pub(crate) use transfer::TransferState;
#[derive(Default)]
pub(crate) struct TransferManager {
    book: transfer::TransferBook,
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
        files: WeakEntity<FilesState>,
        cx: &mut Context<Self>,
    ) {
        let Transfer { mut events, cancel } = transfer;
        let id = self.book.start(source.clone(), label.to_string(), cancel);
        cx.notify();

        cx.spawn(move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
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
                            let _ = files.update(&mut cx, |files, cx| {
                                files.on_transfer_done(
                                    source.session_id(),
                                    label,
                                    result,
                                    refresh,
                                    done,
                                    cx,
                                )
                            });
                            let _ = this.update(&mut cx, |manager, cx| manager.remove(id, cx));
                            return;
                        }
                    }
                }
                let _ = files.update(&mut cx, |files, cx| files.report_transfer_lost(label, cx));
                let _ = this.update(&mut cx, |manager, cx| manager.remove(id, cx));
            }
        })
        .detach();
    }

    fn progress(&mut self, id: u64, progress: TransferProgress, cx: &mut Context<Self>) {
        self.book.progress(id, progress);
        cx.notify();
    }
    fn remove(&mut self, id: u64, cx: &mut Context<Self>) {
        self.book.remove(id);
        cx.notify();
    }
}

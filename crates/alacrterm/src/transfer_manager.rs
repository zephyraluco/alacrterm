use futures::StreamExt as _;
use gpui::{AsyncApp, Context, SharedString, WeakEntity};
use terminal::{CancelToken, SshFs, Transfer, TransferEvent, TransferProgress};

use crate::sidebar_panel::files::{FilesState, TransferDone};

#[derive(Clone)]
pub(crate) struct TransferState {
    pub(crate) id: u64,
    pub(crate) source: SshFs,
    pub(crate) cancelling: bool,
    pub(crate) label: SharedString,
    pub(crate) files_done: u64,
    pub(crate) files_total: u64,
    pub(crate) bytes_done: u64,
    pub(crate) bytes_total: u64,
    cancel: CancelToken,
}

impl TransferState {
    fn update_progress(&mut self, progress: TransferProgress) {
        self.label = progress.label.into();
        self.files_done = progress.files_done;
        self.files_total = progress.files_total;
        self.bytes_done = progress.bytes_done;
        self.bytes_total = progress.bytes_total;
    }

    fn request_cancel(&mut self) {
        self.cancel.cancel();
        self.cancelling = true;
    }

    pub(crate) fn percent(&self) -> f32 {
        if self.bytes_total > 0 {
            (self.bytes_done as f32 / self.bytes_total as f32 * 100.).clamp(0., 100.)
        } else if self.files_total > 0 {
            (self.files_done as f32 / self.files_total as f32 * 100.).clamp(0., 100.)
        } else {
            0.
        }
    }
}

#[derive(Default)]
pub(crate) struct TransferManager {
    transfers: Vec<TransferState>,
    next_id: u64,
}

impl TransferManager {
    pub(crate) fn active_count(&self) -> usize {
        self.transfers.len()
    }

    pub(crate) fn cancel_all(&mut self, cx: &mut Context<Self>) {
        if self.transfers.is_empty() {
            return;
        }
        for transfer in &mut self.transfers {
            transfer.request_cancel();
        }
        cx.notify();
    }

    pub(crate) fn transfers(&self) -> &[TransferState] {
        &self.transfers
    }

    pub(crate) fn transfer(&self, id: u64) -> Option<TransferState> {
        self.transfers.iter().find(|transfer| transfer.id == id).cloned()
    }

    pub(crate) fn cancel(&mut self, id: u64, cx: &mut Context<Self>) {
        if let Some(transfer) = self.transfers.iter_mut().find(|transfer| transfer.id == id) {
            transfer.request_cancel();
            cx.notify();
        }
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
        let id = self.next_id;
        self.next_id = self.next_id.checked_add(1).expect("transfer identity exhausted");
        let Transfer { mut events, cancel } = transfer;
        self.transfers.insert(0, TransferState {
            id,
            source: source.clone(),
            cancelling: false,
            label: label.clone(),
            files_done: 0,
            files_total: 0,
            bytes_done: 0,
            bytes_total: 0,
            cancel,
        });
        cx.notify();

        cx.spawn(move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let mut cx = cx.clone();
            async move {
                while let Some(event) = events.next().await {
                    match event {
                        TransferEvent::Progress(progress) => {
                            if this.update(&mut cx, |manager, cx| manager.progress(id, progress, cx)).is_err() {
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
                let _ = files.update(&mut cx, |files, cx| {
                    files.report_transfer_lost(label, cx)
                });
                let _ = this.update(&mut cx, |manager, cx| manager.remove(id, cx));
            }
        }).detach();
    }

    fn progress(&mut self, id: u64, progress: TransferProgress, cx: &mut Context<Self>) {
        if let Some(transfer) = self.transfers.iter_mut().find(|transfer| transfer.id == id) {
            transfer.update_progress(progress);
            cx.notify();
        }
    }

    fn remove(&mut self, id: u64, cx: &mut Context<Self>) {
        self.transfers.retain(|transfer| transfer.id != id);
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn transfer() -> TransferState {
        TransferState {
            id: 0,
            source: SshFs::new(terminal::SshParams::new("example.test", 22, "user")),
            cancelling: false,
            label: "first".into(),
            files_done: 0,
            files_total: 0,
            bytes_done: 0,
            bytes_total: 0,
            cancel: CancelToken::default(),
        }
    }

    #[test]
    fn progress_updates_bytes_and_label() {
        let mut transfer = transfer();
        transfer.update_progress(TransferProgress {
            label: "second".into(),
            files_done: 1,
            files_total: 2,
            bytes_done: 25,
            bytes_total: 100,
        });
        assert_eq!(transfer.percent(), 25.);
        assert_eq!(transfer.label.as_ref(), "second");
    }

    #[test]
    fn cancellation_keeps_state_until_worker_finishes() {
        let mut transfer = transfer();
        transfer.request_cancel();
        assert!(transfer.cancelling);
        assert!(transfer.cancel.is_cancelled());
    }
}
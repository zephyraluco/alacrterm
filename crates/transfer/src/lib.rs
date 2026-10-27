//! UI-independent active transfer tracking and cancellation.
use ssh::{CancelToken, SshFs, TransferProgress};
#[derive(Clone)]
pub struct TransferState {
    pub id: u64,
    pub source: SshFs,
    pub cancelling: bool,
    pub label: String,
    pub files_done: u64,
    pub files_total: u64,
    pub bytes_done: u64,
    pub bytes_total: u64,
    cancel: CancelToken,
}

impl TransferState {
    fn update_progress(&mut self, progress: TransferProgress) {
        self.label = progress.label;
        self.files_done = progress.files_done;
        self.files_total = progress.files_total;
        self.bytes_done = progress.bytes_done;
        self.bytes_total = progress.bytes_total;
    }

    fn request_cancel(&mut self) {
        self.cancel.cancel();
        self.cancelling = true;
    }

    pub fn percent(&self) -> f32 {
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
pub struct TransferBook {
    transfers: Vec<TransferState>,
    next_id: u64,
}

impl TransferBook {
    pub fn active_count(&self) -> usize {
        self.transfers.len()
    }

    pub fn cancel_all(&mut self) {
        if self.transfers.is_empty() {
            return;
        }
        for transfer in &mut self.transfers {
            transfer.request_cancel();
        }
    }

    pub fn transfers(&self) -> &[TransferState] {
        &self.transfers
    }

    pub fn transfer(&self, id: u64) -> Option<TransferState> {
        self.transfers
            .iter()
            .find(|transfer| transfer.id == id)
            .cloned()
    }

    pub fn cancel(&mut self, id: u64) {
        if let Some(transfer) = self.transfers.iter_mut().find(|transfer| transfer.id == id) {
            transfer.request_cancel();
        }
    }

    /// Register a worker; cancellation leaves it visible until completion.
    pub fn start(&mut self, source: SshFs, label: String, cancel: CancelToken) -> u64 {
        let id = self.next_id;
        self.next_id = self
            .next_id
            .checked_add(1)
            .expect("transfer identity exhausted");
        self.transfers.insert(
            0,
            TransferState {
                id,
                source,
                cancelling: false,
                label,
                files_done: 0,
                files_total: 0,
                bytes_done: 0,
                bytes_total: 0,
                cancel,
            },
        );
        id
    }
    pub fn progress(&mut self, id: u64, progress: TransferProgress) {
        if let Some(transfer) = self.transfers.iter_mut().find(|transfer| transfer.id == id) {
            transfer.update_progress(progress);
        }
    }

    pub fn remove(&mut self, id: u64) {
        self.transfers.retain(|transfer| transfer.id != id);
    }
}

// The owner of active work must also cancel it when the application service ends.
impl Drop for TransferBook {
    fn drop(&mut self) {
        self.cancel_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn transfer() -> TransferState {
        TransferState {
            id: 0,
            source: SshFs::new(ssh::SshParams::new("example.test", 22, "user")),
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
    fn dropping_the_transfer_owner_cancels_remaining_workers() {
        let cancel = CancelToken::default();
        let mut book = TransferBook::default();
        let source = SshFs::new(ssh::SshParams::new("example.test", 22, "user"));
        book.start(source, "active".into(), cancel.clone());
        drop(book);
        assert!(cancel.is_cancelled());
    }

    #[test]
    fn cancellation_retains_worker_until_completion_and_ids_are_not_reused() {
        let mut book = TransferBook::default();
        let cancel = CancelToken::default();
        let source = SshFs::new(ssh::SshParams::new("example.test", 22, "user"));
        let first = book.start(source.clone(), "first".into(), cancel.clone());
        book.cancel(first);
        assert!(book.transfer(first).unwrap().cancelling);
        assert_eq!(book.active_count(), 1);
        book.remove(first);
        let next = book.start(source, "next".into(), CancelToken::default());
        assert_ne!(first, next);
        book.progress(
            first,
            TransferProgress {
                label: "stale".into(),
                files_done: 1,
                files_total: 1,
                bytes_done: 1,
                bytes_total: 1,
            },
        );
        assert_eq!(book.transfer(next).unwrap().label, "next");
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
        assert_eq!(transfer.label.as_str(), "second");
    }

    #[test]
    fn cancellation_keeps_state_until_worker_finishes() {
        let mut transfer = transfer();
        transfer.request_cancel();
        assert!(transfer.cancelling);
        assert!(transfer.cancel.is_cancelled());
    }
}

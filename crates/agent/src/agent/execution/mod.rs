use crate::types::TerminalId;
use std::{
    collections::HashSet,
    sync::{Arc, Mutex},
};
/// An async operation owns a logical terminal lease, never a UI entity lock.
#[derive(Clone, Default)]
pub struct TerminalLeases(Arc<Mutex<HashSet<TerminalId>>>);
pub struct TerminalLease {
    id: TerminalId,
    leases: TerminalLeases,
}
impl TerminalLeases {
    pub fn acquire(&self, id: TerminalId) -> Result<TerminalLease, String> {
        if !self
            .0
            .lock()
            .map_err(|_| "terminal leases unavailable")?
            .insert(id)
        {
            return Err("another AI task is using this terminal".into());
        }
        Ok(TerminalLease {
            id,
            leases: self.clone(),
        })
    }
}
impl Drop for TerminalLease {
    fn drop(&mut self) {
        if let Ok(mut leases) = self.leases.0.lock() {
            leases.remove(&self.id);
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn concurrent_runs_cannot_share_a_terminal_and_other_terminals_are_independent() {
        let leases = TerminalLeases::default();
        let a = TerminalId::default();
        let b = TerminalId::default();
        let first = leases.acquire(a).unwrap();
        assert!(leases.acquire(a).is_err());
        let other = leases.acquire(b).unwrap();
        drop(first);
        assert!(leases.acquire(a).is_ok());
        drop(other);
    }
}

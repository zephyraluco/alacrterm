//! Connection state and identity, owned by the terminal instance rather than its view.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConnectionState {
    Connecting,
    Running,
    Exited,
}

pub(crate) struct Lifecycle {
    state: ConnectionState,
    epoch: u64,
}
impl Lifecycle {
    pub fn new(remote: bool) -> Self {
        Self {
            state: if remote {
                ConnectionState::Connecting
            } else {
                ConnectionState::Running
            },
            epoch: 0,
        }
    }
    pub fn state(&self) -> ConnectionState {
        self.state
    }
    pub fn epoch(&self) -> u64 {
        self.epoch
    }
    pub fn is_current(&self, epoch: u64) -> bool {
        self.epoch == epoch
    }
    pub fn connected(&mut self) {
        if self.state == ConnectionState::Connecting {
            self.state = ConnectionState::Running;
        }
    }
    pub fn finish(&mut self) -> bool {
        let changed = self.state != ConnectionState::Exited;
        self.state = ConnectionState::Exited;
        changed
    }
    pub fn reconnect(&mut self) {
        self.epoch = self.epoch.wrapping_add(1);
        self.state = ConnectionState::Connecting;
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reconnect_invalidates_old_events_and_does_not_revive_a_closed_attempt() {
        let mut state = Lifecycle::new(true);
        let old = state.epoch();
        state.connected();
        assert_eq!(state.state(), ConnectionState::Running);
        assert!(state.finish());
        assert!(!state.finish());
        state.connected();
        assert_eq!(state.state(), ConnectionState::Exited);
        state.reconnect();
        assert!(!state.is_current(old));
        assert_eq!(state.state(), ConnectionState::Connecting);
        state.connected();
        assert_eq!(state.state(), ConnectionState::Running);
    }
    #[test]
    fn local_process_exit_is_no_longer_reported_as_connected() {
        let mut state = Lifecycle::new(false);
        assert_eq!(state.state(), ConnectionState::Running);
        state.finish();
        assert_eq!(state.state(), ConnectionState::Exited);
    }
}

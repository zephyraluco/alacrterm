//! Bounded, atomic history imports. A shell invocation owns its own cache.
use std::time::{Duration, Instant};
use terminal::HistoryEvent;

const LIMIT: usize = 1000;
const MAX_BYTES: usize = LIMIT * 4096;
const TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Default)]
pub(crate) struct History {
    session: Option<String>,
    last_batch: u64,
    pending: Option<Batch>,
    entries: Vec<String>, // newest first
}
struct Batch {
    id: u64,
    reset: bool,
    started: Instant,
    entries: Vec<String>, // oldest first, as sent by the shell
    last_index: Option<u64>,
    bytes: usize,
}
impl History {
    pub fn entries(&self) -> &[String] {
        &self.entries
    }
    pub fn clear(&mut self) {
        self.entries.clear();
        self.pending = None;
    }
    pub fn reset(&mut self) {
        *self = Self::default();
    }
    pub fn record(&mut self, command: String) {
        self.entries.retain(|old| old != &command);
        self.entries.insert(0, command);
        self.entries.truncate(LIMIT);
    }
    pub fn receive(&mut self, event: HistoryEvent, enabled: bool) -> bool {
        self.receive_at(event, enabled, Instant::now())
    }
    fn receive_at(&mut self, event: HistoryEvent, enabled: bool, now: Instant) -> bool {
        if self
            .pending
            .as_ref()
            .is_some_and(|batch| now.duration_since(batch.started) > TIMEOUT)
        {
            self.pending = None;
        }
        if let HistoryEvent::Session(session) = event {
            if self.session.as_ref() == Some(&session) {
                return false;
            }
            self.session = Some(session);
            self.last_batch = 0;
            self.clear();
            return true;
        }
        if !enabled {
            self.clear();
            return false;
        }
        match event {
            HistoryEvent::Begin {
                session,
                batch,
                reset,
            } => {
                if self.session.as_ref() != Some(&session) || batch <= self.last_batch {
                    return false;
                }
                self.last_batch = batch;
                self.pending = Some(Batch {
                    id: batch,
                    reset,
                    started: now,
                    entries: Vec::new(),
                    last_index: None,
                    bytes: 0,
                });
            }
            HistoryEvent::Entry {
                session,
                batch,
                index,
                command,
            } => {
                if self.session.as_ref() != Some(&session) {
                    return false;
                }
                let Some(pending) = &mut self.pending else {
                    return false;
                };
                if batch != pending.id {
                    return false;
                }
                if pending.entries.len() >= LIMIT
                    || command.len() > 4096
                    || command.trim().is_empty()
                    || command.chars().any(char::is_control)
                    || pending.last_index.is_some_and(|old| index <= old)
                    || pending.bytes + command.len() > MAX_BYTES
                {
                    self.pending = None;
                    return false;
                }
                pending.bytes += command.len();
                pending.last_index = Some(index);
                pending.entries.push(command);
            }
            HistoryEvent::End {
                session,
                batch,
                count,
            } => {
                if self.session.as_ref() != Some(&session)
                    || !self
                        .pending
                        .as_ref()
                        .is_some_and(|pending| pending.id == batch)
                {
                    return false;
                }
                let pending = self.pending.take().unwrap();
                if pending.entries.len() != count {
                    return false;
                }
                if pending.reset {
                    self.entries.clear();
                }
                for command in pending.entries {
                    self.record(command);
                }
                return true;
            }
            HistoryEvent::Session(_) => unreachable!(),
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn begin(session: &str, batch: u64, reset: bool) -> HistoryEvent {
        HistoryEvent::Begin {
            session: session.into(),
            batch,
            reset,
        }
    }
    fn entry(session: &str, batch: u64, index: u64, command: &str) -> HistoryEvent {
        HistoryEvent::Entry {
            session: session.into(),
            batch,
            index,
            command: command.into(),
        }
    }
    fn end(session: &str, batch: u64, count: usize) -> HistoryEvent {
        HistoryEvent::End {
            session: session.into(),
            batch,
            count,
        }
    }
    #[test]
    fn atomic_snapshot_append_deduplication_and_clear() {
        let mut h = History::default();
        h.receive(HistoryEvent::Session("shell1".into()), true);
        h.receive(begin("shell1", 1, true), true);
        h.receive(entry("shell1", 1, 1, "git status"), true);
        h.receive(entry("shell1", 1, 2, "git log"), true);
        assert!(h.entries().is_empty());
        assert!(h.receive(end("shell1", 1, 2), true));
        assert_eq!(h.entries(), ["git log", "git status"]);
        h.receive(begin("shell1", 2, false), true);
        h.receive(entry("shell1", 2, 3, "git status"), true);
        h.receive(end("shell1", 2, 1), true);
        assert_eq!(h.entries(), ["git status", "git log"]);
        h.receive(begin("shell1", 3, true), true);
        h.receive(end("shell1", 3, 0), true);
        assert!(h.entries().is_empty());
    }
    #[test]
    fn sessions_disabled_imports_replays_and_incomplete_batches_are_isolated() {
        let mut h = History::default();
        h.receive(HistoryEvent::Session("local".into()), true);
        h.receive(begin("local", 1, true), true);
        h.receive(entry("local", 1, 1, "local secret"), true);
        h.receive(HistoryEvent::Session("ssh".into()), true);
        assert!(!h.receive(end("local", 1, 1), true));
        h.receive(begin("local", 2, true), true);
        assert!(!h.receive(end("local", 2, 0), true));
        h.receive(begin("ssh", 1, true), true);
        h.receive(entry("ssh", 1, 1, "remote command"), true);
        assert!(!h.receive(end("ssh", 1, 2), true));
        h.receive(begin("ssh", 1, true), true); // no replay
        assert!(!h.receive(end("ssh", 1, 0), true));
        h.receive(begin("ssh", 2, true), false);
        h.receive(entry("ssh", 2, 1, "disabled"), false);
        h.receive(end("ssh", 2, 1), false);
        assert!(h.entries().is_empty());
    }
    #[test]
    fn expired_or_invalid_batches_never_replace_existing_history() {
        let mut h = History::default();
        h.record("keep".into());
        h.session = Some("s".into());
        let now = Instant::now();
        h.receive_at(begin("s", 1, true), true, now);
        h.receive_at(
            entry("s", 1, 1, "late"),
            true,
            now + TIMEOUT + Duration::from_secs(1),
        );
        assert!(!h.receive_at(end("s", 1, 1), true, now + TIMEOUT + Duration::from_secs(1)));
        for (batch, command) in [(2, "bad\nline"), (3, "\u{1b}evil"), (4, "")] {
            h.receive(begin("s", batch, true), true);
            h.receive(entry("s", batch, 1, command), true);
            assert!(!h.receive(end("s", batch, 0), true));
        }
        assert_eq!(h.entries(), ["keep"]);
    }
    #[test]
    fn history_is_bounded_to_the_most_recent_thousand_unique_commands() {
        let mut h = History::default();
        for index in 0..1100 {
            h.record(format!("echo {index}"));
        }
        assert_eq!(h.entries().len(), LIMIT);
        assert_eq!(h.entries()[0], "echo 1099");
        assert_eq!(h.entries()[999], "echo 100");
    }
}

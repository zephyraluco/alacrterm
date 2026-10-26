//! Observe shell lifecycle markers without changing PTY bytes or Alacritty parsing.
use crate::{TerminalBackendEvent, alacritty::AlacrittyPty, prompt::PromptScanner};
use alacritty_terminal::{
    event::{OnResize, WindowSize},
    tty::{ChildEvent, EventedPty, EventedReadWrite},
};
use futures::channel::mpsc::UnboundedSender;
use polling::{Event, PollMode, Poller};
use std::{
    io::{self, Read, Write},
    sync::Arc,
};

pub(crate) struct ObservedPty {
    pty: AlacrittyPty,
    scanner: PromptScanner,
    events: UnboundedSender<TerminalBackendEvent>,
    _resources: Option<crate::SessionResources>,
    decoder: Option<crate::OscDecoder>,
}
impl ObservedPty {
    pub(crate) fn new(
        pty: AlacrittyPty,
        events: UnboundedSender<TerminalBackendEvent>,
        resources: Option<crate::SessionResources>,
        decoder: Option<crate::OscDecoder>,
    ) -> Self {
        Self {
            pty,
            events,
            scanner: Default::default(),
            _resources: resources,
            decoder,
        }
    }
}
impl Read for ObservedPty {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        let count = self.pty.reader().read(bytes)?;
        for event in self.scanner.feed(&bytes[..count], self.decoder) {
            let _ = self
                .events
                .unbounded_send(TerminalBackendEvent::Prompt(event));
        }
        for command in self.scanner.commands() {
            let _ = self
                .events
                .unbounded_send(TerminalBackendEvent::ShellCommand(command));
        }
        for (key, value) in self.scanner.context() {
            let _ = self
                .events
                .unbounded_send(TerminalBackendEvent::ShellContext(key, value));
        }
        Ok(count)
    }
}
impl Write for ObservedPty {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.pty.writer().write(bytes)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.pty.writer().flush()
    }
}
impl EventedReadWrite for ObservedPty {
    type Reader = Self;
    type Writer = Self;
    unsafe fn register(
        &mut self,
        poller: &Arc<Poller>,
        event: Event,
        mode: PollMode,
    ) -> io::Result<()> {
        // SAFETY: this wrapper owns the PTY for the entire registration lifetime.
        unsafe { self.pty.register(poller, event, mode) }
    }
    fn reregister(&mut self, poller: &Arc<Poller>, event: Event, mode: PollMode) -> io::Result<()> {
        self.pty.reregister(poller, event, mode)
    }
    fn deregister(&mut self, poller: &Arc<Poller>) -> io::Result<()> {
        self.pty.deregister(poller)
    }
    fn reader(&mut self) -> &mut Self {
        self
    }
    fn writer(&mut self) -> &mut Self {
        self
    }
}
impl EventedPty for ObservedPty {
    fn next_child_event(&mut self) -> Option<ChildEvent> {
        self.pty.next_child_event()
    }
}
impl OnResize for ObservedPty {
    fn on_resize(&mut self, size: WindowSize) {
        self.pty.on_resize(size);
    }
}

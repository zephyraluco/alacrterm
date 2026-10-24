//! Shared receive side lets SSH drain protocol responses immediately after parsing,
//! while the retained event task handles asynchronous/local PTY events.
use crate::TerminalBackendEvent;
use futures::{Stream, channel::mpsc::UnboundedReceiver};
use std::{
    pin::Pin,
    sync::{Arc, Mutex},
    task::{Context, Poll},
};

#[derive(Clone)]
pub(crate) struct ParserEvents(Arc<Mutex<UnboundedReceiver<TerminalBackendEvent>>>);
impl ParserEvents {
    pub(crate) fn new(receiver: UnboundedReceiver<TerminalBackendEvent>) -> Self {
        Self(Arc::new(Mutex::new(receiver)))
    }
    pub(crate) fn try_next(&self) -> Option<TerminalBackendEvent> {
        self.0
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .try_recv()
            .ok()
    }
}
impl Stream for ParserEvents {
    type Item = TerminalBackendEvent;
    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Pin::new(&mut *self.0.lock().unwrap_or_else(|error| error.into_inner())).poll_next(cx)
    }
}

impl futures::stream::FusedStream for ParserEvents {
    fn is_terminated(&self) -> bool {
        self.0
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .is_terminated()
    }
}

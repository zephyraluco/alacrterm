//! Application-provided local X11 destination and connection lifetime.
use anyhow::Result;
use futures::future::BoxFuture;
use std::sync::Arc;
use tokio::io::{AsyncRead, AsyncWrite};

pub trait X11Stream: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> X11Stream for T {}

pub trait X11Lease: Send + Sync {
    /// Called only after the forwarded client has authenticated.
    fn connect(&self) -> BoxFuture<'_, Result<Box<dyn X11Stream>>>;
    /// End forwarding when the destination is no longer available.
    fn closed(&self) -> BoxFuture<'_, ()> {
        Box::pin(std::future::pending())
    }
}

pub trait X11Target: Send + Sync {
    /// Reserve capacity without opening the local display socket.
    fn acquire(&self) -> Result<Box<dyn X11Lease>>;
}

#[derive(Clone, Default)]
pub struct SessionOptions {
    pub setup: Option<Arc<dyn crate::SessionSetup>>,
    pub x11: Option<Arc<dyn X11Target>>,
}

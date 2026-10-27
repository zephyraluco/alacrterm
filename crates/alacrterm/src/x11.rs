//! WayX selection, concurrency and service-generation policy belong to the app.
use anyhow::Result;
use futures::future::BoxFuture;
use ssh::{X11Lease, X11Stream, X11Target};

pub(crate) struct WayxTarget;
struct WayxLease {
    endpoint: platform::wayx::Endpoint,
    _permit: tokio::sync::OwnedSemaphorePermit,
}
impl X11Target for WayxTarget {
    fn acquire(&self) -> Result<Box<dyn X11Lease>> {
        let endpoint = platform::wayx::Endpoint::current()?;
        let permit = endpoint.acquire()?;
        Ok(Box::new(WayxLease {
            endpoint,
            _permit: permit,
        }))
    }
}
impl X11Lease for WayxLease {
    fn connect(&self) -> BoxFuture<'_, Result<Box<dyn X11Stream>>> {
        Box::pin(async move { Ok(Box::new(self.endpoint.local().await?) as Box<dyn X11Stream>) })
    }
    fn closed(&self) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            while self.endpoint.is_current() {
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
        })
    }
}

#[cfg(test)]
mod tests;

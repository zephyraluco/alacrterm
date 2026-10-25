//! Native platform initialization and shared display service.
#[cfg(windows)]
mod conpty;
pub mod wayx;
/// Initialize the native pseudoconsole backend before creating terminals.
pub fn initialize() {
    #[cfg(windows)]
    conpty::ensure();
}
pub use wayx::{WayxStatus, shutdown_wayx, start_wayx, stop_wayx, wayx_status};

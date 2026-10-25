//! Native platform initialization and shared display service.
pub mod wayx;
#[cfg(windows)]
mod conpty;
/// Initialize the native pseudoconsole backend before creating terminals.
pub fn initialize() {
    #[cfg(windows)]
    conpty::ensure();
}
pub use wayx::{WayxStatus, start_wayx, stop_wayx, wayx_status, shutdown_wayx};

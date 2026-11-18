//! Model configuration and catalog resolution, separate from transport and credentials.
mod profile;
pub use profile::{CredentialSource, ModelProfile, Protocol};

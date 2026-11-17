//! Native SDK model connections; no application SSE parser or second model loop.
pub use solaris_backend::{
    Backend, BackendError, Endpoint, ModelClient, ModelConnector, ModelParameters, TurnPlan, Wire,
};

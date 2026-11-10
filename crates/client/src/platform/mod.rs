//! The one thing that differs per target: how the iroh endpoint is built.

#[cfg(not(target_family = "wasm"))]
mod native;
#[cfg(target_family = "wasm")]
mod web;

#[cfg(not(target_family = "wasm"))]
pub use native::bind_endpoint;
#[cfg(target_family = "wasm")]
pub use web::bind_endpoint;

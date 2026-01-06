#![doc = include_str!("../README.md")]

pub mod context;
mod export;
#[cfg(feature = "http")]
pub mod http;
mod layer;
mod log;
mod span;

pub use export::ApiVersion;
pub use layer::{BuilderError, DatadogTraceLayer, DatadogTraceLayerBuilder};

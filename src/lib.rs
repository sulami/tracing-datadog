#![doc = include_str!("../README.md")]

pub mod context;
mod export;
#[cfg(any(feature = "http", docsrs))]
pub mod http;
mod layer;
mod log;
mod span;

pub use layer::{BuilderError, DatadogTraceLayer, DatadogTraceLayerBuilder};

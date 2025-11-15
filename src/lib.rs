#![doc = include_str!("../README.md")]

mod export;
#[cfg(feature = "http")]
pub mod http;
mod layer;
mod log;
mod span;

pub use layer::{BuilderError, DatadogTraceLayer, DatadogTraceLayerBuilder};

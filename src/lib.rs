use tracing_subscriber::Layer;

/// A [`Layer`] that sends traces to DataDog.
///
/// ```
/// # use tracing_subscriber::prelude::*;
/// # use tracing_datadog::DataDogTraceLayer;
/// tracing_subscriber::registry()
///   .with(DataDogTraceLayer::new())
///   .init();
/// ```
pub struct DataDogTraceLayer;

impl DataDogTraceLayer {
    pub fn new() -> Self {
        Self
    }
}

impl<S: tracing::Subscriber> Layer<S> for DataDogTraceLayer {}

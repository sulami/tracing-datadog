use std::{
    sync::Arc,
    sync::Mutex,
    thread::{JoinHandle, sleep, spawn},
    time::Duration,
};
use tracing::Id;
use tracing_subscriber::{Layer, layer::Context, registry::LookupSpan};

/// A [`Layer`] that sends traces to DataDog.
///
/// ```
/// # use tracing_subscriber::prelude::*;
/// # use tracing_datadog::DataDogTraceLayer;
/// tracing_subscriber::registry()
///   .with(DataDogTraceLayer::new())
///   .init();
/// ```
pub struct DataDogTraceLayer {
    buffer: Arc<Mutex<Vec<DataDogSpan>>>,
    exporter_thread: Option<JoinHandle<()>>,
}

impl DataDogTraceLayer {
    pub fn new() -> Self {
        let buffer = Arc::new(Mutex::new(Vec::new()));
        Self {
            buffer: buffer.clone(),
            exporter_thread: Some(spawn(move || {
                loop {
                    sleep(Duration::from_secs(10));
                    println!("Exporting traces to DataDog");
                    let spans = buffer.lock().unwrap().drain(..).collect::<Vec<_>>();
                    spans.into_iter().for_each(|span| {
                        println!("Exporting span: {}", span.id);
                    });
                }
            })),
        }
    }
}

impl Drop for DataDogTraceLayer {
    fn drop(&mut self) {
        self.exporter_thread.take().and_then(|t| t.join().ok());
    }
}

impl<S: tracing::Subscriber + for<'a> LookupSpan<'a>> Layer<S> for DataDogTraceLayer {
    fn on_close(&self, id: Id, ctx: Context<'_, S>) {
        let span = ctx.span(&id).expect("Span not found, this is a bug");
        self.buffer.lock().unwrap().push(DataDogSpan {
            id: span.id().into_u64(),
        });
    }
}

struct DataDogSpan {
    id: u64,
}

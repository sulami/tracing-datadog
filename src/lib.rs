use rmp_serde::Serializer;
use serde::Serialize;
use std::{
    collections::HashMap,
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
///   .with(DataDogTraceLayer::new("localhost:8126"))
///   .init();
/// ```
pub struct DataDogTraceLayer {
    buffer: Arc<Mutex<Vec<DataDogSpan>>>,
    exporter_thread: Option<JoinHandle<()>>,
}

impl DataDogTraceLayer {
    pub fn new(agent_address: impl Into<String>) -> Self {
        let buffer = Arc::new(Mutex::new(Vec::new()));
        let url = format!("http://{}/v0.4/traces", agent_address.into());
        Self {
            buffer: buffer.clone(),
            exporter_thread: Some(spawn(move || {
                let client = reqwest::blocking::Client::new();
                loop {
                    sleep(Duration::from_secs(10));

                    let spans = buffer.lock().unwrap().drain(..).collect::<Vec<_>>();
                    println!("Exporting {} spans", spans.len());

                    let mut body = vec![0b10010001];
                    let _ = spans
                        .serialize(&mut Serializer::new(&mut body).with_struct_map())
                        .inspect_err(|error| println!("Error serializing spans: {error:?}"));

                    let _ = client
                        .post(&url)
                        .header("Datadog-Meta-Tracer-Version", "v1.27.0")
                        .header("Content-Type", "application/msgpack")
                        .body(body)
                        .send()
                        .inspect_err(|error| println!("Error exporting spans: {error:?}"));
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
            span_id: span.id().into_u64(),
            ..Default::default()
        });
    }
}

#[derive(Default, Serialize)]
struct DataDogSpan {
    name: String,
    service: String,
    r#type: String,
    resource: String,
    start: i64,
    duration: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    meta: Option<HashMap<String, String>>,
    error_code: i32,
    span_id: u64,
    trace_id: u64,
    parent_id: u64,
}

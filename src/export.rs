use crate::span::Span;
use reqwest::header::{self, HeaderMap, HeaderName, HeaderValue};
use rmp_serde::Serializer as MpSerializer;
use serde::Serialize;
use std::{
    collections::HashMap,
    ops::DerefMut,
    sync::{Arc, Mutex, mpsc},
    thread::sleep,
    time::Duration,
};

const DATADOG_LANGUAGE_HEADER: HeaderName = HeaderName::from_static("datadog-meta-lang");
const DATADOG_TRACER_VERSION_HEADER: HeaderName =
    HeaderName::from_static("datadog-meta-tracer-version");
const DATADOG_TRACE_COUNT_HEADER: HeaderName = HeaderName::from_static("x-datadog-trace-count");
const DATADOG_CONTAINER_ID_HEADER: HeaderName = HeaderName::from_static("datadog-container-id");

pub(crate) fn exporter(
    agent_address: String,
    buffer: Arc<Mutex<Vec<Span>>>,
    container_id: Option<HeaderValue>,
    shutdown_signal: mpsc::Receiver<()>,
) -> impl FnOnce() {
    move || {
        let url = format!("http://{}/v0.4/traces", agent_address);
        let client = {
            let mut default_headers = HeaderMap::new();

            if let Some(container_id) = container_id {
                default_headers.insert(DATADOG_CONTAINER_ID_HEADER, container_id);
            };

            reqwest::blocking::Client::builder()
                .default_headers(default_headers)
                .retry(reqwest::retry::for_host(agent_address).max_retries_per_request(2))
                .build()
                .expect("Failed to build reqwest client")
        };
        let mut spans = Vec::new();

        loop {
            if matches!(
                shutdown_signal.try_recv(),
                Ok(()) | Err(mpsc::TryRecvError::Disconnected)
            ) {
                break;
            }

            sleep(Duration::from_secs(1));

            std::mem::swap(&mut spans, buffer.lock().unwrap().deref_mut());

            if spans.is_empty() {
                continue;
            }

            let mut body = vec![];

            let trace_chunks = group_traces(spans.drain(..)).collect::<Vec<_>>();
            let _ = trace_chunks
                .serialize(&mut MpSerializer::new(&mut body).with_struct_map())
                .inspect_err(|error| println!("Error serializing spans: {error:?}"));

            let _ = client
                .post(&url)
                .header(DATADOG_TRACER_VERSION_HEADER, env!("CARGO_PKG_VERSION"))
                .header(DATADOG_LANGUAGE_HEADER, "rust")
                .header(DATADOG_TRACE_COUNT_HEADER, trace_chunks.len())
                .header(header::CONTENT_TYPE, "application/msgpack")
                .body(body)
                .send()
                .inspect_err(|error| println!("Error exporting spans: {error:?}"));
        }
    }
}

/// Groups spans into trace chunks.
fn group_traces(spans: impl Iterator<Item = Span>) -> impl Iterator<Item = Vec<Span>> {
    let mut traces = HashMap::new();
    spans.for_each(|span| {
        traces
            .entry(span.trace_id)
            .or_insert_with(Vec::new)
            .push(span);
    });
    traces.into_values()
}

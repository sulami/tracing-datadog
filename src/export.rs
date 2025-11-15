use crate::span::DatadogSpan;
use reqwest::header::{self, HeaderMap, HeaderName, HeaderValue};
use rmp_serde::Serializer as MpSerializer;
use serde::Serialize;
use std::{
    collections::HashSet,
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
    buffer: Arc<Mutex<Vec<DatadogSpan>>>,
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
            if shutdown_signal.try_recv().is_ok() {
                break;
            }

            sleep(Duration::from_secs(1));

            std::mem::swap(&mut spans, buffer.lock().unwrap().deref_mut());

            if spans.is_empty() {
                continue;
            }

            let mut body = vec![0b10010001];
            let _ = spans
                .serialize(&mut MpSerializer::new(&mut body).with_struct_map())
                .inspect_err(|error| println!("Error serializing spans: {error:?}"));

            let _ = client
                .post(&url)
                .header(DATADOG_TRACER_VERSION_HEADER, env!("CARGO_PKG_VERSION"))
                .header(DATADOG_LANGUAGE_HEADER, "rust")
                .header(DATADOG_TRACE_COUNT_HEADER, trace_count(&spans))
                .header(header::CONTENT_TYPE, "application/msgpack")
                .body(body)
                .send()
                .inspect_err(|error| println!("Error exporting spans: {error:?}"));

            spans.clear();
        }
    }
}

/// Returns the number of unique trace IDs in a list of spans.
fn trace_count(spans: &[DatadogSpan]) -> usize {
    spans
        .iter()
        .map(|span| span.trace_id)
        .collect::<HashSet<_>>()
        .len()
}

mod span;

use crate::span::Span as InternalSpan;
#[cfg(feature = "ahash")]
use ahash::AHashMap as HashMap;
use reqwest::header::{self, HeaderMap, HeaderName, HeaderValue};
use rmp_serde::Serializer as MpSerializer;
use serde::Serialize;
use span::Span as ExportSpan;
#[cfg(not(feature = "ahash"))]
use std::collections::HashMap;
use std::{
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

/// The different versions of the Datadog trace API.
///
/// This maps to <https://github.com/DataDog/datadog-agent/blob/main/pkg/trace/api/version.go>.
#[derive(Copy, Clone)]
#[non_exhaustive]
pub enum ApiVersion {
    /// v0.4 sends all trace chunks as-is, as a big array.
    ///
    /// This is the default.
    V04,
}

impl ApiVersion {
    /// Returns the URL path for the given API version.
    fn url_path(&self) -> &'static str {
        match self {
            Self::V04 => "/v0.4/traces",
        }
    }

    /// Returns a function that produces a payload for the given API version.
    fn serializer(&self) -> SerializerFn {
        match self {
            Self::V04 => v04_trace_api_payload,
        }
    }
}

pub(crate) fn exporter(
    agent_address: String,
    api_version: ApiVersion,
    buffer: Arc<Mutex<Vec<InternalSpan>>>,
    container_id: Option<HeaderValue>,
    pool_idle_timeout: Option<Duration>,
    shutdown_signal: mpsc::Receiver<()>,
) -> impl FnOnce() {
    move || {
        let url = format!("http://{}{}", agent_address, api_version.url_path());
        let client = {
            let mut default_headers = HeaderMap::new();

            if let Some(container_id) = container_id {
                default_headers.insert(DATADOG_CONTAINER_ID_HEADER, container_id);
            };

            let mut builder = reqwest::blocking::Client::builder()
                .default_headers(default_headers)
                .retry(reqwest::retry::for_host(agent_address).max_retries_per_request(2));

            if let Some(timeout) = pool_idle_timeout {
                builder = builder.pool_idle_timeout(timeout);
            }

            builder.build().expect("Failed to build reqwest client")
        };
        let mut spans = Vec::new();

        while let Err(mpsc::TryRecvError::Empty) = shutdown_signal.try_recv() {
            sleep(Duration::from_secs(1));

            std::mem::swap(&mut spans, buffer.lock().unwrap().deref_mut());

            if spans.is_empty() {
                continue;
            }

            let (body, trace_count) = api_version.serializer()(&mut spans);

            let _ = client
                .post(&url)
                .header(DATADOG_TRACER_VERSION_HEADER, env!("CARGO_PKG_VERSION"))
                .header(DATADOG_LANGUAGE_HEADER, "rust")
                .header(DATADOG_TRACE_COUNT_HEADER, trace_count)
                .header(header::CONTENT_TYPE, "application/msgpack")
                .body(body)
                .send()
                .inspect_err(|error| tracing::error!(?error, "Error exporting spans"));
        }
    }
}

/// Groups spans into trace chunks.
fn group_traces(
    spans: impl Iterator<Item = InternalSpan>,
) -> impl Iterator<Item = Vec<ExportSpan>> {
    let mut traces = HashMap::new();
    spans.for_each(|span| {
        traces
            .entry(span.trace_id)
            .or_insert_with(Vec::new)
            .push(ExportSpan::from(span));
    });
    traces.into_values()
}

/// The type of a function that produces a payload for a given API version.
type SerializerFn = fn(&mut Vec<InternalSpan>) -> (Vec<u8>, usize);

/// Produces the payload for the v0.4 Datadog trace API.
///
/// Also returns the number of traces serialized.
fn v04_trace_api_payload(spans: &mut Vec<InternalSpan>) -> (Vec<u8>, usize) {
    let mut payload = vec![];

    let trace_chunks = group_traces(spans.drain(..)).collect::<Vec<_>>();

    let _ = trace_chunks
        .serialize(&mut MpSerializer::new(&mut payload).with_struct_map())
        .inspect_err(|error| tracing::error!(?error, "Error serializing spans"));

    (payload, trace_chunks.len())
}

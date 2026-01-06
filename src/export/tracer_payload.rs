//! Tracer payload format descriptions for the Datadog agent.
//!
//! These map to the formats described in
//! https://github.com/DataDog/datadog-agent/blob/main/pkg/proto/datadog/trace/tracer_payload.proto,
//! and the descriptions are lifted directly from there, with some grammatical adjustments.

use super::span::Span as ExportSpan;
use crate::span::Span as InternalSpan;
#[cfg(feature = "ahash")]
use ahash::AHashMap as HashMap;
use serde::Serialize;
use std::borrow::Cow;
#[cfg(not(feature = "ahash"))]
use std::collections::HashMap;

/// Represents a payload the trace agent receives from tracers.
#[derive(Serialize)]
pub(crate) struct TracerPayload {
    /// The ID of the container where the tracer is running on.
    container_id: Cow<'static, str>,
    /// The language of the tracer.
    language_name: Cow<'static, str>,
    /// The language version of the tracer.
    language_version: Cow<'static, str>,
    /// The version of the tracer.
    tracer_version: Cow<'static, str>,
    /// The V4 UUID representation of a tracer session.
    runtime_id: Cow<'static, str>,
    /// A list of contained trace chunks.
    chunks: Vec<TraceChunk>,
    /// Tags common in all `chunks`.
    tags: HashMap<Cow<'static, str>, String>,
    /// The `env` tag that is set in the tracer.
    env: Cow<'static, str>,
    /// The hostname of where the tracer is running.
    hostname: Cow<'static, str>,
    /// The `version` tag that set in the tracer.
    app_version: Cow<'static, str>,
}

impl TracerPayload {
    pub(crate) fn trace_count(&self) -> usize {
        self.chunks.len()
    }
}

impl FromIterator<InternalSpan> for TracerPayload {
    fn from_iter<T: IntoIterator<Item = InternalSpan>>(iter: T) -> Self {
        let chunks = group_traces(iter.into_iter());

        TracerPayload {
            container_id: Default::default(),
            language_name: "rust".into(),
            language_version: Default::default(),
            tracer_version: env!("CARGO_PKG_VERSION").into(),
            runtime_id: Cow::Borrowed(""),
            chunks: chunks.map(TraceChunk::from_iter).collect(),
            // XXX: We could be deduplicating some tags here, though that becomes less relevant
            //      in the v1.0 trace API, which deduplicates tags globally.
            tags: HashMap::new(),
            env: Cow::Borrowed(""),
            hostname: Cow::Borrowed(""),
            app_version: Cow::Borrowed(""),
        }
    }
}

/// Represents a list of spans with the same trace ID.
#[derive(Serialize)]
struct TraceChunk {
    /// Sampling priority of the trace.
    priority: i32,
    /// Origin product ("lambda", "rum", etc.) of the trace.
    origin: Cow<'static, str>,
    /// List of contained spans.
    spans: Vec<ExportSpan>,
    /// Tags common in all spans.
    tags: HashMap<Cow<'static, str>, String>,
    /// Whether the trace was dropped by samplers.
    dropped_trace: bool,
}

impl FromIterator<InternalSpan> for TraceChunk {
    fn from_iter<T: IntoIterator<Item = InternalSpan>>(iter: T) -> Self {
        TraceChunk {
            priority: 1,
            // XXX: It's not clear what the expected value here is. I've found examples of
            //      "lambda", "rum", and "appsec", as well as testing values.
            origin: Cow::Borrowed("apm"),
            spans: iter.into_iter().map(Into::into).collect(),
            // XXX: We could be deduplicating some tags here, though that becomes less relevant
            //      in the v1.0 trace API, which deduplicates tags globally.
            tags: HashMap::new(),
            dropped_trace: false,
        }
    }
}

/// Groups spans into trace chunks.
fn group_traces(
    spans: impl Iterator<Item = InternalSpan>,
) -> impl Iterator<Item = Vec<InternalSpan>> {
    let mut traces = HashMap::new();
    spans.for_each(|span| {
        traces
            .entry(span.trace_id)
            .or_insert_with(Vec::new)
            .push(span);
    });
    traces.into_values()
}

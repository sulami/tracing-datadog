//! Spans and related types for exporting to the Datadog agent.
//!
//! These types are similar to the ones in [`crate::span`], but are constructed in a shape ready for
//! serialization. There is some conversion that needs to happen, namely the splitting of the trace
//! ID into high and low bits, and having two distinct sets of types ensures that we cannot forget
//! this step.
//!
//! This mostly matches up with
//! <https://github.com/DataDog/datadog-agent/blob/main/pkg/proto/datadog/trace/span.proto>.

#[cfg(feature = "ahash")]
use ahash::AHashMap as HashMap;
use serde::Serialize;
#[cfg(not(feature = "ahash"))]
use std::collections::HashMap;
use std::{borrow::Cow, fmt::Debug};

/// A span. This span has a 64-bit trace ID for exporting.
#[derive(Debug, Serialize)]
pub(crate) struct Span {
    trace_id: u64,
    span_id: u64,
    parent_id: u64,
    start: i64,
    duration: i64,
    /// This is what maps to the operation in Datadog.
    name: String,
    service: String,
    r#type: String,
    resource: String,
    meta: HashMap<Cow<'static, str>, String>,
    metrics: HashMap<Cow<'static, str>, f64>,
    span_links: Vec<SpanLink>,
    error_code: i32,
}

impl From<crate::span::Span> for Span {
    fn from(span: crate::span::Span) -> Self {
        let crate::span::Span {
            trace_id,
            span_id,
            parent_id,
            start,
            duration,
            name,
            service,
            r#type,
            resource,
            mut meta,
            metrics,
            span_links,
            error_code,
        } = span;

        // Split the trace ID into low and high bits.
        let trace_id_low = trace_id as u64;
        let trace_id_high = (trace_id >> 64) as u64;
        let tid_key = Cow::from("_dd.p.tid");
        let tid_value = format!("{trace_id_high:016x}");
        meta.insert(tid_key, tid_value);

        Self {
            trace_id: trace_id_low,
            span_id,
            parent_id,
            start,
            duration,
            name,
            service,
            r#type,
            resource,
            meta,
            metrics,
            span_links: span_links.into_iter().map(Into::into).collect(),
            error_code,
        }
    }
}

#[derive(Debug, Serialize)]
struct SpanLink {
    /// The lower 64 bits of the trace ID.
    trace_id: u64,
    /// The upper 64 bits of the trace ID.
    #[serde(skip_serializing_if = "is_zero")]
    trace_id_high: u64,
    span_id: u64,
}

impl From<crate::span::SpanLink> for SpanLink {
    fn from(span_link: crate::span::SpanLink) -> Self {
        Self {
            trace_id: span_link.trace_id as u64,
            trace_id_high: (span_link.trace_id >> 64) as u64,
            span_id: span_link.span_id,
        }
    }
}

/// Returns true if the given `u64` is zero.
fn is_zero(u: &u64) -> bool {
    *u == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serialize_span_messagepack_snapshot() {
        let span: Span = crate::span::Span {
            trace_id: 123,
            span_id: 456,
            parent_id: 0,
            start: 0,
            duration: 0,
            name: "name".to_string(),
            service: "service".to_string(),
            r#type: "type".to_string(),
            resource: "resource".to_string(),
            meta: Default::default(),
            metrics: Default::default(),
            span_links: vec![crate::span::SpanLink {
                trace_id: 0xb6b63b816e1955e93d160b2295648b4a,
                span_id: 100,
            }],
            error_code: 0,
        }
        .into();

        let mut out = Vec::new();
        let mut serializer = rmp_serde::Serializer::new(&mut out).with_struct_map();
        vec![span].serialize(&mut serializer).unwrap();

        let expected: Vec<u8> = vec![
            145, 141, 168, 116, 114, 97, 99, 101, 95, 105, 100, 123, 167, 115, 112, 97, 110, 95,
            105, 100, 205, 1, 200, 169, 112, 97, 114, 101, 110, 116, 95, 105, 100, 0, 165, 115,
            116, 97, 114, 116, 0, 168, 100, 117, 114, 97, 116, 105, 111, 110, 0, 164, 110, 97, 109,
            101, 164, 110, 97, 109, 101, 167, 115, 101, 114, 118, 105, 99, 101, 167, 115, 101, 114,
            118, 105, 99, 101, 164, 116, 121, 112, 101, 164, 116, 121, 112, 101, 168, 114, 101,
            115, 111, 117, 114, 99, 101, 168, 114, 101, 115, 111, 117, 114, 99, 101, 164, 109, 101,
            116, 97, 129, 169, 95, 100, 100, 46, 112, 46, 116, 105, 100, 176, 48, 48, 48, 48, 48,
            48, 48, 48, 48, 48, 48, 48, 48, 48, 48, 48, 167, 109, 101, 116, 114, 105, 99, 115, 128,
            170, 115, 112, 97, 110, 95, 108, 105, 110, 107, 115, 145, 131, 168, 116, 114, 97, 99,
            101, 95, 105, 100, 207, 61, 22, 11, 34, 149, 100, 139, 74, 173, 116, 114, 97, 99, 101,
            95, 105, 100, 95, 104, 105, 103, 104, 207, 182, 182, 59, 129, 110, 25, 85, 233, 167,
            115, 112, 97, 110, 95, 105, 100, 100, 170, 101, 114, 114, 111, 114, 95, 99, 111, 100,
            101, 0,
        ];

        assert_eq!(out, expected);
    }

    #[test]
    fn serialize_span_json_snapshot() {
        // This test is just here because it's more reasonable to compare differences in JSON
        // by human eyeball than in MessagePack.
        let span: Span = crate::span::Span {
            trace_id: 0x4e347cc0b27982c400e12912865cc52f,
            span_id: 456,
            parent_id: 0,
            start: 0,
            duration: 0,
            name: "name".to_string(),
            service: "service".to_string(),
            r#type: "type".to_string(),
            resource: "resource".to_string(),
            meta: Default::default(),
            metrics: Default::default(),
            span_links: vec![crate::span::SpanLink {
                trace_id: 0xb6b63b816e1955e93d160b2295648b4a,
                span_id: 100,
            }],
            error_code: 0,
        }
        .into();

        let out = serde_json::to_string_pretty(&span).unwrap();
        let expected = r#"{
  "trace_id": 63377029300274479,
  "span_id": 456,
  "parent_id": 0,
  "start": 0,
  "duration": 0,
  "name": "name",
  "service": "service",
  "type": "type",
  "resource": "resource",
  "meta": {
    "_dd.p.tid": "4e347cc0b27982c4"
  },
  "metrics": {},
  "span_links": [
    {
      "trace_id": 4401717928964426570,
      "trace_id_high": 13165775987748197865,
      "span_id": 100
    }
  ],
  "error_code": 0
}"#;

        assert_eq!(out, expected);
    }
}

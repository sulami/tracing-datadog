#[cfg(feature = "ahash")]
use ahash::AHashMap as HashMap;
use serde::Serialize;
#[cfg(not(feature = "ahash"))]
use std::collections::HashMap;
use std::{borrow::Cow, fmt::Debug};
use tracing_core::{Field, field::Visit};

/// The v0.4 Datadog trace API format for spans.
#[derive(Default, Debug, Serialize)]
pub(crate) struct Span {
    pub trace_id: u64,
    pub span_id: u64,
    pub parent_id: u64,
    pub start: i64,
    pub duration: i64,
    /// This is what maps to the operation in Datadog.
    pub name: String,
    pub service: String,
    pub r#type: String,
    pub resource: String,
    #[serde(borrow)]
    pub meta: HashMap<Cow<'static, str>, String>,
    #[serde(borrow)]
    pub metrics: HashMap<&'static str, f64>,
    pub span_links: Vec<SpanLink>,
    pub error_code: i32,
}

#[derive(Debug, Serialize)]
pub(crate) struct SpanLink {
    pub trace_id: u64,
    pub span_id: u64,
}

/// A visitor that converts tracing span attributes to a [`Span`].
pub(crate) struct SpanAttributeVisitor<'a> {
    dd_span: &'a mut Span,
}

impl<'a> SpanAttributeVisitor<'a> {
    pub fn new(dd_span: &'a mut Span) -> Self {
        Self { dd_span }
    }
}

impl<'a> Visit for SpanAttributeVisitor<'a> {
    fn record_str(&mut self, field: &Field, value: &str) {
        // Strings are broken out because their debug representation includes quotation marks.
        match field.name() {
            "service" => self.dd_span.service = value.to_string(),
            "span.type" => self.dd_span.r#type = value.to_string(),
            "operation" => self.dd_span.name = value.to_string(),
            "resource" => self.dd_span.resource = value.to_string(),
            name => {
                self.dd_span.meta.insert(name.into(), value.to_string());
            }
        };
    }

    fn record_debug(&mut self, field: &Field, value: &dyn Debug) {
        match field.name() {
            "service" => self.dd_span.service = format!("{value:?}"),
            "span.type" => self.dd_span.r#type = format!("{value:?}"),
            "operation" => self.dd_span.name = format!("{value:?}"),
            "resource" => self.dd_span.resource = format!("{value:?}"),
            name => {
                self.dd_span.meta.insert(name.into(), format!("{value:?}"));
            }
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serialize_span_messagepack_snapshot() {
        let span = Span {
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
            span_links: vec![SpanLink {
                trace_id: 789,
                span_id: 100,
            }],
            error_code: 0,
        };

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
            116, 97, 128, 167, 109, 101, 116, 114, 105, 99, 115, 128, 170, 115, 112, 97, 110, 95,
            108, 105, 110, 107, 115, 145, 130, 168, 116, 114, 97, 99, 101, 95, 105, 100, 205, 3,
            21, 167, 115, 112, 97, 110, 95, 105, 100, 100, 170, 101, 114, 114, 111, 114, 95, 99,
            111, 100, 101, 0,
        ];

        assert_eq!(out, expected);
    }

    #[test]
    fn serialize_span_json_snapshot() {
        let span = Span {
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
            span_links: vec![SpanLink {
                trace_id: 789,
                span_id: 100,
            }],
            error_code: 0,
        };

        let out = serde_json::to_string_pretty(&span).unwrap();
        let expected = r#"{
  "trace_id": 123,
  "span_id": 456,
  "parent_id": 0,
  "start": 0,
  "duration": 0,
  "name": "name",
  "service": "service",
  "type": "type",
  "resource": "resource",
  "meta": {},
  "metrics": {},
  "span_links": [
    {
      "trace_id": 789,
      "span_id": 100
    }
  ],
  "error_code": 0
}"#;

        assert_eq!(out, expected);
    }
}

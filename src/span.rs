#[cfg(feature = "ahash")]
use ahash::AHashMap as HashMap;
use serde::{Serialize, Serializer, ser::SerializeStruct};
#[cfg(not(feature = "ahash"))]
use std::collections::HashMap;
use std::{borrow::Cow, fmt::Debug};
use tracing_core::{Field, field::Visit};

/// A span, which incidentally matches the v0.4 trace API format.
#[derive(Default, Debug)]
pub(crate) struct Span {
    pub trace_id: u128,
    pub span_id: u64,
    pub parent_id: u64,
    pub start: i64,
    pub duration: i64,
    /// This is what maps to the operation in Datadog.
    pub name: String,
    pub service: String,
    pub r#type: String,
    pub resource: String,
    pub meta: HashMap<Cow<'static, str>, String>,
    pub metrics: HashMap<Cow<'static, str>, f64>,
    pub span_links: Vec<SpanLink>,
    pub error_code: i32,
}

impl Serialize for Span {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        // Custom impl because we need to split the trace ID across two fields.
        let trace_id_low = self.trace_id as u64;
        let trace_id_high = (self.trace_id >> 64) as u64;

        let tid_key = Cow::from("_dd.p.tid");
        let tid_value = format!("{trace_id_high:016x}");

        let meta = IterMapSerializer {
            iter: self
                .meta
                .iter()
                .chain(std::iter::once((&tid_key, &tid_value))),
        };

        let mut s = serializer.serialize_struct("span", 13)?;
        s.serialize_field("trace_id", &trace_id_low)?;
        s.serialize_field("span_id", &self.span_id)?;
        s.serialize_field("parent_id", &self.parent_id)?;
        s.serialize_field("start", &self.start)?;
        s.serialize_field("duration", &self.duration)?;
        s.serialize_field("name", &self.name)?;
        s.serialize_field("service", &self.service)?;
        s.serialize_field("type", &self.r#type)?;
        s.serialize_field("resource", &self.resource)?;
        s.serialize_field("meta", &meta)?;
        s.serialize_field("metrics", &self.metrics)?;
        s.serialize_field("span_links", &self.span_links)?;
        s.serialize_field("error_code", &self.error_code)?;
        s.end()
    }
}

/// Helper type that serializes an iterator of 2-tuples to a map without cloning or hashing the
/// items.
struct IterMapSerializer<I> {
    iter: I,
}

impl<I, K, V> Serialize for IterMapSerializer<I>
where
    I: Iterator<Item = (K, V)> + Clone,
    K: Serialize,
    V: Serialize,
{
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.collect_map(self.iter.clone())
    }
}

#[derive(Debug)]
pub(crate) struct SpanLink {
    pub trace_id: u128,
    pub span_id: u64,
}

impl Serialize for SpanLink {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        // Custom impl because we need to split the trace ID across two fields.
        let mut s = serializer.serialize_struct("span_link", 3)?;
        s.serialize_field("trace_id", &(self.trace_id as u64))?;
        s.serialize_field("trace_id_high", &((self.trace_id >> 64) as u64))?;
        s.serialize_field("span_id", &self.span_id)?;
        s.end()
    }
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
                trace_id: 0xb6b63b816e1955e93d160b2295648b4a,
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
        let span = Span {
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
            span_links: vec![SpanLink {
                trace_id: 0xb6b63b816e1955e93d160b2295648b4a,
                span_id: 100,
            }],
            error_code: 0,
        };

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

//! Library-internal representations for spans.

#[cfg(feature = "ahash")]
use ahash::AHashMap as HashMap;
#[cfg(not(feature = "ahash"))]
use std::collections::HashMap;
use std::{borrow::Cow, fmt::Debug};
use tracing_core::{Field, field::Visit};

/// A span. This is the info we attach to tracing spans as extensions.
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

#[derive(Debug)]
pub(crate) struct SpanLink {
    pub trace_id: u128,
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

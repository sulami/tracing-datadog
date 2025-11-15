use jiff::Timestamp;
use serde::ser::SerializeMap;
use serde::{Serialize, Serializer};
use std::{borrow::Cow, collections::HashMap, fmt::Debug};
use tracing_core::{Field, Level, field::Visit};

/// The Datadog structure log format. This is what we write to JSON.
pub(crate) struct DatadogLog {
    pub timestamp: Timestamp,
    pub level: Level,
    pub message: String,
    pub trace_id: Option<u64>,
    pub span_id: Option<u64>,
    pub fields: HashMap<Cow<'static, str>, String>,
}

impl Serialize for DatadogLog {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("timestamp", &self.timestamp)?;
        map.serialize_entry("level", &self.level.as_str())?;
        map.serialize_entry("message", &self.message)?;
        if let Some(trace_id) = &self.trace_id {
            map.serialize_entry("dd.trace_id", &trace_id)?;
        }
        if let Some(span_id) = &self.span_id {
            map.serialize_entry("dd.span_id", &span_id)?;
        }
        for (key, value) in &self.fields {
            map.serialize_entry(&format!("fields.{key}"), value)?;
        }
        map.end()
    }
}

/// A visitor that collects tracing attributes into a map.
#[derive(Default)]
pub(crate) struct FieldVisitor {
    fields: HashMap<Cow<'static, str>, String>,
}

impl FieldVisitor {
    pub(crate) fn finish(self) -> HashMap<Cow<'static, str>, String> {
        self.fields
    }
}

impl Visit for FieldVisitor {
    fn record_str(&mut self, field: &Field, value: &str) {
        self.fields.insert(field.name().into(), value.to_string());
    }

    fn record_debug(&mut self, field: &Field, value: &dyn Debug) {
        self.fields
            .insert(field.name().into(), format!("{value:?}"));
    }
}

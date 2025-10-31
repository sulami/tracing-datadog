use rmp_serde::Serializer;
use serde::Serialize;
use std::{
    collections::HashMap,
    sync::Arc,
    sync::Mutex,
    thread::{JoinHandle, sleep, spawn},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tracing_core::{
    Field, Subscriber,
    field::Visit,
    span::{Attributes, Id, Record},
};
use tracing_subscriber::{Layer, layer::Context, registry::LookupSpan};

/// A [`Layer`] that sends traces to DataDog.
///
/// ```
/// # use tracing_subscriber::prelude::*;
/// # use tracing_datadog::DataDogTraceLayer;
/// tracing_subscriber::registry()
///   .with(DataDogTraceLayer::new("service", "env", "version", "localhost:8126"))
///   .init();
/// ```
pub struct DataDogTraceLayer {
    buffer: Arc<Mutex<Vec<DataDogSpan>>>,
    service: String,
    env: String,
    version: String,
    exporter_thread: Option<JoinHandle<()>>,
}

impl DataDogTraceLayer {
    pub fn new(
        service: impl Into<String>,
        env: impl Into<String>,
        version: impl Into<String>,
        agent_address: impl Into<String>,
    ) -> Self {
        let buffer = Arc::new(Mutex::new(Vec::new()));
        let url = format!("http://{}/v0.4/traces", agent_address.into());

        Self {
            buffer: buffer.clone(),
            service: service.into(),
            env: env.into(),
            version: version.into(),
            exporter_thread: Some(spawn(move || {
                let client = reqwest::blocking::Client::new();
                loop {
                    sleep(Duration::from_secs(5));

                    let spans = buffer.lock().unwrap().drain(..).collect::<Vec<_>>();
                    if spans.is_empty() {
                        continue;
                    }

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

impl<S: Subscriber + for<'a> LookupSpan<'a>> Layer<S> for DataDogTraceLayer {
    fn on_new_span(&self, attrs: &Attributes<'_>, id: &Id, ctx: Context<'_, S>) {
        let span = ctx.span(id).expect("Span not found, this is a bug");
        let mut extensions = span.extensions_mut();

        let trace_id = span
            .parent()
            .and_then(|parent| {
                parent
                    .extensions()
                    .get::<DataDogSpan>()
                    .map(|dd_span| dd_span.trace_id)
            })
            .unwrap_or(rand::random());

        let mut dd_span = DataDogSpan {
            name: span.name().to_string(),
            service: self.service.clone(),
            r#type: "internal".into(),
            span_id: span.id().into_u64(),
            start: epoch_ns(),
            parent_id: span
                .parent()
                .map(|parent| parent.id().into_u64())
                .unwrap_or_default(),
            trace_id,
            meta: HashMap::from_iter([
                ("env".into(), self.env.clone()),
                ("version".into(), self.version.clone()),
            ]),
            ..Default::default()
        };

        attrs.record(&mut SpanAttributeVisitor::new(&mut dd_span));

        extensions.insert(dd_span);
    }

    fn on_record(&self, id: &Id, values: &Record<'_>, ctx: Context<'_, S>) {
        let span = ctx.span(id).expect("Span not found, this is a bug");
        let mut extensions = span.extensions_mut();

        if let Some(dd_span) = extensions.get_mut::<DataDogSpan>() {
            values.record(&mut SpanAttributeVisitor::new(dd_span));
        }
    }

    fn on_follows_from(&self, id: &Id, follows: &Id, ctx: Context<'_, S>) {
        let span = ctx.span(id).expect("Span not found, this is a bug");
        let mut extensions = span.extensions_mut();

        if let Some(dd_span) = extensions.get_mut::<DataDogSpan>() {
            dd_span.parent_id = follows.into_u64();
        }
    }

    fn on_enter(&self, id: &Id, ctx: Context<'_, S>) {
        let span = ctx.span(id).expect("Span not found, this is a bug");
        let mut extensions = span.extensions_mut();

        let now = epoch_ns();

        match extensions.get_mut::<DataDogSpan>() {
            Some(dd_span) if dd_span.start != 0 => dd_span.start = now,
            None => return,
            _ => {}
        }

        match extensions.get_mut::<LastEnter>() {
            Some(last_enter) => last_enter.0 = now,
            None => extensions.insert(LastEnter(now)),
        }
    }

    fn on_exit(&self, id: &Id, ctx: Context<'_, S>) {
        let span = ctx.span(id).expect("Span not found, this is a bug");
        let mut extensions = span.extensions_mut();

        let now = epoch_ns();

        let last_enter = extensions
            .remove::<LastEnter>()
            .expect("LastEnter not found, this is a bug");

        if let Some(dd_span) = extensions.get_mut::<DataDogSpan>() {
            dd_span.duration += now - last_enter.0
        }
    }

    fn on_close(&self, id: Id, ctx: Context<'_, S>) {
        let span = ctx.span(&id).expect("Span not found, this is a bug");
        let mut extensions = span.extensions_mut();

        if let Some(dd_span) = extensions.remove::<DataDogSpan>() {
            self.buffer.lock().unwrap().push(dd_span);
        }
    }
}

fn epoch_ns() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("SystemTime is before UNIX epoch")
        .as_nanos() as i64
}

#[derive(Default, Debug, Serialize)]
struct DataDogSpan {
    name: String,
    service: String,
    r#type: String,
    resource: String,
    start: i64,
    duration: i64,
    meta: HashMap<String, String>,
    error_code: i32,
    span_id: u64,
    trace_id: u64,
    parent_id: u64,
}

struct LastEnter(i64);

struct SpanAttributeVisitor<'a> {
    dd_span: &'a mut DataDogSpan,
}

impl<'a> SpanAttributeVisitor<'a> {
    fn new(dd_span: &'a mut DataDogSpan) -> Self {
        Self { dd_span }
    }
}

impl<'a> Visit for SpanAttributeVisitor<'a> {
    fn record_str(&mut self, field: &Field, value: &str) {
        match field.name() {
            "service" => self.dd_span.service = value.to_string(),
            "span.kind" => self.dd_span.r#type = value.to_string(),
            "operation" => self.dd_span.name = value.to_string(),
            "resource" => self.dd_span.resource = value.to_string(),
            name => {
                self.dd_span
                    .meta
                    .insert(name.to_string(), value.to_string());
            }
        };
    }

    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        match field.name() {
            "service" => self.dd_span.service = format!("{value:?}"),
            "span.kind" => self.dd_span.r#type = format!("{value:?}"),
            "operation" => self.dd_span.name = format!("{value:?}"),
            "resource" => self.dd_span.resource = format!("{value:?}"),
            name => {
                self.dd_span
                    .meta
                    .insert(name.to_string(), format!("{value:?}"));
            }
        };
    }
}

use rmp_serde::Serializer;
use serde::Serialize;
use std::{
    collections::HashMap,
    marker::PhantomData,
    sync::Arc,
    sync::Mutex,
    thread::{JoinHandle, sleep, spawn},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tracing_core::{
    Dispatch, Field, Subscriber,
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
pub struct DataDogTraceLayer<S> {
    buffer: Arc<Mutex<Vec<DataDogSpan>>>,
    service: String,
    env: String,
    version: String,
    #[cfg(feature = "http")]
    #[cfg_attr(feature = "http", allow(unused))]
    get_context: http::WithContext,
    exporter_thread: Option<JoinHandle<()>>,
    _registry: PhantomData<S>,
}

impl<S> DataDogTraceLayer<S>
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
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
            #[cfg(feature = "http")]
            get_context: http::WithContext(Self::get_context),
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
            _registry: PhantomData,
        }
    }

    fn get_context(dispatch: &Dispatch, id: &Id, f: &mut dyn FnMut(&mut DataDogSpan)) {
        let subscriber = dispatch
            .downcast_ref::<S>()
            .expect("Subscriber did not downcast to expected type, this is a bug");
        let span = subscriber.span(id).expect("Span not found, this is a bug");

        let mut extensions = span.extensions_mut();
        if let Some(dd_span) = extensions.get_mut::<DataDogSpan>() {
            f(dd_span);
        }
    }
}

impl<S> Drop for DataDogTraceLayer<S> {
    fn drop(&mut self) {
        self.exporter_thread.take().and_then(|t| t.join().ok());
    }
}

impl<S> Layer<S> for DataDogTraceLayer<S>
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
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
            Some(dd_span) if dd_span.start == 0 => dd_span.start = now,
            _ => {}
        }
    }

    fn on_exit(&self, id: &Id, ctx: Context<'_, S>) {
        let span = ctx.span(id).expect("Span not found, this is a bug");
        let mut extensions = span.extensions_mut();

        let now = epoch_ns();

        if let Some(dd_span) = extensions.get_mut::<DataDogSpan>() {
            dd_span.duration = now - dd_span.start
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

#[cfg(feature = "http")]
pub mod http {
    use crate::DataDogSpan;
    use http::{HeaderMap, HeaderName};
    use tracing_core::{Dispatch, span::Id};

    /// The trace context for distributed tracing. This is a subset of the W3C trace context
    /// which allows stitching together traces with spans from different services.
    #[derive(Default)]
    pub struct DataDogContext {
        trace_id: u128,
        parent_id: u64,
    }

    impl DataDogContext {
        /// Parses a context for distributed tracing from W3C trace context headers.
        pub fn from_w3c_headers(headers: &HeaderMap) -> Self {
            Self::parse_w3c_headers(headers).unwrap_or_default()
        }

        fn parse_w3c_headers(headers: &HeaderMap) -> Option<Self> {
            let header = headers.get("traceparent")?.to_str().ok()?;

            let parts: Vec<&str> = header.split('-').collect();
            if parts.len() != 4 {
                return None;
            }

            let Some(0) = u8::from_str_radix(parts[0], 16).ok() else {
                return None;
            };

            let trace_id = u128::from_str_radix(parts[1], 16).ok()?;
            let parent_id = u64::from_str_radix(parts[2], 16).ok()?;

            Some(Self {
                trace_id,
                parent_id,
            })
        }

        /// Serializes a context for distributed tracing to W3C trace context headers.
        pub fn to_w3c_headers(&self) -> HeaderMap {
            let header = format!(
                "{version:02x}-{trace_id:032x}-{parent_id:016x}-{trace_flags:02x}",
                version = 0,
                trace_id = self.trace_id,
                parent_id = self.parent_id,
                trace_flags = 1,
            );

            HeaderMap::from_iter([(
                HeaderName::from_static("traceparent"),
                header.parse().unwrap(),
            )])
        }
    }

    // This function "remembers" the types of the subscriber so that we can downcast to something
    // aware of them without knowing those types at the call site. Adapted from tracing-error.
    pub(crate) struct WithContext(
        pub(crate) fn(&Dispatch, &Id, f: &mut dyn FnMut(&mut DataDogSpan)),
    );

    impl WithContext {
        pub(crate) fn with_context(
            &self,
            dispatch: &Dispatch,
            id: &Id,
            mut f: &mut dyn FnMut(&mut DataDogSpan),
        ) {
            (self.0)(dispatch, id, &mut f);
        }
    }

    pub trait DistributedTracingContext {
        /// Gets the context for distributed tracing from the current span.
        fn get_context(&self) -> DataDogContext;

        /// Sets the context for distributed tracing on the current span.
        fn set_context(&self, context: DataDogContext);
    }

    impl DistributedTracingContext for tracing::Span {
        fn get_context(&self) -> DataDogContext {
            let mut ctx = None;

            self.with_subscriber(|(id, subscriber)| {
                let Some(get_context) = subscriber.downcast_ref::<WithContext>() else {
                    return;
                };
                get_context.with_context(subscriber, id, &mut |dd_span| {
                    ctx = Some(DataDogContext {
                        // NB Trace IDs can be 128-bit nowadays, but the 0.4 API still uses 64-bit.
                        trace_id: dd_span.trace_id as u128,
                        parent_id: dd_span.parent_id,
                    })
                });
            });

            ctx.unwrap_or_default()
        }

        fn set_context(&self, context: DataDogContext) {
            self.with_subscriber(move |(id, subscriber)| {
                let Some(get_context) = subscriber.downcast_ref::<WithContext>() else {
                    return;
                };
                get_context.with_context(subscriber, id, &mut |dd_span| {
                    // NB Trace IDs can be 128-bit nowadays, but the 0.4 API still uses 64-bit.
                    dd_span.trace_id = context.trace_id as u64;
                    dd_span.parent_id = context.parent_id;
                })
            });
        }
    }
}

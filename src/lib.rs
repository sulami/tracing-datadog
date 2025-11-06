#![doc = include_str!("../README.md")]

use jiff::{Timestamp, Zoned};
use rmp_serde::Serializer as MpSerializer;
use serde::{Serialize, Serializer};
use std::{
    collections::HashMap,
    fmt::Write,
    marker::PhantomData,
    sync::{Arc, Mutex},
    thread::{JoinHandle, sleep, spawn},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tracing_core::{
    Event, Field, Level, Subscriber,
    field::Visit,
    span::{Attributes, Id, Record},
};
use tracing_subscriber::{
    Layer,
    layer::Context,
    registry::{LookupSpan, Scope},
};

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
    logging_enabled: bool,
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
    /// Creates a new [`DataDogTraceLayer`].
    ///
    /// `service`, `env`, and `version` are global tags injected into all spans.
    ///
    /// `agent_address` should be in the format `host:port`.
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
            logging_enabled: false,
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
                        .serialize(&mut MpSerializer::new(&mut body).with_struct_map())
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

    /// Enables log output to stdout in a format compatible with DataDog, including log correlation
    /// to APM.
    pub fn with_logs(mut self) -> Self {
        self.logging_enabled = true;
        self
    }

    #[cfg(feature = "http")]
    fn get_context(
        dispatch: &tracing_core::Dispatch,
        id: &Id,
        f: &mut dyn FnMut(&mut DataDogSpan),
    ) {
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

    fn on_event(&self, event: &Event<'_>, ctx: Context<'_, S>) {
        if !self.logging_enabled {
            return;
        }

        let mut fields = {
            let mut visitor = FieldVisitor::default();
            event.record(&mut visitor);
            visitor.fields
        };

        let mut message = fields.remove("message").unwrap_or_default();

        fields.extend(
            ctx.event_scope(event)
                .into_iter()
                .flat_map(Scope::from_root)
                .flat_map(|span| match span.extensions().get::<DataDogSpan>() {
                    Some(dd_span) => dd_span.meta.clone(),
                    None => panic!("Span not found, this is a bug"),
                }),
        );

        fields
            .into_iter()
            .try_for_each(|(k, v)| write!(&mut message, " {k}={v}"))
            .expect("Failed to write message");

        let (trace_id, span_id) = ctx
            .lookup_current()
            .and_then(|span| {
                span.extensions()
                    .get::<DataDogSpan>()
                    .map(|dd_span| (Some(dd_span.trace_id), Some(dd_span.span_id)))
            })
            .unwrap_or_default();

        let log = DataDogLog {
            timestamp: Zoned::now().timestamp(),
            level: event.metadata().level().to_owned(),
            message,
            trace_id,
            span_id,
        };

        let serialized = serde_json::to_string(&log).expect("Failed to serialize log");

        println!("{serialized}");
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

/// The v0.4 DataDog trace API format for spans. This is what we write to MessagePack.
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

/// A visitor that converts tracing span attributes to a [`DataDogSpan`].
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
        // Strings are broken out because their debug representation includes quotation marks.
        match field.name() {
            "service" => self.dd_span.service = value.to_string(),
            "span.type" => self.dd_span.r#type = value.to_string(),
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
            "span.type" => self.dd_span.r#type = format!("{value:?}"),
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

/// The DataDog structure log format. This is what we write to JSON.
#[derive(Serialize)]
struct DataDogLog {
    timestamp: Timestamp,
    #[serde(serialize_with = "serialize_level")]
    level: Level,
    message: String,
    #[serde(rename = "dd.trace_id", skip_serializing_if = "Option::is_none")]
    trace_id: Option<u64>,
    #[serde(rename = "dd.span_id", skip_serializing_if = "Option::is_none")]
    span_id: Option<u64>,
}

/// Serializes a `Level` to a string, e.g. `"INFO"`.
fn serialize_level<S: Serializer>(level: &Level, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(level.as_str())
}

/// A visitor that collects tracing attributes into a map.
#[derive(Default)]
struct FieldVisitor {
    fields: HashMap<String, String>,
}

impl Visit for FieldVisitor {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.fields
            .insert(field.name().to_string(), format!("{value:?}"));
    }
}

#[cfg(feature = "http")]
#[doc = "Functionality for working with distributed tracing HTTP headers"]
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
        ///
        /// This would be useful in HTTP server middleware.
        ///
        /// ```
        /// # let request = http::Request::builder().body(()).unwrap();
        /// use tracing_datadog::http::{DataDogContext, DistributedTracingContext};
        ///
        /// // Construct a new span.
        /// let span = tracing::info_span!("http.request");
        ///
        /// // Set the context on the span based on request headers.
        /// span.set_context(DataDogContext::from_w3c_headers(request.headers()));
        /// ```
        ///
        /// An alternative use case is setting the context on the current span, for example
        /// within `#[instrument]`ed functions.
        ///
        /// ```
        /// # let request = http::Request::builder().body(()).unwrap();
        /// use tracing_datadog::http::{DataDogContext, DistributedTracingContext};
        ///
        /// tracing::Span::current().set_context(DataDogContext::from_w3c_headers(request.headers()));
        /// ```
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
        ///
        /// ```
        /// # use http::Request;
        /// use tracing_datadog::http::DistributedTracingContext;
        ///
        /// // Build the request.
        /// let mut request = Request::builder().body(()).unwrap();
        ///
        /// // Inject distributed tracing headers.
        /// request.headers_mut().extend(tracing::Span::current().get_context().to_w3c_headers());
        ///
        /// // Execute the request.
        /// // ..
        /// ```
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
            self.0(dispatch, id, &mut f);
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

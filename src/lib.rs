#![doc = include_str!("../README.md")]

use jiff::{Timestamp, Zoned};
use reqwest::header::{self, HeaderMap, HeaderName, HeaderValue};
use rmp_serde::Serializer as MpSerializer;
use serde::{Serialize, Serializer, ser::SerializeMap};
use std::{
    collections::HashMap,
    fmt::{Debug, Display, Formatter},
    marker::PhantomData,
    ops::DerefMut,
    sync::{Arc, Mutex, mpsc},
    thread::{sleep, spawn},
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

/// A [`Layer`] that sends traces to Datadog.
///
/// ```
/// # use tracing_subscriber::prelude::*;
/// # use tracing_datadog::DatadogTraceLayer;
/// tracing_subscriber::registry()
///    .with(
///        DatadogTraceLayer::builder()
///            .service("my-service")
///            .env("production")
///            .version("git sha")
///            .agent_address("localhost:8126")
///            .build()
///            .expect("failed to build DatadogTraceLayer"),
///    )
///    .init();
/// ```
#[derive(Debug)]
pub struct DatadogTraceLayer<S> {
    buffer: Arc<Mutex<Vec<DatadogSpan>>>,
    service: String,
    default_tags: HashMap<String, String>,
    logging_enabled: bool,
    #[cfg(feature = "http")]
    with_context: http::WithContext,
    shutdown: mpsc::Sender<()>,
    _registry: PhantomData<S>,
}

impl<S> DatadogTraceLayer<S>
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    /// Creates a builder to construct a [`DatadogTraceLayer`].
    pub fn builder() -> DatadogTraceLayerBuilder<S> {
        DatadogTraceLayerBuilder {
            service: None,
            default_tags: HashMap::new(),
            agent_address: None,
            container_id: None,
            logging_enabled: false,
            phantom_data: Default::default(),
        }
    }

    #[cfg(feature = "http")]
    fn get_context(
        dispatch: &tracing_core::Dispatch,
        id: &Id,
        f: &mut dyn FnMut(&mut DatadogSpan),
    ) {
        let subscriber = dispatch
            .downcast_ref::<S>()
            .expect("Subscriber did not downcast to expected type, this is a bug");
        let span = subscriber.span(id).expect("Span not found, this is a bug");

        let mut extensions = span.extensions_mut();
        if let Some(dd_span) = extensions.get_mut::<DatadogSpan>() {
            f(dd_span);
        }
    }
}

impl<S> Drop for DatadogTraceLayer<S> {
    fn drop(&mut self) {
        let _ = self.shutdown.send(());
    }
}

impl<S> Layer<S> for DatadogTraceLayer<S>
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_new_span(&self, attrs: &Attributes<'_>, id: &Id, ctx: Context<'_, S>) {
        let span = ctx.span(id).expect("Span not found, this is a bug");
        let mut extensions = span.extensions_mut();

        let trace_id = span
            .parent()
            .map(|parent| {
                parent
                    .extensions()
                    .get::<DatadogSpan>()
                    .expect("Parent span didn't have a DatadogSpan extension, this is a bug")
                    .trace_id
            })
            .unwrap_or(rand::random_range(1..=u64::MAX));

        debug_assert!(trace_id != 0, "Trace ID is zero, this is a bug");

        let mut dd_span = DatadogSpan {
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
            meta: self.default_tags.clone(),
            ..Default::default()
        };

        attrs.record(&mut SpanAttributeVisitor::new(&mut dd_span));

        extensions.insert(dd_span);
    }

    fn on_record(&self, id: &Id, values: &Record<'_>, ctx: Context<'_, S>) {
        let span = ctx.span(id).expect("Span not found, this is a bug");
        let mut extensions = span.extensions_mut();

        if let Some(dd_span) = extensions.get_mut::<DatadogSpan>() {
            values.record(&mut SpanAttributeVisitor::new(dd_span));
        }
    }

    fn on_follows_from(&self, id: &Id, follows: &Id, ctx: Context<'_, S>) {
        let span = ctx.span(id).expect("Span not found, this is a bug");
        let mut extensions = span.extensions_mut();

        if let Some(dd_span) = extensions.get_mut::<DatadogSpan>() {
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

        fields.extend(
            ctx.event_scope(event)
                .into_iter()
                .flat_map(Scope::from_root)
                .flat_map(|span| match span.extensions().get::<DatadogSpan>() {
                    Some(dd_span) => dd_span.meta.clone(),
                    None => panic!("DatadogSpan extension not found, this is a bug"),
                }),
        );

        let message = fields.remove("message").unwrap_or_default();

        let (trace_id, span_id) = ctx
            .lookup_current()
            .and_then(|span| {
                span.extensions()
                    .get::<DatadogSpan>()
                    .map(|dd_span| (Some(dd_span.trace_id), Some(dd_span.span_id)))
            })
            .unwrap_or_default();

        let log = DatadogLog {
            timestamp: Zoned::now().timestamp(),
            level: event.metadata().level().to_owned(),
            message,
            trace_id,
            span_id,
            fields,
        };

        let serialized = serde_json::to_string(&log).expect("Failed to serialize log");

        println!("{serialized}");
    }

    fn on_enter(&self, id: &Id, ctx: Context<'_, S>) {
        let span = ctx.span(id).expect("Span not found, this is a bug");
        let mut extensions = span.extensions_mut();

        let now = epoch_ns();

        match extensions.get_mut::<DatadogSpan>() {
            Some(dd_span) if dd_span.start == 0 => dd_span.start = now,
            _ => {}
        }
    }

    fn on_exit(&self, id: &Id, ctx: Context<'_, S>) {
        let span = ctx.span(id).expect("Span not found, this is a bug");
        let mut extensions = span.extensions_mut();

        let now = epoch_ns();

        if let Some(dd_span) = extensions.get_mut::<DatadogSpan>() {
            dd_span.duration = now - dd_span.start
        }
    }

    fn on_close(&self, id: Id, ctx: Context<'_, S>) {
        let span = ctx.span(&id).expect("Span not found, this is a bug");
        let mut extensions = span.extensions_mut();

        if let Some(dd_span) = extensions.remove::<DatadogSpan>() {
            self.buffer.lock().unwrap().push(dd_span);
        }
    }

    // SAFETY: This is safe because the `WithContext` function pointer is valid
    // for the lifetime of `&self`.
    #[cfg(feature = "http")]
    unsafe fn downcast_raw(&self, id: std::any::TypeId) -> Option<*const ()> {
        match id {
            id if id == std::any::TypeId::of::<Self>() => Some(self as *const _ as *const ()),
            id if id == std::any::TypeId::of::<http::WithContext>() => {
                Some(&self.with_context as *const _ as *const ())
            }
            _ => None,
        }
    }
}

/// A builder for [`DatadogTraceLayer`].
pub struct DatadogTraceLayerBuilder<S> {
    service: Option<String>,
    default_tags: HashMap<String, String>,
    agent_address: Option<String>,
    container_id: Option<String>,
    logging_enabled: bool,
    phantom_data: PhantomData<S>,
}

/// An error that can occur when building a [`DatadogTraceLayer`].
#[derive(Debug)]
pub struct BuilderError(&'static str);

impl Display for BuilderError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}

impl std::error::Error for BuilderError {}

const DATADOG_LANGUAGE_HEADER: HeaderName = HeaderName::from_static("datadog-meta-lang");
const DATADOG_TRACER_VERSION_HEADER: HeaderName =
    HeaderName::from_static("datadog-meta-tracer-version");
const DATADOG_CONTAINER_ID_HEADER: HeaderName = HeaderName::from_static("datadog-container-id");

impl<S> DatadogTraceLayerBuilder<S>
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    /// Sets the `service`. This is required.
    pub fn service(mut self, service: impl Into<String>) -> Self {
        self.service = Some(service.into());
        self
    }

    /// Sets the `env`. This is required.
    pub fn env(mut self, env: impl Into<String>) -> Self {
        self.default_tags.insert("env".into(), env.into());
        self
    }

    /// Sets the `version`. This is required.
    pub fn version(mut self, version: impl Into<String>) -> Self {
        self.default_tags.insert("version".into(), version.into());
        self
    }

    /// Sets the `agent_address`. This is required.
    pub fn agent_address(mut self, agent_address: impl Into<String>) -> Self {
        self.agent_address = Some(agent_address.into());
        self
    }

    /// Adds a fixed default tag to all spans.
    ///
    /// This can be used multiple times for several tags.
    ///
    /// Default tags are overridden by tags set explicitly on a span.
    pub fn default_tag(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        let _ = self.default_tags.insert(key.into(), value.into());
        self
    }

    /// Sets the container ID. This enables infrastructure metrics in APM for supported platforms.
    pub fn container_id(mut self, container_id: impl Into<String>) -> Self {
        self.container_id = Some(container_id.into());
        self
    }

    /// Enables or disables structured logging with trace correlation to stdout.
    /// Disabled by default.
    pub fn enable_logs(mut self, enable_logs: bool) -> Self {
        self.logging_enabled = enable_logs;
        self
    }

    /// Consumes the builder to construct the tracing layer.
    pub fn build(self) -> Result<DatadogTraceLayer<S>, BuilderError> {
        let Some(service) = self.service else {
            return Err(BuilderError("service is required"));
        };
        if !self.default_tags.contains_key("env") {
            return Err(BuilderError("env is required"));
        };
        if !self.default_tags.contains_key("version") {
            return Err(BuilderError("version is required"));
        };
        let Some(agent_address) = self.agent_address else {
            return Err(BuilderError("agent_address is required"));
        };
        let container_id = match self.container_id {
            Some(s) => Some(
                s.parse::<HeaderValue>()
                    .map_err(|_| BuilderError("Failed to parse container ID into header"))?,
            ),
            _ => None,
        };

        let buffer = Arc::new(Mutex::new(Vec::new()));
        let exporter_buffer = buffer.clone();
        let url = format!("http://{}/v0.4/traces", agent_address);
        let (tx, rx) = mpsc::channel();

        spawn(move || {
            let client = {
                let mut default_headers = HeaderMap::from_iter([(
                    DATADOG_LANGUAGE_HEADER,
                    HeaderValue::from_static("rust"),
                )]);

                if let Some(container_id) = container_id {
                    default_headers.insert(DATADOG_CONTAINER_ID_HEADER, container_id);
                };

                reqwest::blocking::Client::builder()
                    .default_headers(default_headers)
                    .retry(reqwest::retry::for_host(agent_address).max_retries_per_request(2))
                    .build()
                    .expect("Failed to build reqwest client")
            };
            let mut spans = Vec::new();

            loop {
                if rx.try_recv().is_ok() {
                    break;
                }

                sleep(Duration::from_secs(1));

                std::mem::swap(&mut spans, exporter_buffer.lock().unwrap().deref_mut());

                if spans.is_empty() {
                    continue;
                }

                let mut body = vec![0b10010001];
                let _ = spans
                    .serialize(&mut MpSerializer::new(&mut body).with_struct_map())
                    .inspect_err(|error| println!("Error serializing spans: {error:?}"));

                spans.clear();

                let _ = client
                    .post(&url)
                    .header(DATADOG_TRACER_VERSION_HEADER, "v1.27.0")
                    .header(header::CONTENT_TYPE, "application/msgpack")
                    .body(body)
                    .send()
                    .inspect_err(|error| println!("Error exporting spans: {error:?}"));
            }
        });

        Ok(DatadogTraceLayer {
            buffer,
            service,
            default_tags: self.default_tags,
            logging_enabled: self.logging_enabled,
            #[cfg(feature = "http")]
            with_context: http::WithContext(DatadogTraceLayer::<S>::get_context),
            shutdown: tx,
            _registry: PhantomData,
        })
    }
}

/// Returns the current system time as nanoseconds since 1970.
fn epoch_ns() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("SystemTime is before UNIX epoch")
        .as_nanos() as i64
}

/// The v0.4 Datadog trace API format for spans. This is what we write to MessagePack.
#[derive(Default, Debug, Serialize)]
struct DatadogSpan {
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
    meta: HashMap<String, String>,
    error_code: i32,
}

/// A visitor that converts tracing span attributes to a [`DatadogSpan`].
struct SpanAttributeVisitor<'a> {
    dd_span: &'a mut DatadogSpan,
}

impl<'a> SpanAttributeVisitor<'a> {
    fn new(dd_span: &'a mut DatadogSpan) -> Self {
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

    fn record_debug(&mut self, field: &Field, value: &dyn Debug) {
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

/// The Datadog structure log format. This is what we write to JSON.
struct DatadogLog {
    timestamp: Timestamp,
    level: Level,
    message: String,
    trace_id: Option<u64>,
    span_id: Option<u64>,
    fields: HashMap<String, String>,
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
struct FieldVisitor {
    fields: HashMap<String, String>,
}

impl Visit for FieldVisitor {
    fn record_str(&mut self, field: &Field, value: &str) {
        self.fields
            .insert(field.name().to_string(), value.to_string());
    }

    fn record_debug(&mut self, field: &Field, value: &dyn Debug) {
        self.fields
            .insert(field.name().to_string(), format!("{value:?}"));
    }
}

#[cfg(feature = "http")]
#[doc = "Functionality for working with distributed tracing HTTP headers"]
pub mod http {
    use crate::DatadogSpan;
    use http::{HeaderMap, HeaderName};
    use tracing_core::{Dispatch, span::Id};

    const W3C_TRACEPARENT_HEADER: HeaderName = HeaderName::from_static("traceparent");

    /// The trace context for distributed tracing. This is a subset of the W3C trace context
    /// which allows stitching together traces with spans from different services.
    #[derive(Copy, Clone, Default)]
    pub struct DatadogContext {
        trace_id: u128,
        parent_id: u64,
    }

    impl DatadogContext {
        /// Parses a context for distributed tracing from W3C trace context headers.
        ///
        /// This would be useful in HTTP server middleware.
        ///
        /// ```
        /// # let request = http::Request::builder().body(()).unwrap();
        /// use tracing_datadog::http::{DatadogContext, DistributedTracingContext};
        ///
        /// // Construct a new span.
        /// let span = tracing::info_span!("http.request");
        ///
        /// // Set the context on the span based on request headers.
        /// span.set_context(DatadogContext::from_w3c_headers(request.headers()));
        /// ```
        ///
        /// An alternative use case is setting the context on the current span, for example
        /// within `#[instrument]`ed functions.
        ///
        /// ```
        /// # let request = http::Request::builder().body(()).unwrap();
        /// use tracing_datadog::http::{DatadogContext, DistributedTracingContext};
        ///
        /// tracing::Span::current().set_context(DatadogContext::from_w3c_headers(request.headers()));
        /// ```
        pub fn from_w3c_headers(headers: &HeaderMap) -> Self {
            Self::parse_w3c_headers(headers).unwrap_or_default()
        }

        fn parse_w3c_headers(headers: &HeaderMap) -> Option<Self> {
            let header = headers.get(W3C_TRACEPARENT_HEADER)?.to_str().ok()?;

            let parts: Vec<&str> = header.split('-').collect();
            if parts.len() != 4 {
                return None;
            }

            let Some(0) = u8::from_str_radix(parts[0], 16).ok() else {
                // Wrong version.
                return None;
            };

            let Some(0x01) = u8::from_str_radix(parts[3], 16).ok().map(|n| n & 0x01) else {
                // Not sampled.
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
            if self.is_empty() {
                return Default::default();
            }

            let header = format!(
                "{version:02x}-{trace_id:032x}-{parent_id:016x}-{trace_flags:02x}",
                version = 0,
                trace_id = self.trace_id,
                parent_id = self.parent_id,
                trace_flags = 1,
            );

            HeaderMap::from_iter([(W3C_TRACEPARENT_HEADER, header.parse().unwrap())])
        }

        /// Returns `true` if the context is empty, i.e. if it does not contain a trace ID or
        /// a parent ID.
        fn is_empty(&self) -> bool {
            self.trace_id == 0 || self.parent_id == 0
        }
    }

    // This function "remembers" the types of the subscriber so that we can downcast to something
    // aware of them without knowing those types at the call site. Adapted from tracing-error.
    #[derive(Debug)]
    pub(crate) struct WithContext(
        #[allow(clippy::type_complexity)]
        pub(crate)  fn(&Dispatch, &Id, f: &mut dyn FnMut(&mut DatadogSpan)),
    );

    impl WithContext {
        pub(crate) fn with_context(
            &self,
            dispatch: &Dispatch,
            id: &Id,
            mut f: &mut dyn FnMut(&mut DatadogSpan),
        ) {
            self.0(dispatch, id, &mut f);
        }
    }

    pub trait DistributedTracingContext {
        /// Gets the context for distributed tracing from the current span.
        fn get_context(&self) -> DatadogContext;

        /// Sets the context for distributed tracing on the current span.
        fn set_context(&self, context: DatadogContext);
    }

    impl DistributedTracingContext for tracing::Span {
        fn get_context(&self) -> DatadogContext {
            let mut ctx = None;

            self.with_subscriber(|(id, subscriber)| {
                let Some(get_context) = subscriber.downcast_ref::<WithContext>() else {
                    return;
                };
                get_context.with_context(subscriber, id, &mut |dd_span| {
                    ctx = Some(DatadogContext {
                        // NB Trace IDs can be 128-bit nowadays, but the 0.4 API still uses 64-bit.
                        trace_id: dd_span.trace_id as u128,
                        parent_id: dd_span.parent_id,
                    })
                });
            });

            ctx.unwrap_or_default()
        }

        fn set_context(&self, context: DatadogContext) {
            // Avoid setting a null context.
            if context.is_empty() {
                return;
            }

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

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::DatadogTraceLayer;
        use rand::random_range;
        use tracing::info_span;
        use tracing_subscriber::layer::SubscriberExt;

        #[test]
        fn w3c_trace_header_round_trip() {
            let context = DatadogContext {
                trace_id: random_range(1..=u128::MAX),
                parent_id: random_range(1..=u64::MAX),
            };

            let headers = context.to_w3c_headers();
            let parsed = DatadogContext::from_w3c_headers(&headers);

            assert_eq!(context.trace_id, parsed.trace_id);
            assert_eq!(context.parent_id, parsed.parent_id);
        }

        #[test]
        fn empty_context_doesnt_produce_w3c_trace_header() {
            assert!(DatadogContext::default().to_w3c_headers().is_empty());
        }

        #[test]
        fn w3c_trace_header_with_wrong_version_produces_empty_context() {
            let headers = HeaderMap::from_iter([(
                HeaderName::from_static("traceparent"),
                "01-00000000000000000000000000000001-0000000000000001-01"
                    .parse()
                    .unwrap(),
            )]);
            let context = DatadogContext::from_w3c_headers(&headers);
            assert!(context.is_empty());
        }

        #[test]
        fn w3c_trace_header_without_sampling_flag_produces_empty_context() {
            let headers = HeaderMap::from_iter([(
                HeaderName::from_static("traceparent"),
                "00-00000000000000000000000000000001-0000000000000001-00"
                    .parse()
                    .unwrap(),
            )]);
            let context = DatadogContext::from_w3c_headers(&headers);
            assert!(context.is_empty());
        }

        #[test]
        fn span_context_round_trip() {
            tracing::subscriber::with_default(
                tracing_subscriber::registry().with(
                    DatadogTraceLayer::builder()
                        .service("test-service")
                        .env("test")
                        .version("test-version")
                        .agent_address("localhost:8126")
                        .build()
                        .unwrap(),
                ),
                || {
                    let context = DatadogContext {
                        // Need to limit the size here as we only track 64-bit trace IDs.
                        trace_id: random_range(1..=u64::MAX) as u128,
                        parent_id: random_range(1..=u64::MAX),
                    };

                    let span = info_span!("test");

                    span.set_context(context);
                    let result = span.get_context();

                    assert_eq!(context.trace_id, result.trace_id);
                    assert_eq!(context.parent_id, result.parent_id);
                },
            );
        }

        #[test]
        fn empty_span_context_does_not_erase_ids() {
            tracing::subscriber::with_default(
                tracing_subscriber::registry().with(
                    DatadogTraceLayer::builder()
                        .service("test-service")
                        .env("test")
                        .version("test-version")
                        .agent_address("localhost:8126")
                        .build()
                        .unwrap(),
                ),
                || {
                    let context = DatadogContext::default();

                    let span = info_span!("test");

                    span.set_context(context);
                    let result = span.get_context();

                    assert_ne!(result.trace_id, 0);
                    assert_eq!(result.parent_id, 0);
                },
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builder_builds_successfully() {
        assert!(
            DatadogTraceLayer::<tracing_subscriber::Registry>::builder()
                .service("test-service")
                .env("test")
                .version("test-version")
                .agent_address("localhost:8126")
                .build()
                .is_ok()
        );
    }

    #[test]
    fn service_is_required() {
        let result = DatadogTraceLayer::<tracing_subscriber::Registry>::builder()
            .env("test")
            .version("test-version")
            .agent_address("localhost:8126")
            .build();
        assert!(result.unwrap_err().to_string().contains("service"));
    }

    #[test]
    fn env_is_required() {
        let result = DatadogTraceLayer::<tracing_subscriber::Registry>::builder()
            .service("test-service")
            .version("test-version")
            .agent_address("localhost:8126")
            .build();
        assert!(result.unwrap_err().to_string().contains("env"));
    }

    #[test]
    fn version_is_required() {
        let result = DatadogTraceLayer::<tracing_subscriber::Registry>::builder()
            .service("test-service")
            .env("test")
            .agent_address("localhost:8126")
            .build();
        assert!(result.unwrap_err().to_string().contains("version"));
    }

    #[test]
    fn agent_address_is_required() {
        let result = DatadogTraceLayer::<tracing_subscriber::Registry>::builder()
            .service("test-service")
            .env("test")
            .version("test-version")
            .build();
        assert!(result.unwrap_err().to_string().contains("agent_address"));
    }

    #[test]
    fn default_default_tags_include_env_and_version() {
        let layer: DatadogTraceLayer<tracing_subscriber::Registry> = DatadogTraceLayer::builder()
            .service("test-service")
            .env("test")
            .version("test-version")
            .agent_address("localhost:8126")
            .build()
            .unwrap();
        let default_tags = &layer.default_tags;
        assert_eq!(default_tags["env"], "test");
        assert_eq!(default_tags["version"], "test-version");
    }

    #[test]
    fn default_tags_can_be_added() {
        let layer: DatadogTraceLayer<tracing_subscriber::Registry> = DatadogTraceLayer::builder()
            .service("test-service")
            .env("test")
            .version("test-version")
            .agent_address("localhost:8126")
            .default_tag("foo", "bar")
            .default_tag("baz", "qux")
            .build()
            .unwrap();
        let default_tags = &layer.default_tags;
        assert_eq!(default_tags["foo"], "bar");
        assert_eq!(default_tags["baz"], "qux");
    }
}

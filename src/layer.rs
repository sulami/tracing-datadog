use crate::export::ApiVersion;
use crate::{
    log::{FieldVisitor, Log},
    span::{Span, SpanAttributeVisitor, SpanLink},
};
#[cfg(feature = "ahash")]
use ahash::AHashMap as HashMap;
use jiff::Zoned;
use reqwest::header::HeaderValue;
#[cfg(not(feature = "ahash"))]
use std::collections::HashMap;
use std::{
    borrow::Cow,
    fmt::{Display, Formatter},
    marker::PhantomData,
    sync::{Arc, Mutex, mpsc},
    thread::spawn,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tracing_core::{
    Event, Subscriber,
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
    buffer: Arc<Mutex<Vec<Span>>>,
    service: String,
    default_tags: HashMap<Cow<'static, str>, String>,
    logging_enabled: bool,
    with_context: crate::context::WithContext,
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
            default_tags: HashMap::from_iter([("span.kind".into(), "internal".to_string())]),
            agent_address: None,
            api_version: ApiVersion::V04,
            container_id: None,
            logging_enabled: false,
            pool_idle_timeout: None,
            phantom_data: Default::default(),
        }
    }

    fn get_context(dispatch: &tracing_core::Dispatch, id: &Id, f: &mut dyn FnMut(&mut Span)) {
        let subscriber = dispatch
            .downcast_ref::<S>()
            .expect("Subscriber did not downcast to expected type, this is a bug");
        let span = subscriber.span(id).expect("Span not found, this is a bug");

        let mut extensions = span.extensions_mut();
        if let Some(dd_span) = extensions.get_mut::<Span>() {
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
                    .get::<Span>()
                    .expect("Parent span didn't have a DatadogSpan extension, this is a bug")
                    .trace_id
            })
            .unwrap_or(rand::random_range(1..=u128::MAX));

        debug_assert!(trace_id != 0, "Trace ID is zero, this is a bug");

        let mut dd_span = Span {
            name: span.name().to_string(),
            service: self.service.clone(),
            r#type: "custom".into(),
            span_id: span.id().into_u64(),
            start: epoch_ns(),
            parent_id: span
                .parent()
                .map(|parent| parent.id().into_u64())
                .unwrap_or_default(),
            trace_id,
            meta: self.default_tags.clone(),
            metrics: {
                let mut m = HashMap::new();
                if span.parent().is_none() {
                    // Special tag to mark the service entry span.
                    m.insert("_dd.top_level".into(), 1.0);
                    m.insert("_dd.agent_psr".into(), 1.0);
                    m.insert("_dd.rule_psr".into(), 1.0);
                    m.insert("_dd.limit_psr".into(), 1.0);
                    m.insert("_sample_rate".into(), 1.0);
                    m.insert("_dd.tracer_kr".into(), 1.0);
                }
                m.insert("_sampling_priority_v1".into(), 2.0);
                m.insert("process_id".into(), std::process::id() as f64);
                m
            },
            ..Default::default()
        };

        attrs.record(&mut SpanAttributeVisitor::new(&mut dd_span));

        extensions.insert(dd_span);
    }

    fn on_record(&self, id: &Id, values: &Record<'_>, ctx: Context<'_, S>) {
        let span = ctx.span(id).expect("Span not found, this is a bug");
        let mut extensions = span.extensions_mut();

        if let Some(dd_span) = extensions.get_mut::<Span>() {
            values.record(&mut SpanAttributeVisitor::new(dd_span));
        }
    }

    fn on_follows_from(&self, id: &Id, follows: &Id, ctx: Context<'_, S>) {
        let span = ctx.span(id).expect("Span not found, this is a bug");
        let mut extensions = span.extensions_mut();

        let Some(other_span) = ctx.span(follows) else {
            // The other span might be filtered or closed, so we can't access it.
            return;
        };

        if let Some(dd_span) = extensions.get_mut::<Span>()
            && let Some(other_dd_span) = other_span.extensions().get::<Span>()
        {
            dd_span.span_links.push(SpanLink {
                trace_id: other_dd_span.trace_id,
                span_id: other_dd_span.span_id,
            })
        }
    }

    fn on_event(&self, event: &Event<'_>, ctx: Context<'_, S>) {
        if !self.logging_enabled {
            return;
        }

        let mut fields = {
            let mut visitor = FieldVisitor::default();
            event.record(&mut visitor);
            visitor.finish()
        };

        fields.extend(
            ctx.event_scope(event)
                .into_iter()
                .flat_map(Scope::from_root)
                .flat_map(|span| match span.extensions().get::<Span>() {
                    Some(dd_span) => dd_span.meta.clone(),
                    None => panic!("Datadog Span extension not found, this is a bug"),
                }),
        );

        let message = fields.remove("message").unwrap_or_default();

        let trace_context = ctx.lookup_current().and_then(|span| {
            span.extensions()
                .get::<Span>()
                .map(|dd_span| (dd_span.trace_id, dd_span.span_id))
        });

        let log = Log {
            timestamp: Zoned::now().timestamp(),
            level: event.metadata().level().to_owned(),
            message,
            trace_context,
            fields,
        };

        let serialized = serde_json::to_string(&log).expect("Failed to serialize log");

        println!("{serialized}");
    }

    fn on_enter(&self, id: &Id, ctx: Context<'_, S>) {
        let span = ctx.span(id).expect("Span not found, this is a bug");
        let mut extensions = span.extensions_mut();

        let now = epoch_ns();

        match extensions.get_mut::<Span>() {
            Some(dd_span) if dd_span.start == 0 => dd_span.start = now,
            _ => {}
        }
    }

    fn on_exit(&self, id: &Id, ctx: Context<'_, S>) {
        let span = ctx.span(id).expect("Span not found, this is a bug");
        let mut extensions = span.extensions_mut();

        let now = epoch_ns();

        if let Some(dd_span) = extensions.get_mut::<Span>() {
            dd_span.duration = now - dd_span.start
        }
    }

    fn on_close(&self, id: Id, ctx: Context<'_, S>) {
        let span = ctx.span(&id).expect("Span not found, this is a bug");
        let mut extensions = span.extensions_mut();

        if let Some(mut dd_span) = extensions.remove::<Span>() {
            // Enable trace metrics for select span kinds.
            if let Some("server" | "client" | "consumer" | "producer") =
                dd_span.meta.get("span.kind").map(String::as_str)
            {
                dd_span.metrics.insert("_dd.measured".into(), 1.0);
                dd_span.metrics.insert("_dd1.sr.eausr".into(), 1.0);
            }

            self.buffer.lock().unwrap().push(dd_span);
        }
    }

    // SAFETY: This is safe because the `WithContext` function pointer is valid
    // for the lifetime of `&self`.
    unsafe fn downcast_raw(&self, id: std::any::TypeId) -> Option<*const ()> {
        match id {
            id if id == std::any::TypeId::of::<Self>() => Some(self as *const _ as *const ()),
            id if id == std::any::TypeId::of::<crate::context::WithContext>() => {
                Some(&self.with_context as *const _ as *const ())
            }
            _ => None,
        }
    }
}

/// A builder for [`DatadogTraceLayer`].
pub struct DatadogTraceLayerBuilder<S> {
    service: Option<String>,
    default_tags: HashMap<Cow<'static, str>, String>,
    agent_address: Option<String>,
    api_version: ApiVersion,
    container_id: Option<String>,
    logging_enabled: bool,
    pool_idle_timeout: Option<Duration>,
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

    /// Sets the Datadog trace agent API version. Defaults to [`ApiVersion::V04`].
    pub fn api_version(mut self, api_version: ApiVersion) -> Self {
        self.api_version = api_version;
        self
    }

    /// Adds a fixed default tag to all spans.
    ///
    /// This can be used multiple times for several tags.
    ///
    /// Default tags are overridden by tags set explicitly on a span.
    pub fn default_tag(
        mut self,
        key: impl Into<Cow<'static, str>>,
        value: impl Into<String>,
    ) -> Self {
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

    /// Sets the idle timeout for HTTP connections to the Datadog agent.
    ///
    /// Connections that have been idle for longer than this duration are closed
    /// before reuse. Set to `None` to use the reqwest default (90 seconds).
    ///
    /// When the agent closes idle keep-alive connections server-side before the
    /// client's pool timeout, the next flush attempt on a stale connection will
    /// fail with a connection error. Setting this shorter than the agent's
    /// server-side keep-alive timeout prevents those errors.
    ///
    /// Use `Some(Duration::ZERO)` to disable connection pooling entirely,
    /// creating a fresh connection for every flush.
    pub fn pool_idle_timeout(mut self, timeout: impl Into<Option<Duration>>) -> Self {
        self.pool_idle_timeout = timeout.into();
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
        let (shutdown, shutdown_rx) = mpsc::channel();

        spawn(crate::export::exporter(
            agent_address,
            self.api_version,
            buffer.clone(),
            container_id,
            self.pool_idle_timeout,
            shutdown_rx,
        ));

        Ok(DatadogTraceLayer {
            buffer,
            service,
            default_tags: self.default_tags,
            logging_enabled: self.logging_enabled,
            with_context: crate::context::WithContext(DatadogTraceLayer::<S>::get_context),
            shutdown,
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
            .default_tag("static", "bar")
            .default_tag(String::from("dynamic"), "qux")
            .build()
            .unwrap();
        let default_tags = &layer.default_tags;
        assert_eq!(default_tags["static"], "bar");
        assert_eq!(default_tags["dynamic"], "qux");
    }
}

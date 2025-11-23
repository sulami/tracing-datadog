//! Functionality for working with distributed trace context.

use crate::span::DatadogSpan;
use tracing_core::{Dispatch, span::Id};

/// The trace context for distributed tracing. This is a subset of the W3C trace context
/// which allows stitching together traces with spans from different services.
///
/// It maps to [`tracing::Span`] via [`TracingContextExt`], and to other types via
/// [`TraceContextExt`].
#[derive(Copy, Clone, Default)]
pub struct DatadogContext {
    pub trace_id: u128,
    pub parent_id: u64,
}

impl DatadogContext {
    /// Returns `true` if the context is empty, i.e. if it does not contain a trace ID or
    /// a parent ID.
    pub(crate) fn is_empty(&self) -> bool {
        self.trace_id == 0 || self.parent_id == 0
    }
}

/// Extension trait for extracting/injecting [`DatadogContext`] using a [`Strategy`].
///
/// This trait allows handling of trace context in arbitrary container types. To implement it for
/// additional types, you need to implement [`Strategy`] for them.
///
/// You should not need to override this trait's default implementations.
///
/// See this example:
///
/// ```
/// use tracing_datadog::context::{DatadogContext, TraceContextExt, Strategy};
///
/// struct MyType(String);
///
/// impl TraceContextExt for MyType {}
///
/// struct MyTypeStrategy;
///
/// impl Strategy<MyType> for MyTypeStrategy {
///     fn inject(my_type: &mut MyType, context: DatadogContext) {
///         my_type.0 = format!("{}:{}", context.trace_id, context.parent_id);
///     }
///
///     fn extract(my_type: &MyType) -> DatadogContext {
///         let Some((trace_id, span_id)) = my_type.0.split_once(':') else {
///             return DatadogContext::default();
///         };
///
///         DatadogContext {
///             trace_id: trace_id.parse().unwrap_or_default(),
///             parent_id: span_id.parse().unwrap_or_default(),
///         }
///     }
/// }
///
/// // Round-trip through MyType.
/// let before = DatadogContext { trace_id: 123, parent_id: 456 };
/// let mut my_type = MyType(String::new());
///
/// my_type.inject_trace_context::<MyTypeStrategy>(before);
/// let after = my_type.extract_trace_context::<MyTypeStrategy>();
///
/// assert_eq!(before.trace_id, after.trace_id);
/// assert_eq!(before.parent_id, after.parent_id);
/// ```
///
/// See [`W3CTraceContextHeaders`](crate::http::W3CTraceContextHeaders) for a real example
/// implementation.
pub trait TraceContextExt {
    fn inject_trace_context<S>(&mut self, context: DatadogContext)
    where
        S: Strategy<Self>,
    {
        S::inject(self, context)
    }

    fn extract_trace_context<S>(&self) -> DatadogContext
    where
        S: Strategy<Self>,
    {
        S::extract(self)
    }
}

/// Strategy for extracting/injecting [`DatadogContext`] from/to a container type.
///
/// See [`TraceContextExt`] for an example of the intended use.
pub trait Strategy<T: ?Sized> {
    /// Injects a trace context into `T`.
    ///
    /// If the provided context is empty, `T` is unchanged.
    fn inject(container: &mut T, context: DatadogContext);

    /// Extracts a trace context from `T`.
    ///
    /// If `T` does not contain a valid trace context, the resulting context will be empty.
    fn extract(container: &T) -> DatadogContext;
}

/// This function "remembers" the types of the subscriber so that we can downcast to something
/// aware of them without knowing those types at the call site. Adapted from tracing-error.
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

// Technically, this duplicates TraceContextExt, but it has a nicer API because it doesn't require
// strategies or a mutable reference to inject context.

/// Extension trait for [`tracing::Span`] that allows extracting/injecting [`DatadogContext`].
///
/// For other types see [`TraceContextExt`].
pub trait TracingContextExt {
    /// Sets the distributed trace context on the tracing span.
    fn set_context(&self, context: DatadogContext);

    /// Gets the distributed trace context from the tracing span.
    fn get_context(&self) -> DatadogContext;
}

impl TracingContextExt for tracing::Span {
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
                    parent_id: dd_span.span_id,
                })
            });
        });

        ctx.unwrap_or_default()
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
                // NB Parent ID is asymmetrical, this span's ID becomes the next span's parent ID.
                assert_eq!(span.id().unwrap().into_u64(), result.parent_id);
            },
        );
    }

    #[test]
    fn empty_span_context_does_not_erase_trace_id() {
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
            },
        );
    }
}

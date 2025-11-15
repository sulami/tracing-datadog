//! Functionality for working with distributed tracing HTTP headers

use crate::span::DatadogSpan;
use http::{HeaderMap, HeaderName, HeaderValue};
use tracing_core::{Dispatch, span::Id};

const W3C_TRACEPARENT_HEADER: HeaderName = HeaderName::from_static("traceparent");
const DATADOG_TRACE_ID_HEADER: HeaderName = HeaderName::from_static("x-datadog-trace-id");
const DATADOG_PARENT_ID_HEADER: HeaderName = HeaderName::from_static("x-datadog-parent-id");
const DATADOG_SAMPLING_PRIORITY_HEADER: HeaderName =
    HeaderName::from_static("x-datadog-sampling-priority");
const DATADOG_TAGS_HEADER: HeaderName = HeaderName::from_static("x-datadog-tags");

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

    /// Parses a context for distributed tracing from Datadog headers.
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
    /// span.set_context(DatadogContext::from_datadog_headers(request.headers()));
    /// ```
    ///
    /// An alternative use case is setting the context on the current span, for example
    /// within `#[instrument]`ed functions.
    ///
    /// ```
    /// # let request = http::Request::builder().body(()).unwrap();
    /// use tracing_datadog::http::{DatadogContext, DistributedTracingContext};
    ///
    /// tracing::Span::current().set_context(DatadogContext::from_datadog_headers(request.headers()));
    /// ```
    pub fn from_datadog_headers(headers: &HeaderMap) -> Self {
        Self::parse_datadog_headers(headers).unwrap_or_default()
    }

    fn parse_datadog_headers(headers: &HeaderMap) -> Option<Self> {
        if headers
            .get(DATADOG_SAMPLING_PRIORITY_HEADER)?
            .to_str()
            .ok()?
            .parse::<u8>()
            .ok()?
            < 1
        {
            return None;
        }

        let lower_64_bits = headers
            .get(DATADOG_TRACE_ID_HEADER)?
            .to_str()
            .ok()?
            .parse::<u64>()
            .ok()? as u128;
        let parent_id = headers
            .get(DATADOG_PARENT_ID_HEADER)?
            .to_str()
            .ok()?
            .parse()
            .ok()?;

        let upper_64_bits = headers
            .get(DATADOG_TAGS_HEADER)
            .and_then(|header| {
                header.to_str().ok()?.split(',').find_map(|pair| {
                    pair.strip_prefix("_dd.p.tid=").and_then(|hex_value| {
                        u64::from_str_radix(hex_value, 16).map(|x| x as u128).ok()
                    })
                })
            })
            .unwrap_or_default();

        let trace_id = (upper_64_bits << 64) | lower_64_bits;

        Some(Self {
            trace_id,
            parent_id,
        })
    }

    /// Serializes a context for distributed tracing to Datadog headers.
    ///
    /// ```
    /// # use http::Request;
    /// use tracing_datadog::http::DistributedTracingContext;
    ///
    /// // Build the request.
    /// let mut request = Request::builder().body(()).unwrap();
    ///
    /// // Inject distributed tracing headers.
    /// request.headers_mut().extend(tracing::Span::current().get_context().to_datadog_headers());
    ///
    /// // Execute the request.
    /// // ..
    /// ```
    pub fn to_datadog_headers(&self) -> HeaderMap {
        if self.is_empty() {
            return Default::default();
        }

        let lower_64_bits = self.trace_id as u64;
        let upper_64_bits = (self.trace_id >> 64) as u64;

        let mut headers = HeaderMap::new();
        headers.insert(
            DATADOG_TRACE_ID_HEADER,
            lower_64_bits.to_string().parse().unwrap(),
        );
        headers.insert(
            DATADOG_PARENT_ID_HEADER,
            self.parent_id.to_string().parse().unwrap(),
        );
        headers.insert(
            DATADOG_SAMPLING_PRIORITY_HEADER,
            HeaderValue::from_static("1"),
        );
        headers.insert(
            DATADOG_TAGS_HEADER,
            format!("_dd.p.tid={upper_64_bits:016x}")
                .parse()
                .ok()
                .unwrap(),
        );
        headers
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
                    parent_id: dd_span.span_id,
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
            W3C_TRACEPARENT_HEADER,
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
            W3C_TRACEPARENT_HEADER,
            "00-00000000000000000000000000000001-0000000000000001-00"
                .parse()
                .unwrap(),
        )]);
        let context = DatadogContext::from_w3c_headers(&headers);
        assert!(context.is_empty());
    }

    #[test]
    fn datadog_headers_round_trip() {
        let context = DatadogContext {
            // We want to check that the upper 64 bits are preserved.
            trace_id: random_range((u64::MAX as u128 + 1)..=u128::MAX),
            parent_id: random_range(1..=u64::MAX),
        };

        let headers = context.to_datadog_headers();
        dbg!(&headers);
        let parsed = DatadogContext::from_datadog_headers(&headers);

        assert_eq!(context.trace_id, parsed.trace_id);
        assert_eq!(context.parent_id, parsed.parent_id);
    }

    #[test]
    fn empty_context_doesnt_produce_datadog_headers() {
        assert!(DatadogContext::default().to_datadog_headers().is_empty());
    }

    #[test]
    fn datadog_headers_without_sampling_produce_empty_context() {
        let headers = HeaderMap::from_iter([(
            DATADOG_SAMPLING_PRIORITY_HEADER,
            HeaderValue::from_static("0"),
        )]);
        let context = DatadogContext::from_datadog_headers(&headers);
        assert!(context.is_empty());
    }

    #[test]
    fn from_datadog_headers_works_without_tags_header() {
        let headers = HeaderMap::from_iter([
            (
                DATADOG_TRACE_ID_HEADER,
                HeaderValue::from_static("0000000000000001"),
            ),
            (
                DATADOG_PARENT_ID_HEADER,
                HeaderValue::from_static("0000000000000001"),
            ),
            (
                DATADOG_SAMPLING_PRIORITY_HEADER,
                HeaderValue::from_static("1"),
            ),
        ]);
        let context = DatadogContext::from_datadog_headers(&headers);
        assert_eq!(context.trace_id, 0x0000000000000001);
        assert_eq!(context.parent_id, 0x0000000000000001);
    }

    #[test]
    fn from_datadog_header_works_with_other_tags() {
        let headers = HeaderMap::from_iter([
            (
                DATADOG_TRACE_ID_HEADER,
                HeaderValue::from_static("0000000000000001"),
            ),
            (
                DATADOG_PARENT_ID_HEADER,
                HeaderValue::from_static("0000000000000001"),
            ),
            (
                DATADOG_SAMPLING_PRIORITY_HEADER,
                HeaderValue::from_static("1"),
            ),
            (
                DATADOG_TAGS_HEADER,
                HeaderValue::from_static("other=tags,_dd.p.tid=0000000000000002,more=tags"),
            ),
        ]);
        let context = DatadogContext::from_datadog_headers(&headers);
        assert_eq!(context.trace_id, 0x20000000000000001);
        assert_eq!(context.parent_id, 0x0000000000000001);
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

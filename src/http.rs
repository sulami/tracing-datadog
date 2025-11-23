//! Distributed trace context for HTTP.
//!
//! This module contains strategies for injecting/extracting distributed trace context into/from
//! HTTP headers using different formats.

use crate::context::{DatadogContext, Strategy, TraceContextExt};
use http::{HeaderMap, HeaderName, HeaderValue};

impl TraceContextExt for HeaderMap {}

/// W3C Trace Context headers strategy for [`HeaderMap`].
///
/// This uses the `traceparent` header.
///
/// ```
/// # use http::HeaderMap;
/// use tracing_datadog::{
///     context::{DatadogContext, TraceContextExt, TracingContextExt},
///     http::W3CTraceContextHeaders,
/// };
///
/// let mut headers = HeaderMap::new();
/// let current_span = tracing::Span::current();
///
/// // Extract W3C headers and set the context on the current span.
/// current_span.set_context(headers.extract_trace_context::<W3CTraceContextHeaders>());
///
/// // Set the current span's context as W3C headers.
/// headers.inject_trace_context::<W3CTraceContextHeaders>(current_span.get_context());
///```
pub struct W3CTraceContextHeaders;

const W3C_TRACEPARENT_HEADER: HeaderName = HeaderName::from_static("traceparent");

impl Strategy<HeaderMap> for W3CTraceContextHeaders {
    fn inject(headers: &mut HeaderMap, context: DatadogContext) {
        if context.is_empty() {
            return;
        }

        let header = format!(
            "{version:02x}-{trace_id:032x}-{parent_id:016x}-{trace_flags:02x}",
            version = 0,
            trace_id = context.trace_id,
            parent_id = context.parent_id,
            trace_flags = 1,
        );

        headers.insert(W3C_TRACEPARENT_HEADER, header.parse().unwrap());
    }

    fn extract(headers: &HeaderMap) -> DatadogContext {
        move || -> Option<DatadogContext> {
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

            Some(DatadogContext {
                trace_id,
                parent_id,
            })
        }()
        .unwrap_or_default()
    }
}

/// Datadog-specific header strategy for [`HeaderMap`].
///
/// This uses the various `x-datadog-` headers.
///
/// ```
/// # use http::HeaderMap;
/// use tracing_datadog::{
///     context::{DatadogContext, TraceContextExt, TracingContextExt},
///     http::DatadogHeaders,
/// };
///
/// let mut headers = HeaderMap::new();
/// let current_span = tracing::Span::current();
///
/// // Extract W3C headers and set the context on the current span.
/// current_span.set_context(headers.extract_trace_context::<DatadogHeaders>());
///
/// // Set the current span's context as W3C headers.
/// headers.inject_trace_context::<DatadogHeaders>(current_span.get_context());
///```
pub struct DatadogHeaders;

const DATADOG_TRACE_ID_HEADER: HeaderName = HeaderName::from_static("x-datadog-trace-id");
const DATADOG_PARENT_ID_HEADER: HeaderName = HeaderName::from_static("x-datadog-parent-id");
const DATADOG_SAMPLING_PRIORITY_HEADER: HeaderName =
    HeaderName::from_static("x-datadog-sampling-priority");
const DATADOG_TAGS_HEADER: HeaderName = HeaderName::from_static("x-datadog-tags");

impl Strategy<HeaderMap> for DatadogHeaders {
    fn inject(headers: &mut HeaderMap, context: DatadogContext) {
        if context.is_empty() {
            return;
        }

        let lower_64_bits = context.trace_id as u64;
        let upper_64_bits = (context.trace_id >> 64) as u64;

        headers.insert(
            DATADOG_TRACE_ID_HEADER,
            lower_64_bits.to_string().parse().unwrap(),
        );
        headers.insert(
            DATADOG_PARENT_ID_HEADER,
            context.parent_id.to_string().parse().unwrap(),
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
    }

    fn extract(headers: &HeaderMap) -> DatadogContext {
        move || -> Option<DatadogContext> {
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

            let upper_64_bits: u128 = headers
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

            Some(DatadogContext {
                trace_id,
                parent_id,
            })
        }()
        .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::random_range;

    #[test]
    fn w3c_trace_header_round_trip() {
        let context = DatadogContext {
            trace_id: random_range(1..=u128::MAX),
            parent_id: random_range(1..=u64::MAX),
        };

        let mut headers = HeaderMap::new();
        headers.inject_trace_context::<W3CTraceContextHeaders>(context);
        let parsed = headers.extract_trace_context::<W3CTraceContextHeaders>();

        assert_eq!(context.trace_id, parsed.trace_id);
        assert_eq!(context.parent_id, parsed.parent_id);
    }

    #[test]
    fn empty_context_doesnt_produce_w3c_trace_header() {
        let mut headers = HeaderMap::new();
        headers.inject_trace_context::<W3CTraceContextHeaders>(DatadogContext::default());
        assert!(headers.is_empty());
    }

    #[test]
    fn w3c_trace_header_with_wrong_version_produces_empty_context() {
        let headers = HeaderMap::from_iter([(
            W3C_TRACEPARENT_HEADER,
            "01-00000000000000000000000000000001-0000000000000001-01"
                .parse()
                .unwrap(),
        )]);
        let context = headers.extract_trace_context::<W3CTraceContextHeaders>();
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
        let context = headers.extract_trace_context::<W3CTraceContextHeaders>();
        assert!(context.is_empty());
    }

    #[test]
    fn datadog_headers_round_trip() {
        let context = DatadogContext {
            // We want to check that the upper 64 bits are preserved.
            trace_id: random_range((u64::MAX as u128 + 1)..=u128::MAX),
            parent_id: random_range(1..=u64::MAX),
        };

        let mut headers = HeaderMap::new();
        headers.inject_trace_context::<DatadogHeaders>(context);
        let parsed = headers.extract_trace_context::<DatadogHeaders>();

        assert_eq!(context.trace_id, parsed.trace_id);
        assert_eq!(context.parent_id, parsed.parent_id);
    }

    #[test]
    fn empty_context_doesnt_produce_datadog_headers() {
        let mut headers = HeaderMap::new();
        headers.inject_trace_context::<DatadogHeaders>(DatadogContext::default());
        assert!(headers.is_empty());
    }

    #[test]
    fn datadog_headers_without_sampling_produce_empty_context() {
        let headers = HeaderMap::from_iter([(
            DATADOG_SAMPLING_PRIORITY_HEADER,
            HeaderValue::from_static("0"),
        )]);
        let context = headers.extract_trace_context::<DatadogHeaders>();
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
        let context = headers.extract_trace_context::<DatadogHeaders>();
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
        let context = headers.extract_trace_context::<DatadogHeaders>();
        assert_eq!(context.trace_id, 0x20000000000000001);
        assert_eq!(context.parent_id, 0x0000000000000001);
    }
}

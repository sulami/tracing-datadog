# tracing-datadog

A `tracing` exporter layer for Datadog, without dependencies on `opentelemetry`.

- Exporter for `tracing` traces to Datadog APM
- (Optional) Datadog-compatible log formatting and APM ↔ log correlation
- (Optional) Distributed tracing support for HTTP requests via W3C Trace Context headers
- (Optional) Container-ID tracking for infrastructure metrics in APM

## Features

- `http` - HTTP Trace Context header support (W3C Trace Context and Datadog)

## Usage

To enable both trace data and log collection, use the `DatadogTraceLayer` layer
in your `tracing` subscriber:

```rust
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
use tracing_datadog::DatadogTraceLayer;

tracing_subscriber::registry()
   .with(
       DatadogTraceLayer::builder()
           .service("my-service")
           .env("production")
           .version("git sha")
           .agent_address("localhost:8126")
           .enable_logs(true)
           .build()
           .expect("failed to build DatadogTraceLayer"),
   )
   .init();
```

Logs will be emitted to stdout in the Datadog JSON format.

### Span tag semantics

Certain span tags have special meaning in Datadog:

- `service` - can be used to override the service on a per-span basis
- `operation` - maps to Datadog APM operations
- `resource` - maps to Datadog APM resources
- `span.kind` - defaults to `internal`, but can be set to `client`, `server`, 
  `producer`, or `consumer`. Internal spans do not generate trace metrics.
- `span.type` - defaults to `custom`, but can be set to `web` for request 
  handlers, `http` for HTTP client requests, or any of `sql`, `cassandra`, 
  `memcached`, `mongodb`, `elasticsearch`, `opensearch`, `redis`, or `db` for 
  data store queries. `custom` can be used for any other type of span.

There are few other semantic conventions, like the ones for [errors](https://docs.datadoghq.com/logs/error_tracking/backend).
[This page](https://docs.datadoghq.com/opentelemetry/mapping/semantic_mapping)
lists a lot of them.

### Distributed tracing over HTTP

To enable distributed tracing over HTTP, enable the `http` feature and use
`DatadogContext` to extract and inject trace context from/into HTTP headers. 
See the API documentation for examples.

## Prior Art

Some code has been adopted for the following projects:

- [tracing-opentelemetry](https://github.com/tokio-rs/tracing-opentelemetry)
- [DatadogFormattingLayer](https://github.com/open-schnick/DatadogFormattingLayer/)
- [fastrace-datadog](https://github.com/fast/fastrace/tree/main/fastrace-datadog)

See also [komoju-datadog](https://github.com/komoju/komoju-datadog) for 
opinionated usage of this library.

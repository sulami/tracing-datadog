# tracing-datadog

A `tracing` exporter layer for DataDog, without dependencies on `opentelemetry`.

- Exporter for `tracing` traces to DataDog APM
- (Optional) DataDog-compatible log formatting and APM ↔ log correlation
- (Optional) Distributed tracing support for HTTP requests via W3C Trace Context headers
- (Optional) Container-ID tracking for infrastructure metrics in APM

## Features

- `http` - W3C Trace Context header support

## Usage

To enable both trace data and log collection, use the `DataDogTraceLayer` layer
in your `tracing` subscriber:

```rust
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
use tracing_datadog::DataDogTraceLayer;

tracing_subscriber::registry()
   .with(
       DataDogTraceLayer::builder()
           .service("my-service")
           .env("production")
           .version("git sha")
           .agent_address("localhost:8126")
           .enable_logs(true)
           .build()
           .expect("failed to build DataDogTraceLayer"),
   )
   .init();
```

Logs will be emitted to stdout in the DataDog JSON format.

### Span tag semantics

Certain span tags have special meaning in DataDog:

- `service` - can be used to override the service on a per-span basis
- `operation` - maps to DataDog APM operations
- `resource` - maps to DataDog APM resources
- `span.type` - defaults to `internal`, but can be set to `web` for request 
  handlers, `http` for HTTP client requests, or any of `sql`, `cassandra`, 
  `memcached`, `mongodb`, `elasticsearch`, `opensearch`, `redis`, or `db` for 
  data store queries. `custom` can be used for any other type of span.

There are few other semantic conventions, like the ones for [errors](https://docs.datadoghq.com/logs/error_tracking/backend).
[This page](https://docs.datadoghq.com/opentelemetry/mapping/semantic_mapping)
lists a lot of them.

### Distributed tracing over HTTP

To enable distributed tracing over HTTP, enable the `http` feature and use
`DataDogContext` to extract and inject trace context from/into HTTP headers. 
See the API documentation for examples.

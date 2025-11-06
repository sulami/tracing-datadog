# tracing-datadog

A `tracing` exporter layer for DataDog, without dependencies on `opentelemetry`.

- Exporter for `tracing` traces to DataDog APM
- (Optional) DataDog-compatible log formatting and APM ↔ log correlation
- (Optional) Distributed tracing support for HTTP requests via W3C Trace Context headers

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
       DataDogTraceLayer::new(
           "my-service",
           "production",
           "git-sha",
           "localhost:8126",
       )
       .with_logs(),
    )
    .init();
```

Logs will be emitted to stdout in the DataDog JSON format.

### Distributed tracing over HTTP

To enable distributed tracing over HTTP, enable the `http` feature and use
`DataDogContext` to extract and inject trace context from/into HTTP headers.

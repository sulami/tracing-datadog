## [0.6.3] - 2026-03-26

### 🐛 Bug Fixes

- Require serde's derive feature
## [0.6.2] - 2026-03-25

### 🚀 Features

- Add configuration for different trace API versions

### 💼 Other

- *(deps)* Bump jiff from 0.2.17 to 0.2.18
- *(deps)* Bump rand from 0.9.2 to 0.10.0
- *(deps)* Bump reqwest from 0.13.1 to 0.13.2
- *(deps)* Bump bytes from 1.10.1 to 1.11.1
- *(deps)* Bump jiff from 0.2.18 to 0.2.19
- *(deps)* Bump serde_json from 1.0.148 to 1.0.149

### 🚜 Refactor

- Split span types into internal and export

### ⚙️ Miscellaneous Tasks

- Release tracing-datadog version 0.6.2
## [0.6.1] - 2026-01-05

### 💼 Other

- *(deps)* Bump the tracing group with 2 updates
- *(deps)* Bump rmp-serde from 1.3.0 to 1.3.1
- *(deps)* Bump jiff from 0.2.16 to 0.2.17
- *(deps)* Bump serde_json from 1.0.145 to 1.0.148
- *(deps)* Bump reqwest from 0.12.26 to 0.13.1
- *(deps)* Remove a stray ryu dependency

### ⚙️ Miscellaneous Tasks

- Release tracing-datadog version 0.6.1
## [0.6.0] - 2025-12-18

### 🚀 Features

- Add support for 128-bit trace IDs

### 💼 Other

- Properly tag the license as MIT
- *(deps)* Bump reqwest from 0.12.24 to 0.12.26
- *(deps)* Bump the tracing group with 3 updates

### 🧪 Testing

- Add snapshot tests for span serialization

### ⚙️ Miscellaneous Tasks

- Release tracing-datadog version 0.6.0
## [0.5.1] - 2025-11-25

### 💼 Other

- *(deps)* Bump http from 1.3.1 to 1.4.0

### 📚 Documentation

- Fix the docs.rs build

### ⚙️ Miscellaneous Tasks

- Release tracing-datadog version 0.5.1
## [0.5.0] - 2025-11-24

### 🚀 Features

- [**breaking**] Handle distributed trace context in a generic way
- Use tracing to report trace export errors
- Optionally use AHash for hash maps

### 🐛 Bug Fixes

- Shut down the exporter thread if the tracer has been dropped

### 💼 Other

- Include the http feature on docs.rs builds

### 🚜 Refactor

- Serialize trace chunks in one go
- Store log trace context in one field

### ⚡ Performance

- Borrow span tag maps for serialization

### ⚙️ Miscellaneous Tasks

- Release tracing-datadog version 0.5.0
## [0.4.3] - 2025-11-21

### 🐛 Bug Fixes

- Set even more sampling-related tags to ensure sampling

### ⚙️ Miscellaneous Tasks

- Release tracing-datadog version 0.4.3
## [0.4.2] - 2025-11-21

### 🚀 Features

- Add more _dd tags for better sample rates
- Add the PID as a tag for correlation with processes

### ⚙️ Miscellaneous Tasks

- Release tracing-datadog version 0.4.2
## [0.4.1] - 2025-11-17

### 🐛 Bug Fixes

- Don't panic if we can't access a linked span
- Export spans as trace chunks

### ⚙️ Miscellaneous Tasks

- Release tracing-datadog version 0.4.1
## [0.4.0] - 2025-11-15

### 🚀 Features

- [**breaking**] Set the default span.kind to internal, span.type to custom
- Set Datadog metrics on spans for sampling & trace metrics
- Add support for span links via follows_from
- Report the crate version as the tracer version
- Report the number of traces sent to the agent
- Add support for Datadog trace context headers

### 🚜 Refactor

- Split up the massive module into smaller ones

### ⚡ Performance

- Use Cow for tag names
- Remove heap allocations for span metrics

### ⚙️ Miscellaneous Tasks

- Release tracing-datadog version 0.4.0
## [0.3.6] - 2025-11-14

### 🐛 Bug Fixes

- *(http)* Use the correct parent span ID in distributed context

### 🚜 Refactor

- Use constants for W3C trace context headers

### ⚙️ Miscellaneous Tasks

- Release tracing-datadog version 0.3.6
## [0.3.5] - 2025-11-14

### 🚀 Features

- Set the language on submitted traces to Rust
- Export traces more often

### 🐛 Bug Fixes

- Retry trace submission requests up to two times
- Serialize log fields into the structured fields

### 🚜 Refactor

- Use constants for headers

### ⚙️ Miscellaneous Tasks

- Release tracing-datadog version 0.3.5
## [0.3.4] - 2025-11-14

### 🐛 Bug Fixes

- Don't inherit a trace context if the upstream didn't sample
- Record string fields without quotation marks on logs

### ⚙️ Miscellaneous Tasks

- Release tracing-datadog version 0.3.4
## [0.3.3] - 2025-11-14

### 🐛 Bug Fixes

- Produce log tags ordered by key

### ⚙️ Miscellaneous Tasks

- Release tracing-datadog version 0.3.3
## [0.3.2] - 2025-11-14

### 🐛 Bug Fixes

- Fix the container ID header by converting it to lowercase

### 💼 Other

- Release a new version as 0.3.1 was not rebased

### 📚 Documentation

- Backfill the 0.3.1 changelog

### ⚡ Performance

- Use double-buffering for spans

### ⚙️ Miscellaneous Tasks

- Release tracing-datadog version 0.3.2
## [0.3.0] - 2025-11-13

### 🚀 Features

- Add the ability to set default tags on spans

### 🐛 Bug Fixes

- Avoid overwriting trace- and span-IDs when setting an empty context

### 🧪 Testing

- Add some tests for the layer builder

### ⚙️ Miscellaneous Tasks

- Release tracing-datadog version 0.3.0
## [0.2.0] - 2025-11-07

### 🐛 Bug Fixes

- [**breaking**] Correctly downcase the second D in Datadog

### ⚙️ Miscellaneous Tasks

- Release tracing-datadog version 0.2.0
## [0.1.0] - 2025-11-06

### ⚙️ Miscellaneous Tasks

- Release tracing-datadog version 0.1.0

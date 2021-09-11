# elfo

[![Crates.io][crates-badge]][crates-url]
[![Documentation][docs-badge]][docs-url]
[![MIT licensed][mit-badge]][mit-url]
[![Build Status][actions-badge]][actions-url]

[crates-badge]: https://img.shields.io/crates/v/elfo.svg
[crates-url]: https://crates.io/crates/elfo
[docs-badge]: https://docs.rs/elfo/badge.svg
[docs-url]: https://docs.rs/elfo
[mit-badge]: https://img.shields.io/badge/license-MIT-blue.svg
[mit-url]: https://github.com/loyd/elfo/blob/master/LICENSE
[actions-badge]: https://github.com/loyd/elfo/workflows/CI/badge.svg
[actions-url]: https://github.com/loyd/elfo/actions?query=workflow%3ACI+branch%3Amaster

**Note: this system is still in early development and is not for production.**

## Features
* Async actors with supervision and custom lifecycle
* Two-level routing system: between actor groups and inside them (sharding)
* Multiple protocols: actors (so-called gates) can handle messages from different protocols
* Multiple patterns of communication: regular messages, request-response (*TODO: subscriptions*)
* Config updating and distribution
* Appropriate for both low latency and high throughput tasks
* Tracing: all messages have `trace_id` that spread across the system
* Telemetry (via the `metrics` crate)
* Efficient dumping
* Seamless distribution across nodes *TODO*
* Hot Actor Replacement *TODO*
* Utils for simple testing
* Utils for benchmarking *TODO*

## Usage
To use `elfo`, add this to your `Cargo.toml`:
```toml
[dependencies]
elfo = { version = "0.1", features = ["full"] }

[dev-dependencies]
elfo = { version = "0.1", features = ["test-util"] }
```

## Examples
* [Basic usage](elfo/examples/usage.rs)
* [Testing](elfo/examples/test.rs)

Also check [The Actoromicon](http://actoromicon.rs/).

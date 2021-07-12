# elfo

[![Build status](https://travis-ci.org/elfo-rs/elfo.svg)](https://api.travis-ci.org/elfo-rs/elfo.svg?branch=master)
[![Crate info](https://img.shields.io/crates/v/elfo.svg)](https://crates.io/crates/elfo)
[![Documentation](https://docs.rs/elfo/badge.svg)](https://docs.rs/elfo)

**Note: this system is still in early development and is not for production.**

## Features
* Async actors with supervision and custom lifecycle
* Two-level routing system: between actor groups and inside them (sharding)
* Multiple protocols: actors (so-called gates) can handle messages from different protocols
* Multiple patterns of communication: regular messages, request-response (*TODO: subscriptions*)
* Config updating and distribution
* Appropriate for both low latency and high throughput tasks
* Tracing: all messages have `trace_id` that spread across the system
* Efficient inspecting *TODO*
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

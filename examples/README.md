# elfo examples

This directory contains a collection of examples that demonstrate the use of the `elfo` ecosystem.

Run `cargo run --bin <name>` to execute specific example if not specified something other.

## usage

Describes common concepts of `elfo`, also how to work with configuration, combine actors together, enable metrics, logs and dumps.

## network

Shows how to connect distributed actor groups.

For simplicity, it uses one common binary that runs a specific service based on the CLI argument:
```sh
cargo run --bin network --features network -- alice &
cargo run --bin network --features network -- bob
```

## stream

Demonstrates how to attach streams to an actor's context.

## test

Shows how to write functional tests for your actors.

Run it as `cargo test --bin test --features test-util`.

## tokio_console

Shows how to enable `tokio-console` support.

Run it as `RUSTFLAGS='--cfg tokio_unstable' cargo run --bin tokio_console --features tokio-tracing`.

## tokio_broadcast

Demonstrates how to attach other channels to an actor's context, e.g. `tokio::sync::broadcast`.

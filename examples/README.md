# elfo examples

This directory contains a collection of examples that demonstrate the use of the `elfo` ecosystem.

Run `cargo run --example <name>` to run specific example if not specified something other.

## usage

Describes common concepts of `elfo`, also how to work with configuration, combine actors together, enable metrics, logs and dumps.

## network

Shows how to connect distributed actor groups.

For simplicity, it uses one common binary that runs a specific service based on the CLI argument:
```sh
cargo run --example network -- alice &
cargo run --example network -- bob
```

## stream

Demonstrates how to attach streams to an actor's context.

## test

Shows how to write functional tests for your actors.

Run it as `cargo test --example test`.

Note that to use test utils (e.g. `Proxy`) you need to enable the `test-util` feature, however in the example it's already enabled.

## tokio-broadcast

Demonstrates how to attach other channels to an actor's context, e.g. `tokio::sync::broadcast`.

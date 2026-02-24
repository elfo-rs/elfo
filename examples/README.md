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

Shows how to enable `tokio-console` support. [The Actoromicon](https://actoromicon.rs/ch05-05-tokio-console.html).

Run it as `RUSTFLAGS='--cfg tokio_unstable' cargo run --bin tokio_console --features tokio-tracing`.

## tokio_broadcast

Demonstrates how to attach other channels to an actor's context, e.g. `tokio::sync::broadcast`.

## multi_runtime

This example demonstrates how to use multiple tokio runtimes within one elfo system in order to isolate different actor groups. [The Actoromicon](https://actoromicon.rs/ch08-02-multiple-runtimes.html).

Run it as `cargo run --bin multi_runtime --features unstable`.

Run `ps -T -p $(pidof multi_runtime)` to see the following output:
```
    PID    SPID TTY          TIME CMD
2163285 2163285 pts/1    00:00:00 multi_runtime
2163285 2163319 pts/1    00:00:00 producers#0
2163285 2163320 pts/1    00:00:00 workers#0
2163285 2163321 pts/1    00:00:00 workers#1
2163285 2163322 pts/1    00:00:00 workers#2
2163285 2163323 pts/1    00:00:00 default#0
```

This kind of isolation is useful for:
- Isolating real-time actors from other workloads.
- Implementing thread-per-core by assigning affinities.
- Preventing resource contention between actor groups.

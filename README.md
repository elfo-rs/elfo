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

Elfo is another actor system. Check [The Actoromicon](http://actoromicon.rs/).

**Note: this system is still in early development and is not for production.**

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

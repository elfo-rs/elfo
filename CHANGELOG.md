# Changelog
All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).


## [Unreleased]

### Added
- telemeter: interoperability with the `metrics` crate.

### Changed
- tls: deprecated in favor of `scope`.

## [0.1.19] - 2021-08-24
### Added
- signal: add `signal::Signal` in order to work with signals.
- configurer: reload configs on SIGHUP.
- logger: get spans back in order to enable filtering by `RUST_LOG`.
- logger: reopen a log file on SIGHUP and when the config is changed.
- logger: add `format.with_location` and `format.with_module` options to append `@location` and `@module` fields to logs.
- dumper: the dumping subsystem and an actor group to save messages on disk.
- `Message::NAME` and `Message::PROTOCOL`.

### Fixed
- `assert_msg!`: fix false positive `unreachable_patterns` warnings.
- `msg!`: fix lost `unreachable_patterns` warnings in some cases.
- `msg!`: support `A | B | C` where components aren't units right way.
- logger: support values containing `=`.

## [0.1.18] - 2021-06-23
### Changed
- Expose the `tls` module to work with the task-local storage.
- Mailboxes are now based on a growing queue instead of the fixed one.
- Increase the maximum size of mailboxes up to 100k messages.

## [0.1.17] - 2021-06-10
### Added
- `Proxy::sync()`: waits until the testable actor handles all previously sent messages.
- Expose `time::{pause, resume, advance}` under the `test-util` feature.
- `time::Stopwatch`.
- `message(transparent)`.

### Fixed
- Do not panic in case of late `resolve()` calls for `Any` requests.

## [0.1.16] - 2021-06-02
### Added
- docs: show features on docs.rs.

### Fixed
- Terminate/restart actors more accurately.

## [0.1.15] - 2021-05-31
### Fixed
- Requests that require only one response return the first _successful_ instead of the last one.

## [0.1.14] - 2021-05-27
## Added
- `Proxy::addr()`

## [0.1.12] - 2021-05-18
### Fixed
- Set last versions of subcrates.

## [0.1.11] - 2021-05-18
### Added
- logger: an actor group to log everything.
- Trace ID generation and propagation.
- `stream::Stream`: a wrapper to attach streams to a actor context.

### Changed
- configurer: update system configs before user ones.

## [0.1.10] - 2021-05-14
### Fixed
- `Proxy::subproxy`.

### Changed
- configurer: do not send `Ping`s.

## [0.1.9] - 2021-05-12
### Added
- configurer: nested config paths support.
  E.g. local topology name `gates.web` corresponds to the following TOML section: `[gates.web]`.
- `Context::group()` to get a group's address.

### Fixed
- `elfo::test::proxy`: a race condition at startup.

## [0.1.8] - 2021-05-06
### Added
- `Proxy::subproxy()`.

### Changed
- Deprecate `Proxy::set_addr()`.

## [0.1.7] - 2021-05-06
### Added
- `Proxy::set_addr()`.

## [0.1.6] - 2021-04-20
### Added
- `#[message]`: add the `part` attribute.

### Changed
- supervisor: log using a group's span.
- configurer: print a group name with errors.
- `assert_msg(_eq)!`: print unexpected messages.

## [0.1.5] - 2021-04-15
### Fixed
- Actually print error chains.

## [0.1.4] - 2021-04-15
### Fixed
- Print causes of `anyhow::Error`.

## [0.1.3] - 2021-04-08
### Added
- `msg!`: support `a @ A` pattern.

## [0.1.2] - 2021-04-08
### Added
- `msg!` matches against `enum`s.
- Add `Proxy::send_to()` to test subscriptions.

### Fixed
- Fix race condition at startup.
- Fix casts of panic messages.

### Changed
- Update startup mechanics.
- `Local<T>` implements `Debug` even if `T` doesn't.
- `msg!` accepts `A | B` patterns.

## [0.1.1] - 2021-04-03
### Added
- Add the "full" feature.

### Changed
- Move `configurer` to a separate crate.

## [0.1.0] - 2021-04-03
- Feuer Frei!


[unreleased]: https://github.com/elfo-rs/elfo/compare/elfo-0.1.19...HEAD
[0.1.19]: https://github.com/elfo-rs/elfo/compare/elfo-0.1.18...elfo-0.1.19
[0.1.18]: https://github.com/elfo-rs/elfo/compare/elfo-0.1.17...elfo-0.1.18
[0.1.17]: https://github.com/elfo-rs/elfo/compare/elfo-0.1.16...elfo-0.1.17
[0.1.16]: https://github.com/elfo-rs/elfo/compare/elfo-0.1.15...elfo-0.1.16
[0.1.15]: https://github.com/elfo-rs/elfo/compare/elfo-0.1.14...elfo-0.1.15
[0.1.14]: https://github.com/elfo-rs/elfo/compare/elfo-0.1.12...elfo-0.1.14
[0.1.12]: https://github.com/elfo-rs/elfo/compare/elfo-0.1.11...elfo-0.1.12
[0.1.11]: https://github.com/elfo-rs/elfo/compare/elfo-0.1.10...elfo-0.1.11
[0.1.10]: https://github.com/elfo-rs/elfo/compare/elfo-0.1.9...elfo-0.1.10
[0.1.9]: https://github.com/elfo-rs/elfo/compare/elfo-0.1.8...elfo-0.1.9
[0.1.8]: https://github.com/elfo-rs/elfo/compare/elfo-0.1.7...elfo-0.1.8
[0.1.7]: https://github.com/elfo-rs/elfo/compare/elfo-0.1.6...elfo-0.1.7
[0.1.6]: https://github.com/elfo-rs/elfo/compare/elfo-0.1.5...elfo-0.1.6
[0.1.5]: https://github.com/elfo-rs/elfo/compare/elfo-0.1.4...elfo-0.1.5
[0.1.4]: https://github.com/elfo-rs/elfo/compare/elfo-0.1.3...elfo-0.1.4
[0.1.3]: https://github.com/elfo-rs/elfo/compare/elfo-0.1.2...elfo-0.1.3
[0.1.2]: https://github.com/elfo-rs/elfo/compare/elfo-0.1.1...elfo-0.1.2
[0.1.1]: https://github.com/elfo-rs/elfo/compare/elfo-0.1.0...elfo-0.1.1
[0.1.0]: https://github.com/elfo-rs/elfo/releases/tag/elfo-0.1.0

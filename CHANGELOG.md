# Changelog
All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).


## [Unreleased]

## [0.1.35] - 2022-03-23
### Added
- topology: unstable support for multiple runtimes.
- dumping: support `#[message(dumping = "disabled")]`.
- context: run `task::yield_now()` after many `recv()` calls to prevent starving other actors.

### Changed
- message: use vtable directly instead of LTID.
- dumping: use human-friendly size for `max_dump_size`.

## [0.1.34] - 2022-02-25
### Added
- dumping, dumper: add `thread_id` to dumps (as the `th` field).
- telemetry: produce `elfo_allocated_bytes_total` and `elfo_deallocated_bytes_total` metrics (@msiling).

## [0.1.33] - 2022-02-07
### Added
- context: add `Context::try_send`.
- errors: add `TrySendError::map` and `RequestError::map`.
- dumping: write `ts` firstly to support the `sort` utility.
- dumping: support multiple dump classes.
- dumping: expose unstable API for external dumping.
- dumper: extract a dump's name if it's not specified.
- dumper: support the `{class}` variable in config's `path` param.
- dumper: don't dump large messages, configurable by `max_dump_size` param (64KiB by default).
- telemeter: add `Retention::ResetOnScrape` policy as the simplest way to protect against stabilization with time.
- scope: `try_set_trace_id`.

### Changed
- telemeter: use `Retention::ResetOnScrape` by default.

### Fixed
- dumper: don't dump partially invalid messages. Previously, it could lead to file corruption.
- Avoid rare `invalid LTID` errors.
- init: do not start termination if the memory tracker fails to read files.

## [0.1.32] - 2021-12-21
### Added
- stream: add `Stream::generate()` to generate a stream from a generator.
- init: check memory usage and terminate the system gracefully if the threshold is reached (`90%`).
- init: add the `elfo_memory_usage` metric.
- tracing: `TraceIdValidator` to check incoming raw trace ids.

### Changed
- init: rename to `system.init` and enable metrics for it.
- The `trace_id` module is deprecated in favor of `tracing`.

## [0.1.31] - 2021-12-09
### Added
- configurer: add the `TryReloadConfigs` request.
- configurer: warn if a group is updating a config suspiciously long time.
- pinger: an actor group that pings other groups to detect freezes.

### Changed
- context: `request(msg).from(addr)` is deprecated in favor of `request_to(addr, msg)`.
- Actors reuse a message's trace id when start instead of generating a new one.
- Actors reuse `Terminate`'s trace id after the mailbox is closed.
- supervisor: restart actors with a linear backoff, starting immediately and with 5s step.
- `Ping`s are handled automatically now.

### Fixed
- `ActorStatusReport`s are dumped after incoming messages.

## [0.1.30] - 2021-11-29
### Added
- `elfo_busy_time_seconds` metric.
- logging: add the `system.logging.targets` section to override logging options for specific targets.

### Changed
- logging: replace `max_rate` with `max_rate_per_level`.

## [0.1.29] - 2021-11-09
### Added
- `MoveOwnership` to transfer ownership over messaging.
- telemeter: `elfo_metrics_usage_bytes` metric.

### Changed
- telemeter: render new counters with `0` value during scraping to avoid [some problems](https://www.section.io/blog/beware-prometheus-counters-that-do-not-begin-at-zero/).

### Fixed
- context: sending methods return an error if a message is discarded by all recipients. Previously, such messages can sometimes be considered as delivered.
- telemeter: close the server before termination.

## [0.1.28] - 2021-10-14
### Added
- Expose `ActorMeta` and `ActorStatusKind`.
- Provide methods to inspect `ActorStatus`.
- `SubscribeToActorStatuses` and `ActorStatusReport` messages.

### Changed
- context: `Context::recv()` and `Context::try_recv()` panics if called again after returning `None`.
- logger: use `_location` and `_module` instead of `@location` and `@module`.
- logger: remove the cargo prefix from locations.
- telemeter: hide rendered metrics in dumps.
- dumping: responses are dumped with the `RequestName::Response` name.

### Fixed
- telemetry: emit metrics in `ctx.respond()`.

## [0.1.27] - 2021-09-27
### Fixed
- telemeter: remove duplicate actor_group/actor_key labels.

## [0.1.26] - 2021-09-27
### Added
- telemeter: periodically compact distributions between scrape requests.

### Fixed
- telemetry: preserve metrics per an actor key after respawning.
- dumper: update the config right way.

## [0.1.25] - 2021-09-24
### Fixed
- supervisor: remove an extra update of configs at startup.
- start: wait some time before exiting in case of errors at startup.

## [0.1.24] - 2021-09-23
### Added
- configurer: merge `[common]` section into all actor group's sections.

### Fixed
- A race condition that leads to `config is unset` at startup.
- telemetry: do not panic if used outside the actor system.

## [0.1.23] - 2021-09-20
### Added
- Graceful termination.
- logging: `elfo_emitted_events_total`, `elfo_limited_events_total` and `elfo_lost_events_total` metrics.
- logging: per group rate limiter, configurable via `system.logging.max_rate` (`1000` by default).
- dumping: per group rate limiter, configurable via `system.dumping.max_rate` (`100_000` by default).
- dumping: `elfo_emitted_dumps_total` and `elfo_limited_dumps_total` metrics.
- logger: `elfo_written_events_total` metric.
- configurer: reload configs forcibly on SIGUSR2.
- group: add the `restart_policy` method to specify a restarting behaviour.
- group: add the `termination_policy` method to specify a restarting behaviour.
- proxy: add `Proxy::finished()` to await termination and `Proxy::close()` to close a (sub)proxy's mailbox.
- dumping: add `dumping::hide()` to hide large fields.

### Fixed
- logger: a memory leak in case of a full channel.
- A new actor status: `Terminating`.

## [0.1.22] - 2021-09-13
### Fixed
- Set last versions of subcrates.

## [0.1.21] - 2021-09-13
### Added
- dumper: provide more detailed errors.

### Changed
- Replace the `elfo_inactive_actors_total` metric with more common `elfo_actor_status_changes_total` one.

## [0.1.20] - 2021-09-09
### Added
- telemeter: interoperability with the `metrics` crate.
- supervisor: `elfo_active_actors`, `elfo_inactive_actors_total`, `elfo_restarting_actors` metrics.
- context: `elfo_message_waiting_time_seconds`, `elfo_message_handling_time_seconds`, `elfo_sent_messages_total` metrics.
- dumping: `elfo_lost_dumps_total` metric.
- dumper: `elfo_written_dumps_total` metric.
- logger: add a filtering layer to control per group logging. Now it's possible to use `system.logging.max_level` under a group section in order to alter the filter settings.

### Fixed
- dumper: protect against serialization errors.

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


[unreleased]: https://github.com/elfo-rs/elfo/compare/elfo-0.1.35...HEAD
[0.1.35]: https://github.com/elfo-rs/elfo/compare/elfo-0.1.34...elfo-0.1.35
[0.1.34]: https://github.com/elfo-rs/elfo/compare/elfo-0.1.33...elfo-0.1.34
[0.1.33]: https://github.com/elfo-rs/elfo/compare/elfo-0.1.32...elfo-0.1.33
[0.1.32]: https://github.com/elfo-rs/elfo/compare/elfo-0.1.31...elfo-0.1.32
[0.1.31]: https://github.com/elfo-rs/elfo/compare/elfo-0.1.30...elfo-0.1.31
[0.1.30]: https://github.com/elfo-rs/elfo/compare/elfo-0.1.29...elfo-0.1.30
[0.1.29]: https://github.com/elfo-rs/elfo/compare/elfo-0.1.28...elfo-0.1.29
[0.1.28]: https://github.com/elfo-rs/elfo/compare/elfo-0.1.27...elfo-0.1.28
[0.1.27]: https://github.com/elfo-rs/elfo/compare/elfo-0.1.26...elfo-0.1.27
[0.1.26]: https://github.com/elfo-rs/elfo/compare/elfo-0.1.25...elfo-0.1.26
[0.1.25]: https://github.com/elfo-rs/elfo/compare/elfo-0.1.24...elfo-0.1.25
[0.1.24]: https://github.com/elfo-rs/elfo/compare/elfo-0.1.23...elfo-0.1.24
[0.1.23]: https://github.com/elfo-rs/elfo/compare/elfo-0.1.22...elfo-0.1.23
[0.1.22]: https://github.com/elfo-rs/elfo/compare/elfo-0.1.21...elfo-0.1.22
[0.1.21]: https://github.com/elfo-rs/elfo/compare/elfo-0.1.20...elfo-0.1.21
[0.1.20]: https://github.com/elfo-rs/elfo/compare/elfo-0.1.19...elfo-0.1.20
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

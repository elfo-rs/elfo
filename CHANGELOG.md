# Changelog
All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).


## [Unreleased]

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


[unreleased]: https://github.com/loyd/elfo/compare/elfo-0.1.5...HEAD
[0.1.5]: https://github.com/loyd/elfo/compare/elfo-0.1.4...elfo-0.1.5
[0.1.4]: https://github.com/loyd/elfo/compare/elfo-0.1.3...elfo-0.1.4
[0.1.3]: https://github.com/loyd/elfo/compare/elfo-0.1.2...elfo-0.1.3
[0.1.2]: https://github.com/loyd/elfo/compare/elfo-0.1.1...elfo-0.1.2
[0.1.1]: https://github.com/loyd/elfo/compare/elfo-0.1.0...elfo-0.1.1
[0.1.0]: https://github.com/loyd/elfo/releases/tag/elfo-0.1.0

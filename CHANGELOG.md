# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

<!-- ## [Unreleased] -->

## [v0.1.1] - 2026-03-25

### Added

- Added `-c` alias for `--compatible`
- Added crate aliases to cli output

### Changed

- Changed the default cache mode to `refresh`
- Fixed updates for `[target.'cfg(...)'.dependencies]` sections
- Fixed issue with workspace-inherited dependencies being incorrectly updated
- `-p/--package` now filters workspace packages

## [v0.1.0] - 2026-03-19

### Added

- Added `--cache <prefer-local|refresh|no-cache>` to control registry cache behavior.
- Added `--fail-on-updates` to exit with status code `2` when updates are available.
- Added a CLI usage section to the README with examples for core flows and common options (`-i/--interactive`, `-u/--update`, `-U/--upgrade`, `--compatible`, `--pre`, `-p/--package`, `--fail-on-updates`).

### Changed

- Cargo registry resolution now prefers local sparse index cache entries
- `--upgrade` now always runs `cargo update`, even when no version requirement changes are needed.
- Fixed renamed Cargo dependencies (e.g. `rand07 = { package = "rand", ... }`) being incorrectly treated as workspace-inherited dependencies during updates.

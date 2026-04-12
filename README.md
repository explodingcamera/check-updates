# `check-updates`

[<img alt="github" src="https://img.shields.io/badge/github-explodingcamera/check--updates-8da0cb?style=flat-square&labelColor=555555&logo=github" height="20">](https://github.com/explodingcamera/check-updates)
[<img alt="crates.io" src="https://img.shields.io/crates/v/check-updates.svg?style=flat-square&color=fc8d62&logo=rust" height="20">](https://crates.io/crates/check-updates)
[<img alt="build status" src="https://img.shields.io/github/actions/workflow/status/explodingcamera/check-updates/ci.yaml?branch=main&style=flat-square" height="20">](https://github.com/explodingcamera/check-updates/actions?query=branch%3Amain)

> check-updates is a Rust library and CLI tool for checking if your dependencies are up to date. It can be used as a cargo subcommand or as a standalone tool.

_Currently only supports `Crates.io`, but support for other package managers / registries is planned for the future._

## Installation

```bash
cargo install check-updates-cli
```

To install prebuilt binaries with [`cargo-binstall`](https://github.com/cargo-bins/cargo-binstall), use:

```bash
cargo binstall check-updates-cli
```

[![asciicast](https://asciinema.org/a/AJrlkt3ugmIvq4Y1.svg)](https://asciinema.org/a/AJrlkt3ugmIvq4Y1)

## CLI usage

Main ways to use it:

- Run `check-updates` to see available dependency updates.
- Use `check-updates -i` for interactive selection.
- Use `check-updates -u` to update version requirements in `Cargo.toml`.
- Use `check-updates -U` to update requirements and run `cargo update`.

Flags and options:

| Short | Long                | Description                                               |
| ----- | ------------------- | --------------------------------------------------------- |
| `-i`  | `--interactive`     | Interactive selection UI                                  |
| `-u`  | `--update`          | Update version requirements in `Cargo.toml`               |
| `-U`  | `--upgrade`         | Update requirements and run `cargo update`                |
| `-c`  | `--compatible`      | Only semver-compatible updates                            |
| `-p`  | `--package <NAME>`  | Only check specific workspace package(s); repeat for more |
| -     | `--root <DIR>`      | Root directory to search from                             |
| -     | `--verbose`         | Enable verbose output                                     |
| -     | `--cache <MODE>`    | Cache mode: `prefer-local`, `refresh`, or `no-cache`      |
| -     | `--pre`             | Include pre-release versions                              |
| -     | `--compact`         | Compact interactive mode (less spacing)                   |
| -     | `--fail-on-updates` | Exit with status code `2` when updates are available      |

For all options and flags, see:

```bash
check-updates --help
```

## See also

- [cargo-outdated](https://crates.io/crates/cargo-outdated) - A cargo subcommand for displaying when Rust dependencies are out of date
- [cargo-edit](https://crates.io/crates/cargo-edit) - A cargo subcommand for editing your Cargo.toml file
- [cargo-upgrades](https://crates.io/crates/cargo-upgrades) - A cargo subcommand for upgrading your dependencies
- [npm-check-updates](https://www.npmjs.com/package/npm-check-updates) - A command-line tool that allows you to find out which of your npm dependencies are outdated

## License

Licensed under either of [Apache License, Version 2.0](./LICENSE-APACHE) or [MIT license](./LICENSE-MIT) at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in check-updates by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.

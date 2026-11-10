# `check-updates`

[<img alt="github" src="https://img.shields.io/badge/github-explodingcamera/check--updates-8da0cb?style=flat-square&labelColor=555555&logo=github" height="20">](https://github.com/explodingcamera/check-updates)
[<img alt="crates.io" src="https://img.shields.io/crates/v/check-updates.svg?style=flat-square&color=fc8d62&logo=rust" height="20">](https://crates.io/crates/check-updates)
[<img alt="build status" src="https://img.shields.io/github/actions/workflow/status/explodingcamera/check-updates/ci.yaml?branch=main&style=flat-square" height="20">](https://github.com/explodingcamera/check-updates/actions?query=branch%3Amain)

A simple tool for upgrading your dependencies to the latest version. Supports `cargo`, `npm`, `pnpm`, and `bun`.

## Installation

```bash
cargo install check-updates
# or if you have cargo-binstall:
cargo binstall check-updates
```

You can also download prebuilt binaries from [GitHub Releases](https://github.com/explodingcamera/check-updates/releases).

[![asciicast](https://asciinema.org/a/AJrlkt3ugmIvq4Y1.svg)](https://asciinema.org/a/AJrlkt3ugmIvq4Y1)

## CLI usage

Run these commands from your project directory:

```bash
check-updates        # List available updates without changing files
check-updates -u     # Update version requirements
check-updates -U     # Update requirements and installed dependencies
check-updates -iU    # Choose updates interactively and apply them
check-updates --help # See all available options
```

Automatic updates are non-breaking by default. Add `-b` to include breaking updates.

## See also

- [cargo-outdated](https://crates.io/crates/cargo-outdated) - A cargo subcommand for displaying when Rust dependencies are out of date
- [cargo-edit](https://crates.io/crates/cargo-edit) - A cargo subcommand for editing your Cargo.toml file
- [cargo-upgrades](https://crates.io/crates/cargo-upgrades) - A cargo subcommand for upgrading your dependencies
- [npm-check-updates](https://www.npmjs.com/package/npm-check-updates) - A command-line tool that allows you to find out which of your npm dependencies are outdated

## License

Licensed under either of [Apache License, Version 2.0](./LICENSE-APACHE) or [MIT license](./LICENSE-MIT) at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in check-updates by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.

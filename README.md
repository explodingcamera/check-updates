# `check-updates`

<!--

todo:
- use https://github.com/axodotdev/cargo-dist
- concurrent fetching of versions
 -->

> check-updates is a Rust library and CLI tool for checking if your dependencies are up to date. It can be used as a cargo subcommand or as a standalone tool.
>
> _Currently only supports Cargo dependencies, but support for other package managers is planned for the future._

## Installation

```bash
cargo install --git https://github.com/explodingcamera/check-updates
```

## See also

- [cargo-outdated](https://crates.io/crates/cargo-outdated) - A cargo subcommand for displaying when Rust dependencies are out of date
- [cargo-edit](https://crates.io/crates/cargo-edit) - A cargo subcommand for editing your Cargo.toml file
- [cargo-upgrades](https://crates.io/crates/cargo-upgrades) - A cargo subcommand for upgrading your dependencies
- [npm-check-updates](https://www.npmjs.com/package/npm-check-updates) - A command-line tool that allows you to find out which of your npm dependencies are outdated

## License

Licensed under either of [Apache License, Version 2.0](./LICENSE-APACHE) or [MIT license](./LICENSE-MIT) at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in check-updates by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.

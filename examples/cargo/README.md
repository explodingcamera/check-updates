# CLI Examples

This directory contains sample Cargo projects you can use to test `check-updates` behavior.

- `workspace-demo/`: multi-member workspace with:
  - workspace deps (`workspace = true`)
  - per-package dependency versions
  - mixed requirements for the same crate across packages
  - renamed deps using different versions of the same crate
- `single-package-demo/`: one package with many outdated dependencies, including renamed duplicates, plus git/path deps.

Run from repository root:

```bash
cargo run -p check-updates-cli -- --root crates/check-updates-cli/examples/cargo/workspace-demo --interactive
cargo run -p check-updates-cli -- --root crates/check-updates-cli/examples/cargo/single-package-demo --interactive
```

Tip: use `--compact` to test compact interactive mode:

```bash
cargo run -p check-updates-cli -- --root crates/check-updates-cli/examples/cargo/workspace-demo --interactive --compact
```

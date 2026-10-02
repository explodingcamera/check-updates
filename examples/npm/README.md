# JavaScript examples

- `workspace-demo/` is a pnpm workspace declared in `pnpm-workspace.yaml`.
- `bun-demo/` is a single-package Bun project.

From the repository root, run:

```bash
cargo run -p check-updates-cli -- --root examples/npm/workspace-demo
cargo run -p check-updates-cli -- --root examples/npm/bun-demo
cargo run --example npm -- examples/npm/workspace-demo
```

The examples query the public npm registry. `-u` edits the example manifests. `-U` also installs dependencies with the detected package manager, without running lifecycle scripts. Add `--lockfile-only` to skip installation.

# Contributing to Sony Connect

Thanks for your interest! Sony Connect is a small hobby project; contributions are welcome.

## Getting started

See [Build from source](README.md#build-from-source) for dependencies. Layout:

- `crates/sony-core` — protocol library and the `sony-connect-cli` binary
- `app/` — Tauri v2 desktop/tray app (`app/src-tauri` Rust, `app/ui` static HTML/CSS/JS)
- `packaging/` — Arch PKGBUILDs and the `.desktop` file
- `.github/workflows/` — CI and release pipelines

## Before opening a pull request

```sh
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

- Keep pull requests focused; describe what you changed and how you tested it.
- If you add support for a new headset model, include the model name, firmware version,
  and (if possible) a `sony-connect-cli status --json` dump in the PR.
- Protocol changes: please cite your source (packet capture, existing open-source research).
  Do not contribute code or assets copied from Sony's proprietary apps.

## Reporting bugs

Open an issue with your distro, headset model, the output of `sony-connect-cli list` and
`sony-connect-cli status --json`, and `RUST_LOG=debug` logs if relevant.

By contributing you agree that your contributions are licensed under the MIT License.

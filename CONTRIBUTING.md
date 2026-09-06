# Contributing to TRend

Thank you for improving TRend. Contributions should preserve the boundary that
TRend renders media while applications own layout, input, scheduling, playback
policy, audio, and decoder lifetime.

## Development Setup

Install a current stable Rust toolchain with `rustfmt` and Clippy. Python 3.10+
is required only for the reference applications. Linux video features require
`pkg-config`, libvpx development files, and a compatible OpenH264 runtime; see
[README.md](README.md#requirements).

## Before Opening a Pull Request

Run from the repository root:

```text
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
python -m unittest "examples\python-tui\test_trend_tui.py" "examples\python-video-player\test_player.py"
```

Platform capability suites are available as `scripts/test-windows.bat` and
`scripts/test-linux-wsl2.bat`. Changes to rendering, decoding, timing, or the C ABI
should include focused regression coverage.

Keep changes scoped, avoid generated artifacts, and do not add media or copied
source unless its redistribution terms and provenance are documented. Preserve
the stable C ABI described in [docs/ffi.md](docs/ffi.md).

## Pull Requests

Describe the behavioral change, affected platforms or formats, and commands you
ran. By submitting a contribution, you agree that it is licensed under the
[Apache License 2.0](LICENSE).

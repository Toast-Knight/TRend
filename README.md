# TRend

TRend is a Rust library for converting borrowed RGBA8 frames into terminal
cells and incremental ANSI output. Its dependency-free core owns rendering;
optional adapters decode images, animations, and video while the host retains
layout, input, scheduling, audio, and decoder lifetime.

```text
decoded RGBA8 -> PixelFrameView -> Renderer -> CellFrame -> AnsiPresenter -> terminal
```

## Five-minute quickstart

Install a current stable Rust toolchain, clone the repository, and run these
commands from its root:

```powershell
cargo build --workspace
cargo run -p trend-first-render -- tests/fixtures/media/ColorTest.png
cargo run -p trend-test-viewer -- tests/fixtures/media/ColorTest.gif
```

The first command validates the workspace, the second demonstrates the complete
PNG-to-ANSI library pipeline, and the third opens animated playback. On Windows,
render the same GIF into a 15x15 top-right terminal region with:

```powershell
cargo build -p trend-python-tui-bridge --release
.\examples\terminal-overlay\play-top-right.bat
```

See [docs/getting-started.md](docs/getting-started.md) for prerequisites and
Linux commands, or [docs/usage.md](docs/usage.md) for viewer and embedding flows.

## Workspace

| Path | Purpose |
| --- | --- |
| `src/` | Dependency-free renderer, ANSI presenter, timing, and C ABI |
| `adapters/` | Image, animation, video, and unified media decoding |
| `examples/` | Minimal Rust render, terminal overlay, and Python hosts |
| `tools/` | Test viewer, subprocess bridge, and Windows UI lab |
| `tests/fixtures/media/` | Checked-in validation media |
| `scripts/` | Build, test, fixture, validation, and packaging entry points |
| `packaging/` | Package-local documentation and manifests |
| `dist/` | Generated packages; never source-controlled |

## Documentation

- [Architecture and ownership](docs/architecture.md)
- [Rust APIs and feature flags](docs/api.md)
- [Usage and integration](docs/usage.md)
- [C ABI](docs/ffi.md)
- [Bridge protocol](docs/bridge-protocol.md)
- [Testing](docs/testing.md)
- [Packaging](docs/packaging.md)
- [Releasing](docs/releasing.md)
- [Windows](docs/platforms/windows.md), [Linux](docs/platforms/linux.md), and
  [Android](docs/platforms/android.md)

## Build and verify

```powershell
cargo build --workspace --all-features
cargo test --workspace
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo fmt --all -- --check
```

Platform entry points live under `scripts/`:

```powershell
.\scripts\build-windows.bat
.\scripts\test-windows.bat
.\scripts\validate-repository.ps1
```

Windows and Linux packages are generated under `dist/` with sorted SHA-256
manifests. Android image/animation and terminal rendering are device-validated;
Android video playback is not implemented.

TRend does not decode or play audio. Video timestamps and playback clocks are
exposed so an embedding application can synchronize an external audio system.

See [CONTRIBUTING.md](CONTRIBUTING.md) before submitting changes and
[SECURITY.md](SECURITY.md) for vulnerability reporting. TRend is licensed under
the [Apache License 2.0](LICENSE). Third-party obligations are recorded in
[THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
# Getting started

## Prerequisites

- Current stable Rust with Cargo, rustfmt, and Clippy.
- Windows 10 or newer for the Media Foundation video backend.
- On Debian/Ubuntu, `pkg-config` and `libvpx-dev` for VP8/VP9; install the
  distribution OpenH264 runtime for the `linux-h264` feature.
- Python 3.10 or newer only for the Python reference hosts.
- An ANSI/VT-capable terminal such as Windows Terminal.

Install Rust from [rustup.rs](https://rustup.rs/) and confirm `rustc --version`
and `cargo --version` work.

## First render

From the repository root:

```powershell
cargo build --workspace
cargo run -p trend-first-render -- tests/fixtures/media/ColorTest.png
```

On Bash, the same commands work unchanged. The example should print a 40x12
color rendering and exit.

## Animation and video

```powershell
cargo run -p trend-test-viewer -- tests/fixtures/media/ColorTest.gif
```

Build all optional codecs when the host provides their platform dependencies:

```powershell
cargo build --workspace --all-features
```

Use `trend-test-viewer --help` for playback controls and `--supports <extension>`
to query the compiled capability set.

## Next steps

- Follow [usage.md](usage.md) to embed the Rust API, C ABI, or bridge.
- Read [architecture.md](architecture.md) before adding application policy.
- Run the checks in [testing.md](testing.md) before submitting a change.
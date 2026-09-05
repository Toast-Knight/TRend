# TRend

TRend is a Rust library that converts borrowed RGBA8 framebuffers into terminal
cells and incremental ANSI output. The core has no third-party dependencies;
optional workspace adapters add PNG, JPEG, GIF, and Windows Media Foundation
video decoding. Default-off features add experimental WebP and WebM support
plus platform-specific MKV, AVI, and MOV support without changing the core
dependency graph.

TRend also exposes frame timestamps, playback position, frame deadlines, and
seek timing so an embedding application can synchronize rendered video with an
external audio system. TRend does not decode or play audio itself.

See [APIS.md](APIS.md) for the API and feature overview and
[APPLICATION_INTEGRATION.md](APPLICATION_INTEGRATION.md) for embedding guidance.
Dependency attribution and distribution obligations are recorded in
[THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md), with an all-feature package
snapshot in [DEPENDENCY_LICENSES.md](DEPENDENCY_LICENSES.md).

## Requirements

- A current Rust toolchain with Cargo and Rust 2024 edition support.
- Windows when building or using the Media Foundation MP4/H.264 backend.
- On Debian/Ubuntu, `pkg-config` and `libvpx-dev` when enabling `linux-webm` or
	`linux-mkv`.
- On Debian, `libopenh264-8` when enabling `linux-h264` (package/soname may vary
  by distribution).
- Python 3.10 or newer only for the Python reference applications.
- Optional `ffplay` on `PATH` for reference-player audio.

Install Rust with [rustup](https://rustup.rs/), then confirm the toolchain:

```powershell
rustc --version
cargo --version
```

## Compile

Run commands from the repository root.

```powershell
# Compile the dependency-free core in debug mode
cargo build

# Compile the optimized core library
cargo build --release

# Compile every crate and tool in the workspace
cargo build --workspace

# Compile the complete workspace with optimizations
cargo build --workspace --release
```

The optimized C-compatible library is written to `target/release/trend.dll` on
Windows, `target/release/libtrend.so` on Linux, or
`target/release/libtrend.dylib` on macOS. The C declarations are in
[`include/trend.h`](include/trend.h).

Build one adapter or tool by package name:

```powershell
cargo build -p trend-image --release
cargo build -p trend-animation --release
cargo build -p trend-video --release
cargo build -p trend-media --release
cargo build -p trend-python-tui-bridge --release
cargo build -p trend-test-viewer --release
```

Enable experimental formats explicitly:

```powershell
# Static and animated WebP through the unified media API
cargo build -p trend-media --features webp --release

# Windows-only experimental WebM through Media Foundation
cargo build -p trend-media --features webm --release

# Linux experimental WebM through Matroska demuxing and system libvpx
cargo build -p trend-media --features linux-webm --release

# Windows experimental MKV through Media Foundation
cargo build -p trend-media --features mkv --release

# Linux experimental VP8/VP9 MKV through the existing Matroska backend
cargo build -p trend-media --features linux-mkv --release

# Linux experimental H.264 MKV through Matroska demuxing and system OpenH264
cargo build -p trend-media --features linux-h264 --release

# Windows experimental MJPEG AVI through Media Foundation
cargo build -p trend-media --features avi --release

# Windows experimental H.264/HEVC MOV through Media Foundation
cargo build -p trend-media --features mov --release

# Windows viewer with all experimental formats
cargo build -p trend-test-viewer --features webp,webm,mkv,avi,mov --release
```

`webp-spike` remains an alias for the earlier static WebP feature name. New
integrations should use `webp`.

Windows video uses Media Foundation for MP4/H.264 and optional WebM, MKV, MJPEG
AVI, or H.264/HEVC MOV. AVI and MOV remain default-off behind their respective
features; no other codec combinations are claimed for those containers.
Linux WebM and MKV VP8/VP9 are available through the separate default-off
`linux-webm` and `linux-mkv` features and require system libvpx. Linux H.264 MKV
is available through `linux-h264` and a system OpenH264 runtime. Capability-known unsupported containers and codec
pairs return `VideoError::UnsupportedContainer` or `UnsupportedCombination`;
hosts without a video backend return `UnsupportedPlatform`. Neither backend adds
an FFmpeg dependency.

`trend_video::Container`, `VideoCodec`, and `VideoDecoder::capabilities()` expose
the active backend's container/codec matrix. AVI declares only the verified
Windows MJPEG route; MOV declares only the verified Windows H.264 and HEVC
routes. Linux does not advertise AVI or MOV.

The tested Linux H.264 runtime is Debian's `libopenh264-8` 2.6.0. TRend tries
common `libopenh264.so` sonames at runtime; set `TREND_OPENH264_LIBRARY` to an
explicit compatible shared-library path when a distribution uses another name.

## Verify

Run the complete platform capability suites from Command Prompt or PowerShell:

```bat
test_windows.bat
test_linux_wsl2.bat
```

The Linux script uses the `Debian` WSL2 distribution by default. From Command
Prompt, override it when needed with:

```bat
set TREND_WSL_DISTRO=Ubuntu
test_linux_wsl2.bat
```

From PowerShell:

```powershell
$env:TREND_WSL_DISTRO = "Ubuntu"
.\test_linux_wsl2.bat
```

Both scripts run workspace tests, strict Clippy, formatting, and deterministic
viewer probes against the codecs advertised by that platform.

For interactive visual checks, run the matching platform menu:

```bat
visual_test_windows.bat
visual_test_linux_wsl2.bat
```

Each menu recursively discovers media under `Test runs Envs\Materials`, lists
only formats supported by its compiled viewer, and omits intentionally corrupt
regression fixtures. The Linux menu honors the same `TREND_WSL_DISTRO` override.

```powershell
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
```

Optional Python integration checks:

```powershell
python -m unittest "Test runs Envs\Python TUI\test_trend_tui.py" "Test runs Envs\Python Video Player\test_player.py"
python "Test runs Envs\Python Video Player\player.py" --self-test --no-audio
```

## Generate Rust API Documentation

```powershell
cargo doc --workspace --no-deps --open
```

## Contributing and Security

See [CONTRIBUTING.md](CONTRIBUTING.md) before submitting changes. Report
vulnerabilities through the process in [SECURITY.md](SECURITY.md), not through a
public issue.

TRend is licensed under the [Apache License 2.0](LICENSE). Copyright (c) 2026
DigitalDolos. Third-party components remain under their respective licenses.

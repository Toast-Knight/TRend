# Using TRend

TRend can be used as a Rust rendering library, a C ABI, a media viewer, or a
subprocess bridge. The renderer never owns an application event loop, terminal
layout, input, audio, or playback policy.

## Render a static image

The smallest complete Rust example decodes a PNG, borrows its RGBA frame,
renders a 40x12 `CellFrame`, and writes TrueColor ANSI:

```powershell
cargo run -p trend-first-render -- tests/fixtures/media/ColorTest.png
```

Its source is [examples/first-render/src/main.rs](../examples/first-render/src/main.rs).
The core sequence is:

1. Construct or decode RGBA8 pixels.
2. Validate them with `PixelFrameView`.
3. Render into a reusable `CellFrame` with `Renderer` and `RenderRequest`.
4. Reuse an `AnsiPresenter` to emit full then incremental terminal updates.

Applications should retain the `CellFrame` and `AnsiPresenter` across frames to
reuse allocations and incremental state.

## View media

Build and inspect the exact CLI contract:

```powershell
cargo build -p trend-test-viewer --release --all-features
.\target\release\trend-test-viewer.exe --help
```

Common commands on Windows are:

```powershell
.\target\release\trend-test-viewer.exe tests\fixtures\media\ColorTest.png
.\target\release\trend-test-viewer.exe --quality quality tests\fixtures\media\ColorTest.gif
.\target\release\trend-test-viewer.exe --probe 80x24 10 tests\fixtures\media\ColorTest.mp4
.\target\release\trend-test-viewer.exe --supports webp
```

On Linux, use `./target/release/trend-test-viewer` and forward-slash paths.
Format availability follows compiled Cargo features and platform codecs; query
it with `--supports` rather than assuming a backend exists.

## Render a positioned overlay

The official Windows example hosts the bridge and renders a GIF into a 15x15
region at the top-right of the current terminal:

```powershell
cargo build -p trend-python-tui-bridge --release
.\examples\terminal-overlay\play-top-right.bat
```

Pass a GIF path as the first argument. The PowerShell client demonstrates exact
byte framing, zero-byte incremental frames, terminal positioning, and clean
shutdown. See [bridge-protocol.md](bridge-protocol.md) before implementing
another host.

## Embed in Rust

Use path dependencies while developing alongside this repository:

```toml
[dependencies]
trend = { path = "../Terminal renderer - TRend" }
trend-image = { path = "../Terminal renderer - TRend/adapters/image" }
```

Use `trend-image` for PNG/JPEG/BMP/TIFF, `trend-animation` for GIF and optional
WebP, `trend-video` for platform video backends, or `trend-media` for one
codec-neutral source API. Detailed types and features are in [api.md](api.md),
and lifecycle boundaries are in [architecture.md](architecture.md).

## Embed from C or C++

Include `include/trend.h`, link the platform library, allocate at least
`viewport_width * viewport_height` `TrendCell` values, then call
`trend_render_rgba8` or `trend_render_rgba8_ex`. See [ffi.md](ffi.md) for ABI
rules and a minimal call shape.

## Use packaged binaries

Create a validated Windows package with:

```powershell
.\scripts\build-windows.bat
```

The output is `dist/windows`. Commands in its README are relative to that
directory. Linux and package contents are documented in [packaging.md](packaging.md).
# Architecture audit

Snapshot of what TRend is today, taken before any engine-level refactor.
Companion document: [architecture-proposal.md](architecture-proposal.md).

- Date: 2026-09-25
- Basis: commit `4691d41` plus uncommitted `README.md` / `.gitignore` edits
- Method: full read of the core crate (`src/`); public surface and dispatch
  paths of adapters, tools, and platform video backends. Tests were not run
  during the audit.

For ownership rules and host integration, see [architecture.md](architecture.md).
For current platform status and the Android milestone, see
[development-status.md](development-status.md).

## What TRend is

A Rust library that converts a borrowed RGBA8 frame into a grid of terminal
cells (always `▀` half-blocks with foreground/background colors) and encodes
those cells as incremental ANSI output. Optional adapter crates decode images,
animations, and video into RGBA8. The host owns input, layout, threads, the
event loop, and audio. TRend performs no audio work.

```text
file ─► trend-media (detect ─► decoder) ─► PixelFrameView (borrowed RGBA8)
     ─► Renderer (geometry ─► sample 2 px per cell ─► optional dither) ─► CellFrame
     ─► AnsiPresenter (diff vs previous ─► cursor moves + SGR colors) ─► bytes ─► stdout
```

## Workspace

| Crate / path | Role |
| --- | --- |
| `trend` (root, `src/`) | Zero-dependency core; built as `rlib` + `cdylib` |
| `adapters/image` | PNG, JPEG, BMP, TIFF, optional WebP ─► `DecodedImage` |
| `adapters/animation` | GIF and optional animated WebP; eager `DecodedAnimation` and streaming `AnimationDecoder` / `WebPAnimationDecoder` |
| `adapters/video` | `VideoDecoder` selected by `cfg`: Windows Media Foundation (BGRA ─► RGBA), Linux libvpx + Matroska (VP8/VP9), Linux openh264 (H.264), otherwise a stub |
| `adapters/media` | Format detection (magic, MIME, extension); `MediaSource` enum over all decoders; `PreviewSession` clock wrapper |
| `tools/test-viewer` | Interactive viewer; crossterm on Unix, hand-rolled Win32 console FFI on Windows |
| `tools/python-tui-bridge` | Line-based stdin/stdout protocol used by the Python example |
| `tools/ui-lab` | Windows-only GDI window with its own video worker thread |
| `examples/first-render` | Minimal PNG ─► ANSI example |
| `examples/fidelity_baseline.rs` | Root-crate example; only user of `experimental.rs` |
| `tests/` | `core_pipeline.rs`, `black_alpha_regression.rs`, `fixtures/media` |

## Core modules (`src/`)

| Module | Contents |
| --- | --- |
| `framebuffer.rs` | `PixelFrameView`: validated borrowed view with stride and timestamp; `PixelFormat::Rgba8` only |
| `geometry.rs` | `Viewport`, `FitMode` (contain/cover/stretch), `RenderTransform` (zoom, pan, alignment), `RenderGeometry` |
| `scale.rs` | Nearest, bilinear, and area sampling; alpha-correct integer math |
| `renderer.rs` | `Renderer` (stateless unit struct); per-cell loop taking an upper and lower sample |
| `cell.rs` | `Cell` (16 B, `#[repr(C)]`), `Color` (`kind` 0 = terminal default, 1 = RGB), `CellFrame` |
| `terminal.rs` | Env-based capability detection, `AnsiPresenter` (diff, full-screen or positioned, color modes), `render_at` render+present+timing helper |
| `color.rs`, `dither.rs` | ANSI 256/16 quantization, 4×4 Bayer ordered dither |
| `timing.rs` | `PlaybackClock`, `AnimationTimeline`, `FrameDecision` |
| `cache.rs`, `queue.rs`, `resize.rs` | Byte-bounded LRU `CellFrame` cache, bounded queue, latest-wins resize coalescer |
| `experimental.rs` | 8×8 glyph-matching renderer (parallel copy of the render loop) |
| `ffi.rs` + `include/trend.h` | C ABI |

## Public API

- **Core (Rust):** `Renderer::{render_into, render_with_options_into,
  render_to_slice, render_to_slice_with_options, render_to_slice_cancellable}`;
  `RenderRequest { viewport, fit, generation }`;
  `RenderOptions { sampling, dither_strength, transform, alpha_background }`;
  `AnsiPresenter::{new, with_capabilities, present, present_at, render_at}`;
  `TerminalCapabilities`, `TerminalEnvironment`; timing, cache, queue, and
  resize utilities.
- **Adapters:** `decode_png`, `decode_jpeg`, `decode_bmp`, `decode_tiff`,
  `decode_webp`; `decode_gif`, `decode_gif_streaming`, `decode_webp_streaming`;
  `VideoDecoder::{capabilities, open, frame_for_time, seek, restart}`;
  `MediaSource::{open, detect, frame_at, frame_at_with_timing, seek}`;
  `PreviewSession::{frame_at_media_time, start, frame_for_time, pause, resume}`.
- **C ABI:** `trend_version`, `trend_cell_size`, `trend_render_rgba8`,
  `trend_render_rgba8_ex`. It renders into cells only; ANSI presentation is not
  exposed.
- **Bridge protocol:** see [bridge-protocol.md](bridge-protocol.md). It's a test
  interface, not a versioned ABI.

## Dependencies

| Crate | Dependency | Used for |
| --- | --- | --- |
| `trend` | none | — |
| `trend-image` | `png`, `jpeg-decoder` | PNG, JPEG |
| `trend-image` | `image` (bmp, tiff features only) | BMP, TIFF |
| `trend-image`, `trend-animation` | `image-webp` (optional) | WebP still/animated |
| `trend-animation` | `gif` | GIF |
| `trend-video` | `windows` | Media Foundation (Windows) |
| `trend-video` | `libvpx-native-sys`, `matroska-demuxer` (optional) | Linux VP8/VP9 in WebM/MKV |
| `trend-video` | `openh264` (optional, runtime-loaded) | Linux H.264 |
| `trend-test-viewer` | `crossterm` (non-Windows) | Raw mode, input, terminal size |
| dev | `jpeg-encoder` | Fixture generation in tests |

## How rendering works

1. `RenderGeometry` treats the viewport as `width × (height × 2)` virtual
   pixels, fits the source into that area (contain/cover/stretch), and applies
   zoom, pan, and alignment as integer offsets.
2. For each cell, `Renderer` samples virtual rows `2y` and `2y+1` with the
   chosen filter. A zero-alpha sample means "terminal default color";
   partial alpha keeps straight RGB unless a matte is supplied.
3. Optional ordered dither is applied per virtual pixel.
4. The cell becomes `▀`, `▄`, or blank.
5. `AnsiPresenter` compares against the previous frame, emits a cursor move
   followed by runs of changed cells, and emits SGR color codes only when
   colors change. Monochrome mode remaps cells to a density ramp at
   presentation time.

For video and animation, the adapter selects the frame for a media time and
lends out its RGBA buffer, which then goes through the same path. Resizing
only changes the `Viewport`; the source is not reopened.

## Duplication and coupling

- **Three ways into the decoders.** `test-viewer` dispatches by extension
  instead of using `MediaSource` (it uses `MediaSource` only for probing) and
  duplicates `video_container_enabled`. `ui-lab` calls decoders directly and
  runs its own worker thread.
- **Quality mapping repeated.** Quality mode ─► `SamplingFilter` exists
  separately in the viewer, ui-lab, and bridge (and on the Python side).
- **Per-adapter repeats.** `DecodeLimits` and `DecodeError` are defined in all
  three decoder crates. Three animation types have near-identical APIs.
  `MediaSource` repeats one `match` over its decoder enum in almost every
  method, with extra `#[cfg(feature = "webp")]` arms. `duration_from_nanos`
  appears twice.
- **No video backend trait.** Backends conform to the same method set by
  convention only; nothing checks it at compile time.
- **Two Win32 FFI layers** in tools (console and GDI).
- **`terminal.rs` has too many jobs:** environment/TTY detection, ANSI
  encoding, cell diffing, and a render+present+timing helper (`render_at`).
- **Stale documents.** [architecture.md](architecture.md) says WebP is
  unsupported and video is Windows-only, contradicting
  [development-status.md](development-status.md).
  [repository-audit.md](repository-audit.md) references pre-cleanup paths.
- **Local weight.** The working folder holds `android-ndk-r27d/` (git-ignored),
  `dist/`, and `target/`. The core's `cdylib` crate type makes every dependent
  build also produce the shared library.

## Limits for future 2D/3D rendering

1. **The half-block assumption is spread across several modules.** Geometry
   doubles height, the renderer's inner loop assumes two samples per cell, and
   the monochrome fallback assumes `▀`/`▄`. There's no cell-encoding
   abstraction; the experimental glyph renderer copies the loop instead of
   plugging into it.
2. **No owned drawing surface.** The core has a borrowed input view and the
   output cell grid only. There's no owned RGBA target a 2D or 3D rasterizer
   could draw into before cell encoding.
3. **One source fills the whole output.** Each render clears and fills the
   entire `CellFrame` from a single image. There's no compositing, no layers,
   no drawing into a sub-rectangle, and no z-order. "Terminal default" is the
   only cell-level transparency.
4. **Cell aspect ratio is hard-coded at 1:2**, so each virtual pixel is
   assumed square. 3D projection and non-half-block encoders both need a
   configurable aspect ratio.
5. **No text or styling.** `Cell.attributes` and reserved bytes are unused, so
   there are no labels, HUDs, or bold/underline styles.
6. **Speed is fine for previews but untuned for real-time.** Each sample goes
   through `Option` values and checked arithmetic. The `Renderer` keeps no
   scratch buffers, rows aren't precomputed, and everything runs on one thread.
7. **RGBA8 only.** `PixelFormat` has one variant.

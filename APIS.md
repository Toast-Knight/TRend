# TRend APIs and Features

TRend separates pixel decoding, terminal rendering, presentation, and
application policy. Applications own input, layout, scheduling, decoder
lifetime, and audio. The library accepts decoded RGBA8 pixels and produces
terminal cells or ANSI bytes.

## Package Map

| Package | Path | Public role |
| --- | --- | --- |
| `trend` | `src/` | Rendering, geometry, cells, ANSI, timing, queues, cache, resize, and C ABI |
| `trend-image` | `adapters/image/` | Bounded PNG and JPEG decoding; experimental WebP feature |
| `trend-animation` | `adapters/animation/` | Bounded GIF and optional animated WebP decoding, compositing, and timeline control |
| `trend-video` | `adapters/video/` | Bounded Windows MP4 plus experimental WebM/MKV and Linux Matroska decoding |
| `trend-media` | `adapters/media/` | Unified file opening, metadata, frames, seeking, and preview sessions |

## Core Rust API

The crate root re-exports the main types. Lower-level helpers remain available
through the public `cache`, `cell`, `color`, `dither`, `experimental`, `ffi`,
`framebuffer`, `geometry`, `queue`, `renderer`, `resize`, `scale`, `terminal`,
and `timing` modules.

### Input and Cells

- `PixelFrameView<'a>` is a validated, borrowed pixel buffer. `new` checks
  dimensions, stride, buffer length, and overflow. Accessors expose pixels,
  dimensions, stride, `PixelFormat`, and timestamp.
- `PixelFormat` currently supports `Rgba8` only. `FrameError` reports zero
  dimensions, short stride/buffer, and size overflow.
- `Color` represents terminal-default or explicit RGB color. `Cell` is the
  stable 16-byte C-compatible glyph, foreground, background, and attribute
  record. Constructors support arbitrary glyphs and upper/lower half blocks.
- `CellFrame` owns a reusable terminal grid with width, height, generation,
  immutable/mutable cell access, resize, clear, capacity, and allocated-byte
  queries.
- `color::ansi256_index`, `color::ansi16_index`, and `dither::ordered_4x4`
  expose color quantization and ordered dithering helpers.

### Geometry and Rendering

- `Viewport` gives terminal width and height in cells.
- `FitMode::{Contain, Cover, Stretch}` controls letterboxing, cropping, or
  independent-axis scaling.
- `Alignment::{Start, Center, End}` and `RenderTransform` control horizontal
  and vertical alignment, zoom percentage, and cell-space pan.
- `RenderGeometry::calculate` and `calculate_transformed` compute the source
  sampling region. `GeometryError` reports invalid dimensions, zero zoom, and
  overflow.
- `SamplingFilter::{Nearest, Bilinear, Area}` selects speed or resampling
  quality. Area sampling is intended for downscaling.
- `RenderRequest` contains viewport, fit mode, and a generation token.
- `RenderOptions` contains sampling, dither strength, transform, and an
  optional alpha-compositing background.
- `Renderer::render_into` and `render_with_options_into` render into reusable
  `CellFrame` storage.
- `Renderer::render_to_slice` and `render_to_slice_with_options` render into a
  caller-owned `&mut [Cell]`.
- `Renderer::render_to_slice_cancellable` checks a callback between rows and
  returns `RenderStatus::{Complete, Cancelled}`.
- `RenderError` wraps geometry and output-buffer failures.

The production renderer uses Unicode half blocks, giving two vertical pixel
samples per terminal cell. Fully transparent samples use terminal-default
color; `alpha_background` can matte RGBA input against a chosen color.

### ANSI Presentation

- `TerminalColorMode::{TrueColor, Ansi256, Ansi16, Monochrome}` defines output
  color depth.
- `TerminalCapabilities` combines color mode and Unicode support.
  `CONSERVATIVE` selects monochrome ASCII, while `from_env_values` derives
  capabilities from `TERM`, `COLORTERM`, and `NO_COLOR` values supplied by the
  host.
- `AnsiPresenter::present` emits incremental full-frame ANSI output and clears
  when geometry changes.
- `AnsiPresenter::present_at` renders at a 1-based row and column, does not
  clear surrounding application UI, and restores terminal attributes.
- `AnsiPresenter::render_at` combines rendering and positioned presentation.
  `PositionedRenderOutput` returns ANSI bytes plus separate render and ANSI
  generation durations.

### Timing, Queues, Cache, and Resize

- `PlaybackClock` maps host wall time to media time and supports play, pause,
  seek, restart, and paused-state inspection.
- `AnimationTimeline` maps frame durations to `AnimationPosition` and supports
  play, pause, restart, time seeking, and frame seeking.
- `FrameDecision::for_timestamp` returns `Wait`, `Present`, or `Drop` from a
  media timestamp and lateness threshold.
- `BoundedQueue<T>` provides file-mode backpressure with `push_file` and
  live-mode oldest-frame eviction with `push_live`, plus pop and clear.
- `CellFrameCache<K>` is bounded by entry count and bytes and evicts the least
  recently used entries. `CacheInsert` reports the insertion outcome.
- `ResizeCoalescer` assigns generations to viewport updates and retains only
  the newest pending resize.

These APIs are also TRend's audio/video synchronization interface. An embedding
application may use its audio clock as `wall_now`, request video at the resulting
media position, compare actual presentation timestamps, and use
`FrameDecision` to wait, present, or drop. Audio decoding and output remain
external; synchronization metadata and presentation timing remain part of
TRend.

### Experimental Glyph API

`experimental::ExperimentalGlyphRenderer` exposes alternate multi-glyph
rendering through frame, slice, and cancellable slice methods.
`ExperimentalGlyphOptions`, `ExperimentalGlyphSet`, and `glyph_coverage`
configure this path. It is experimental and is not the active production
half-block renderer.

## Decoder APIs

All decoder limits are caller-configurable and default to bounded dimensions,
pixel counts, decoded bytes, and decoder buffers.

### Images: `trend-image`

- `decode_png(bytes, DecodeLimits)` decodes bounded PNG data to RGBA8.
- `decode_jpeg(bytes, DecodeLimits)` decodes bounded JPEG RGB or grayscale data
  to RGBA8.
- `DecodedImage` exposes dimensions, pixel bytes, and a timestamped
  `PixelFrameView`.
- `DecodeError` distinguishes codec, dimensions, memory, color type,
  animation, decoded-length, and framebuffer validation failures.
- `decode_webp` exists only with the default-off `webp` Cargo feature and
  handles static lossy, lossless, and alpha WebP. `decode_webp_spike` remains a
  compatibility alias.
- WebP remains experimental because `image-webp` does not account all internal
  codec workspace against its memory limit. TRend still checks encoded input,
  dimensions, pixels, and final RGBA storage before output allocation.

### GIF: `trend-animation`

- `decode_gif(bytes, DecodeLimits, wall_now)` decodes and composites a bounded
  GIF animation, including disposal and alpha behavior.
- `DecodedAnimation` exposes dimensions, frame count, decoded bytes, total
  duration, per-frame timestamps/durations/views, and the current timed frame.
- Playback methods are `play`, `pause`, `restart`, `seek_time`, and
  `seek_frame`; `position` exposes the active `AnimationPosition`.
- Limits include maximum dimensions, pixels, frames, decoded bytes, decoder
  bytes, and minimum frame duration.
- With the default-off `webp` feature, `decode_webp` decodes composited animated
  WebP frames and maps frame durations and loop counts into the same
  `DecodedAnimation` and `AnimationTimeline` used by GIF.

### Video: `trend-video`

- `Container` identifies WebM, MKV, MP4, AVI, and MOV. `VideoCodec` identifies
  VP8, VP9, MJPEG, H.264, HEVC, and AV1.
- `VideoDecoder::capabilities()` returns immutable `BackendCapabilities`.
  `containers`, `codecs_for`, `supports_container`, and `supports` allow callers
  to query the active backend without attempting a decode.
- `VideoDecoder::open(path, DecodeLimits)` opens a source. Windows uses Media
  Foundation. Linux supports WebM/MKV VP8/VP9 through libvpx and MKV/H.264
  through OpenH264 when their respective features are enabled.
- `VideoInfo` reports dimensions, duration, nominal frame rate, audio presence,
  decoder name, and `SeekAccuracy`.
- `frame_for_time`, `seek`, and `restart` control decoding. Metrics report
  buffered, decoded, dropped, and seek-discarded frames, retained bytes, and
  the first timestamp reached after seek.
- `VideoFrame` exposes timestamp, duration, decoder epoch, and a borrowed
  `PixelFrameView`.
- Seeking is `SeekAccuracy::KeyframeThenForward`. Audio presence is reported,
  but TRend does not decode or play audio.
- With the default-off `webm` feature, Windows Media Foundation accepts WebM
  through the same `VideoDecoder`. Real VP8 and VP9 fixtures are tested for
  RGBA output, timestamps, duration, seeking, bounds, and core rendering.
- The default-off `linux-webm` feature implies `webm` and uses
  `matroska-demuxer` with system libvpx. Matroska metadata remains authoritative
  for packets, timestamps, duration, audio presence, and seeking; libvpx only
  turns VP8/VP9 packets into tightly packed RGBA8 frames.
- The default-off `mkv` feature enables `.mkv` dispatch on Windows. The
  default-off `linux-mkv` feature implies `mkv` and reuses the Linux Matroska
  backend. The separate default-off `linux-h264` feature also implies `mkv` and
  adds system OpenH264 as an isolated H.264 packet decoder. Matroska retains
  packet, timestamp, duration, audio, and seek ownership; libvpx remains solely
  responsible for VP8/VP9.
- The default-off `avi` feature enables `.avi` dispatch on Windows. The declared
  capability is intentionally limited to `AVI / MJPEG`, verified with real
  normal, large, PCM-audio, and corrupt fixtures. Linux returns
  `VideoError::UnsupportedContainer { container: Container::Avi }`.
- The default-off `mov` feature enables `.mov` dispatch on Windows. The declared
  capabilities are limited to `MOV / H.264` and `MOV / HEVC`, verified with
  real normal, large, AAC-audio, and corrupt fixtures. Linux returns
  `VideoError::UnsupportedContainer { container: Container::Mov }`.

### Unified Media: `trend-media`

- `MediaSource::open(path, MediaLimits)` dispatches `.png`, `.jpg`/`.jpeg`,
  `.gif`, and `.mp4`, plus `.webp`, `.webm`, `.mkv`, `.avi`, or `.mov` when their
  respective features are enabled and the active backend declares support.
- `MediaInfo` reports `MediaKind`, dimensions, optional duration/frame rate,
  audio presence, and optional seek accuracy.
- `representative_frame`, `frame_at`, and `frame_at_with_timing` expose borrowed
  decoded frames. `MediaFrame` keeps requested and actual presentation
  timestamps distinct.
- `seek` and `seek_with_timing` reposition dynamic media. `SeekResult` reports
  the request, first reached frame timestamp, and accuracy.
- Runtime metrics expose retained bytes and buffered, decoded, dropped, and
  seek-discarded frame counts.
- `PreviewSession` owns a `MediaSource` and host-driven `PlaybackClock`.
  `start`, `frame_for_time`, `pause`, `resume`, `seek`, `restart`, `stop`, and
  `cancel` implement bounded, optionally looping `PreviewRange` playback.
- `SessionState` is `Stopped`, `Playing`, `Paused`, `Completed`, or `Cancelled`.

## C ABI

[`include/trend.h`](include/trend.h) defines a caller-buffer interface exported
by the `trend` `cdylib`. The stable functions are:

| Function | Purpose |
| --- | --- |
| `trend_version()` | Return the packed ABI version |
| `trend_cell_size()` | Return `sizeof(TrendCell)` for layout verification |
| `trend_render_rgba8(...)` | Render RGBA8 into a caller-owned cell buffer with defaults |
| `trend_render_rgba8_ex(...)` | Render with size-versioned `TrendRenderOptions` |

`TrendRenderOptions` selects sampling, dither strength, horizontal/vertical
alignment, zoom, and pan. `TrendFitMode`, `TrendSamplingFilter`, and
`TrendAlignment` provide numeric enum constants. `TrendResult` returns success
or null-pointer, frame, viewport, capacity, render, or options errors.

The caller owns every input and output allocation; the API does not retain
pointers or create threads. Allocate at least `viewport_width *
viewport_height` `TrendCell` values and compare `trend_cell_size()` with the
host layout before rendering. New C behavior should remain additive and
size-versioned.

## Feature and Platform Matrix

| Capability | Windows | Linux/macOS |
| --- | --- | --- |
| RGBA8 to cells and ANSI | Supported | Supported |
| Nearest, bilinear, area sampling | Supported | Supported |
| Truecolor, ANSI 256/16, monochrome, ASCII fallback | Supported | Supported |
| PNG and JPEG | Supported | Supported |
| GIF animation | Supported | Supported |
| MP4/H.264 video | Media Foundation/OS codecs | API stub; unsupported at runtime |
| Native audio playback | Not provided | Not provided |
| WebP lossy/lossless/alpha/animation | Experimental opt-in pure Rust decoder | Experimental opt-in pure Rust decoder |
| WebM/VP8 and WebM/VP9 | Experimental; Media Foundation codec availability | Experimental on Linux with `linux-webm`; unavailable on macOS |
| WebM/AV1 | Unavailable | Unavailable |
| MKV/H.264 | Experimental with `mkv`; Media Foundation codec availability | Experimental on Linux with `linux-h264`; unavailable on macOS |
| MKV/VP8 and MKV/VP9 | Experimental with `mkv`; Media Foundation codec availability | Experimental on Linux with `linux-mkv`; unavailable on macOS |
| AVI/MJPEG | Experimental with `avi`; Media Foundation | Explicitly unsupported |
| MOV/H.264 and MOV/HEVC | Experimental with `mov`; Media Foundation | Explicitly unsupported |
| MOV/VP9, other AVI codecs, MKV/AV1 | Unavailable | Unavailable |
| BMP, TIFF, ICO | Planned investigation | Planned investigation |

## Integration Contract

TRend deliberately does not provide terminal input handling, application
widgets, render loops, threading policy, audio decoding/playback, or a portable
video backend. It does provide the clocks, positions, timestamps, durations,
seek results, and deadline decisions needed for a host-managed synchronization
policy. Hosts select terminal capabilities, schedule frames, manage
cancellation and resize generations, write emitted ANSI bytes, and clear old
positioned regions when layouts move or shrink.

### Decoder Compatibility Requirements

New image, animation, container, and codec adapters must fit the existing
pipeline rather than redefine it. Existing timing APIs, frame timestamps,
playback positions, synchronization primitives, timing callbacks, and
presentation/deadline behavior are compatibility surfaces and must remain
available.

A new video adapter must supply:

- decoded frames and dimensions;
- a supported pixel format;
- presentation timestamps and frame timing;
- media duration when available;
- existing seek and timing metadata where the source supports them.

Containers may contain audio, but their adapters do not need to decode or play
it. They may report audio-stream presence for a host that synchronizes through
an external audio system.

### Experimental Matroska Backends

The first WebM path is implemented as a default-off feature over the existing
Windows Media Foundation `VideoDecoder`. It produces timestamped RGBA8 frames,
uses existing keyframe-then-forward seeking, enters `MediaSource`, and renders
through the unchanged core. VP8 and VP9 are tested independently. AV1 is not
claimed.

The Windows path adds no native dependency beyond the existing system backend,
but actual codec availability depends on the Windows installation. Linux's
separate `linux-webm` feature uses `matroska-demuxer` and system libvpx. It
requires `pkg-config` and libvpx development files to build and the matching
libvpx runtime to execute. macOS continues to return `UnsupportedPlatform`.
Neither path spawns or links FFmpeg, and neither decodes or plays audio.

MKV reuses those same private platform backends. Windows Media Foundation is
validated with H.264 and VP9 MKV. Linux uses the same Matroska demux/timing path
with libvpx for VP8/VP9 and system OpenH264 for H.264. Tests cover timestamps,
keyframe-then-forward seeking, bounds, corruption, audio metadata, and rendering.
AV1 remains rejected.

AVI uses the existing Windows Media Foundation backend and unchanged RGBA8
`VideoFrame` path. Probe files for MPEG-4 Part 2, Microsoft MPEG-4 v3, and raw
video also decoded on the development machine, but they are not advertised
because this milestone adds permanent regression fixtures only for MJPEG.

The same adapter rule applies to future MOV, BMP, TIFF, and ICO support. These
formats are not currently claimed as supported.

The remaining planned video order is Linux HEVC, then hardware acceleration.
Adding a `Container` enum value does not imply support: a format
becomes available only when the active backend declares at least one codec for
it and the corresponding opt-in dispatch feature is enabled.

Application ownership, embedding, and lifecycle guidance is available in
[APPLICATION_INTEGRATION.md](APPLICATION_INTEGRATION.md).

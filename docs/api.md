# TRend APIs and Features

TRend separates pixel decoding, terminal rendering, presentation, and
application policy. Applications own input, layout, scheduling, decoder
lifetime, and audio. The library accepts decoded RGBA8 pixels and produces
terminal cells or ANSI bytes.

## Package Map

| Package | Path | Public role |
| --- | --- | --- |
| `trend` | `src/` | Rendering, geometry, cells, ANSI, timing, cache, resize, and C ABI |
| `trend-image` | `adapters/image/` | Bounded PNG and JPEG decoding; experimental WebP feature |
| `trend-animation` | `adapters/animation/` | Bounded GIF and optional animated WebP decoding, compositing, and timeline control |
| `trend-video` | `adapters/video/` | Bounded Windows MP4 plus experimental WebM/MKV and Linux Matroska decoding |
| `trend-media` | `adapters/media/` | Unified file opening, metadata, frames, seeking, and preview sessions |
| `trend-term` | `adapters/term/` | Terminal session: enter and restore, size and resize, keys, mouse, timed frame writes. See its [README](../adapters/term/README.md) |
| `trend-3d` | `adapters/3d/` | Dependency-free software 3D: model loading, materials, environments, cameras. See its [README](../adapters/3d/README.md) |
| `trend-text` | `adapters/text/` | Dependency-free scaled text drawn as pixels. See its [README](../adapters/text/README.md) |

## Core Rust API

The crate root re-exports the main types. Lower-level helpers remain available
through the public `cache`, `cell`, `color`, `compose`, `dither`, `encode`,
`ffi`, `framebuffer`, `geometry`, `pixel_buffer`, `raster`, `renderer`,
`resize`, `scale`, `surface`, `terminal`, `text`, and `timing` modules.

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
- `RenderError` wraps geometry, output-buffer, and region (`Region`) failures.

The production renderer uses Unicode half blocks, giving two vertical pixel
samples per terminal cell. Fully transparent samples use terminal-default
color; `alpha_background` can matte RGBA input against a chosen color.

`Renderer` is a compatibility facade over the staged pipeline: it uses the
same geometry (`calculate_for_spec`), sampling (`ImageSource`), dither, and
half-block cell rule as `ImageSource` + `HalfBlock`, streamed one cell row at a
time so it never allocates a pixel buffer. Its output, allocation behavior,
and cancellation points are unchanged.

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
- In `Monochrome` mode, cells with a foreground or background color are shaded
  by brightness as before. Cells with no color at all (such as `Density`
  output) are written with their own glyph; previously they always rendered
  blank.
- Once its buffers have grown to a frame's size, presenting further frames of
  that size allocates nothing.
- **Output options (M4, opt-in).** `AnsiPresenter::with_options(capabilities,
  OutputOptions)`, `set_options`, and `options` choose per presenter:
  - `synchronized_updates`: wraps each non-empty frame in DEC mode 2026
    (`CSI ? 2026 h` … `CSI ? 2026 l`) so supporting terminals show it at once;
    others ignore it. An unchanged frame still writes nothing.
  - `combined_colors`: one SGR sequence when a cell changes both colors
    (`CSI 38;2;r;g;b;48;2;r;g;b m`).
  - `compact_indexed_colors` (M5): in 256- and 16-color modes, skip color
    sequences that select the palette entry the terminal already has. The
    screen is identical. In M6 measurements it writes 2.1–5.4× fewer bytes
    in 256 colors and 1.7–3.4× fewer in 16, and presents 9–57% and 3–15%
    faster respectively (see [benchmarks.md](benchmarks.md)).

  `OutputOptions::default()` reproduces the original bytes exactly; the golden
  ANSI tests pin it.

  **Decision (M6): `compact_indexed_colors` stays opt-in for now and is
  planned to become the default at the next output-compatibility boundary**
  (the next minor version before 1.0, or 1.0 itself). The evidence supports
  it — same screen, fewer bytes, less CPU, no allocation — but turning it on
  changes the bytes of every 256- and 16-color stream, which the default is
  documented to preserve. The switch will be a one-line change to
  `OutputOptions::default()` plus re-recorded 256/16-color ANSI goldens; the
  screen-model tests already prove the screens are identical. Truecolor
  output is unaffected either way. Hosts that want it today set it
  explicitly. `combined_colors` (about 5–8% fewer bytes) and
  `synchronized_updates` are candidates for the same boundary.
- `AnsiPresenter::last_stats()` returns `PresentStats { bytes, cells, runs,
  color_sequences, attribute_sequences }` for the most recent presentation,
  and `capabilities()` reports the presenter's capabilities.
- **Attributes (M5).** `Cell::attributes` bits `Cell::BOLD`, `DIM`, `ITALIC`,
  `UNDERLINE`, `BLINK`, `REVERSE`, and `STRIKETHROUGH` are written as SGR
  attributes. Only changes are emitted, and positioned output resets them.
  Encoders always produce 0, so image output is byte-identical. Before M5
  the byte was ignored.
- `AnsiPresenter::present_view_at(view, row, column)` presents any `CellView`
  (such as a sub-view) like `present_at`.
- `AnsiPresenter::present_region_at(frame, region, row, column)` diffs and
  writes only `region` of a frame shown at an origin, keeping the presenter's
  knowledge of every other cell, so parts of a frame can be presented at
  different rates. It returns `RegionError` for an empty or out-of-bounds
  region.
- `AnsiPresenter::invalidate()` forgets what the terminal shows, so the next
  presentation redraws every cell (after other output overwrote the area).

### Timing, Cache, and Resize

- `PlaybackClock` maps host wall time to media time and supports play, pause,
  seek, restart, and paused-state inspection.
- `AnimationTimeline` maps frame durations to `AnimationPosition` and supports
  play, pause, restart, time seeking, and frame seeking.
- `FrameDecision::for_timestamp` returns `Wait`, `Present`, or `Drop` from a
  media timestamp and lateness threshold.
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

### Pixel Buffers and Cell Encoders

These APIs split rendering into stages so any application that can produce
pixels can use TRend as its terminal output. `Surface` (below) ties them
together and is the place to start; `Renderer` remains for fitting decoded
pictures into cells. The original design is recorded in
[phase-0-plan.md](history/phase-0-plan.md).

```text
app renderer or ImageSource -> PixelBuffer -> CellEncoder -> CellFrame/CellViewMut -> AnsiPresenter
```

- `PixelBuffer` is an owned, tightly packed, straight-alpha RGBA8 framebuffer:
  `new`, `resize` (clears to transparent and reuses capacity), `width`,
  `height`, `pixels`, `pixels_mut`, and `view`, which lends a
  `PixelFrameView`. Alpha 0 means no content.
- `SubcellGrid` is the number of pixels an encoder resolves per cell
  (`HALF_BLOCK` is 1x2, `QUADRANT` is 2x2, `SEXTANT` is 2x3, `BRAILLE` is
  2x4). `CellAspect` is the physical
  width:height of a terminal cell, supplied by the host (default 1:2).
- `RasterSpec::new(viewport, grid, cell_aspect)` gives the pixel `width`,
  `height`, and physical `pixel_aspect` a source must render for a viewport.
  Native renderers draw at that size and use `pixel_aspect` in their
  projection.
- `RenderGeometry::calculate_for_spec` is the fit/zoom/pan calculation for any
  spec. `calculate_transformed` delegates to it with a half-block spec.
- `ImageSource` resamples an existing `PixelFrameView` (decoded image, video
  frame, or application framebuffer with any stride) into a `PixelBuffer` for
  a spec, with `with_fit`, `with_sampling`, `with_transform`, and
  `with_alpha_background`. `render_region(spec, x, y, target)` renders the
  rectangle of the spec's surface at `(x, y)` with `target`'s size into any
  `PixelViewMut`: a region of a larger buffer or one row band at a time
  (check for cancellation between bands, or give bands to host threads).
  Every region is identical to the same pixels of `render_into`.
- `dither::ordered_4x4_in_place` applies the ordered dither to a `PixelBuffer`.
  `dither::ordered_4x4_region(&mut view, origin_x, origin_y, strength)`
  dithers a region or row band using its position in the full surface, so
  banded or regional rendering matches whole-surface dithering with no seams.
- `CellEncoder` has `grid()`, `encode(PixelFrameView, CellViewMut)`, and a
  provided `requirements()`. Encoders choose glyphs and colors only;
  `AnsiPresenter` still owns color depth, placement, and escape codes.
  `EncodeError::SizeMismatch` reports pixels that do not match
  `cells x grid`.
- `EncoderRequirements { glyphs, colors }` describes what an encoder's output
  needs: `GlyphRepertoire::{Ascii, BlockElements, Sextants, Braille}` and
  `ColorUse::{None, Foreground, ForegroundAndBackground}`.
  `supported_by(TerminalCapabilities)` tells a host whether a terminal shows
  that output faithfully. Choosing an encoder remains host policy.
- `HalfBlock` reproduces the existing renderer's cells exactly. `Quadrant`
  uses U+2596-U+259F quadrant glyphs with two colors per cell; since M5 it
  computes the same cells about 3-4x faster.
- `Sextant` (M5) uses the Unicode 13 sextant mosaics (U+1FB00-U+1FB3B, plus
  `▌`, `▐`, `█`) for 2x3 pixels per cell with two colors: 1.5x Quadrant's
  vertical detail. Most current terminals draw sextants natively; older
  fonts may not, which `TerminalCapabilities` can't detect, so offer it as a
  host option.
- `Braille` (M5) uses Braille patterns (U+2800-U+28FF) for 2x4 pixels per
  cell: dots in the foreground color over the background. The smaller part
  of each cell's two-color split becomes the dots. Its dots cover little of
  the cell, so it suits line art, plots, and wireframes more than
  photographs.
- `Sextant` and `Braille` share a two-color fitter that orders a cell's
  pixels along the axis between two far-apart colors and picks the best
  split. Its total error is within 0.03% of trying every split. `Quadrant`
  keeps its exhaustive search, which is now just as fast, so its output
  stays identical.
- `Density` is one pixel per cell, drawn as a character whose ink density
  follows brightness. `DensityRamp::{Ascii, Shades}` selects ` .:-=+*#%@` or
  ` ░▒▓█`, and `colored` optionally keeps the pixel's color as the
  foreground. `Density::ASCII` (no color) works faithfully on every terminal,
  including monochrome. It assumes light glyphs on a dark background.
- `CellFrame::view` and `view_mut` lend `CellView` (`width`, `height`, `row`,
  `sub`) and `CellViewMut` (`width`, `height`, `row_mut`, `as_view`, `sub`,
  `fill`, `into_row_bands`). `CellViewMut::from_slice(cells, width, height)` wraps
  caller-owned cells, such as `render_to_slice` output or C ABI buffers.

#### Borrowed frames and layouts

- `PixelLayout::new(width, height, format, alpha)` describes a borrowed frame:
  `PixelFormat::{Rgba8, Bgra8}`, `with_stride` for padded rows, and
  `with_row_order(RowOrder::{TopDown, BottomUp})`. Alpha has no default:
  - `AlphaMode::Straight`: channels independent of alpha; alpha 0 is no content.
  - `AlphaMode::Premultiplied`: channels are divided by alpha when read;
    channels above alpha clamp to alpha; alpha 0 reads as transparent.
  - `AlphaMode::Ignore`: the fourth byte is padding (BGRX/RGBX); every pixel
    is opaque.
- `PixelFrameView::from_layout(bytes, layout)` validates and borrows any
  layout; `new` remains the top-down straight-alpha constructor and
  `with_timestamp` attaches a media timestamp. `rgba(x, y)` reads any layout
  as straight, top-down RGBA; `row(y)` returns a logical row's raw bytes;
  `layout`, `row_order`, and `alpha_mode` describe the view.
- Encoders and `ImageSource` read every layout through the same rules, so a
  BGRA, bottom-up, premultiplied, or padded frame gives the same cells as the
  equivalent straight RGBA image.
- **Exact-size frames are zero-copy:** when `RasterSpec::matches(&frame)` is
  true, pass the borrowed frame straight to `CellEncoder::encode`. Nothing is
  copied or allocated. Otherwise resample it with `ImageSource`.
- Since M6, encoders and `blend_over` read borrowed rows through a decoder
  specialized for the frame's layout, chosen once per frame. A BGRA,
  bottom-up, premultiplied, padded frame costs 3–5% more to encode with the
  block encoders than TRend's own straight RGBA buffer. A sub-view
  (`PixelFrameView::sub`) of a larger framebuffer is read in place the same
  way. [readback-recipes.md](readback-recipes.md) maps common framebuffers
  (GPU readbacks, GDI, X11, Cairo, Skia, Qt, SDL, FFmpeg, CEF) to layouts,
  and `examples/external-renderer` shows the whole path.

#### Sub-views, regions, and row bands

- `PixelRect` and `CellRect` are distinct rectangle types with `intersection`.
  `RegionError::{Empty, OutOfBounds}` reports invalid regions.
- `PixelFrameView::sub(rect)` borrows a region of any layout.
  `PixelViewMut` is a writable, strided, straight top-down RGBA surface from
  `PixelBuffer::view_mut` or `PixelViewMut::new(bytes, width, height, stride)`,
  with `row_mut`, `as_view`, `sub`, `reborrow`, and `into_row_bands`.
  `CellViewMut::reborrow` likewise lets a view be passed by value (for example
  to `encode` or `render_region`) and used again.
- `RasterSpec::pixel_rect(cells)` maps a cell region to the pixels that encode
  it; `RasterSpec::covering_cells(pixels)` maps a damaged pixel rectangle
  outward to whole cells.
- Several encoders can write different regions of one `CellFrame`, presented
  by one `AnsiPresenter`. Row bands (`into_row_bands`) split work for
  sequential cancellation checks or host threads; TRend starts no threads.

```rust
// A wgpu/D3D12-style readback: BGRA, 256-byte rows, premultiplied.
let layout = PixelLayout::new(width, height, PixelFormat::Bgra8, AlphaMode::Premultiplied)
    .with_stride(padded_bytes_per_row);
let frame = PixelFrameView::from_layout(&mapped, layout)?;
if spec.matches(&frame) {
    encoder.encode(frame, cells.view_mut())?;
}

// Re-encode only what changed.
let dirty = spec.covering_cells(damage)?;
encoder.encode(frame.sub(spec.pixel_rect(dirty)?)?, cells.view_mut().sub(dirty)?)?;
```

`PixelFormat`, `AlphaMode`, `PixelLayout`, and the core error enums
(`FrameError`, `GeometryError`, `RenderError`, `EncodeError`, `RegionError`)
are `#[non_exhaustive]`, so later milestones can extend them without breaking
callers. M4 added `RenderError::Encode(EncodeError)`.

```rust
let spec = RasterSpec::new(viewport, Quadrant.grid(), CellAspect::default())?;
ImageSource::new(frame_view)
    .with_sampling(SamplingFilter::Area)
    .render_into(&spec, &mut pixels)?;
cells.resize(viewport.width, viewport.height)?;
Quadrant.encode(pixels.view(), cells.view_mut())?;
stdout.write_all(presenter.present(&cells))?;
```

`examples/procedural_source.rs` shows native and scaled application
rendering (`--encoder half|quadrant|density`), built on `Surface` since M4. `examples/render_benchmark.rs`
times the legacy renderer against the pipeline, and
`examples/workload_benchmark.rs` times realistic workloads stage by stage (see
[benchmarks.md](benchmarks.md)).

### Surface and input mapping (M4)

`Surface<E: CellEncoder>` owns one render target: the encoder, the
`RasterSpec`, a `PixelBuffer`, a `CellFrame`, and an `AnsiPresenter`, and keeps
them consistent. It is a convenience layer only: it holds no scene, never
composites, reads no input, starts no threads except in the opt-in
`*_parallel` methods, and writes only to the writer it's given. It produces exactly the cells and bytes of the equivalent manual
steps (tested).

```rust
let mut surface = Surface::new(HalfBlock, viewport, capabilities)?;
surface.set_output_options(OutputOptions::new().with_synchronized_updates(true));

// Native rendering: draw into the pixel buffer, then every cell is encoded.
surface.draw(|pixels, spec| app.render(pixels, spec))?;
surface.present(&mut stdout)?;

// An exact-size readback, in any layout, encoded in place.
surface.encode_from(readback_view)?;

// An image or video frame, fitted and resampled.
surface.encode_image(&ImageSource::new(frame).with_sampling(SamplingFilter::Area))?;

// Damage: re-encode and present only the cells covering changed pixels.
if let Some(cells) = surface.encode_region(damage)? {
    surface.present_region(cells, &mut stdout)?;
}
```

- Setup: `new(encoder, viewport, capabilities)`, `resize`, `set_encoder`,
  `set_cell_aspect`, `set_capabilities`, `set_output_options`, and
  `set_origin(row, column)` (1-based). Changing size, origin, or capabilities
  makes the next presentation redraw every cell.
- Accessors: `encoder`, `spec`, `viewport`, `cell_aspect`, `capabilities`,
  `output_options`, `origin`, `pixels`, `pixels_mut`, `cells`, `cells_mut`
  (for overlays; the next encode overwrites the cells it covers),
  `source_mapping`, and `last_stats`.
- Encoding:
  - `draw(|pixels, spec| …)` returns the closure's value. The buffer keeps its
    contents between frames.
  - `encode` and `encode_region(damage)` re-encode after `pixels_mut` edits.
    `encode_region` clips the damage and returns the covering `CellRect`.
  - `encode_from(frame)` accepts only frames of the spec's exact size and
    reads them in place. Any other size is `EncodeError::SizeMismatch`.
  - `encode_from_region(frame, damage)` (M5) re-encodes only the cells
    covering a dirty rectangle of such a frame, again in place.
  - `encode_image(&source)` and `encode_image_region(&source, damage)` fit
    and resample.
  - `encode_image_parallel(&source, threads)` and
    `encode_from_parallel(frame, threads)` (after M6) do the same as
    `encode_image` and `encode_from`, split into bands of cell rows on up to
    `threads` scoped threads. The cells, pixels, and mapping are identical
    (`tests/parallel.rs`). They need `E: Sync` (use
    `Box<dyn CellEncoder + Sync>` for run-time choice). Unlike the rest of
    `Surface` they allocate per call (thread stacks), which costs tens of
    microseconds; use them for large terminals, where a frame costs
    milliseconds. `threads <= 1` is the single-threaded method.

  Exact-size and fitted input are separate calls on purpose: when pixels
  aren't square (for example `Density`), a frame that happens to have the
  spec's size would otherwise be stretched instead of fitted.
- Presenting: `present(&mut writer)` and `present_region(cells, &mut writer)`
  return `io::Result<PresentStats>`. `present_bytes` and
  `present_region_bytes` return the bytes instead, and `invalidate` forces a
  full redraw. Output is positioned at the origin and restores default
  attributes, so the surface can share the screen with host text. It never
  clears anything outside its own cells.
- A warmed-up surface allocates nothing: drawing, every single-threaded
  encode path, and whole or regional presentation with any options.
- `Surface<Box<dyn CellEncoder>>` lets a host choose the encoder at run time,
  for example from `CellEncoder::requirements`. `CellEncoder` is implemented
  for `&T` and `Box<T>`.

Mapping between terminal cells, surface pixels, and source pixels:

- `RasterSpec::cell_at(x, y)` gives the cell `(column, row)` showing a pixel.
  `RasterSpec::cell_pixels(column, row)` gives the pixels a cell encodes.
- `Surface::cell_at_terminal(row, column)` maps a 1-based terminal position,
  as mouse reports give it, to a surface cell.
- `ImageSource::mapping(&spec)` returns a `SourceMapping`:
  - `to_source(x, y)` is the source pixel a surface pixel shows (the Nearest
    sample), or `None` in letterboxing.
  - `cell_to_source(column, row)` maps a cell through its center pixel.
  - `content()` is the surface rectangle the image covers.
  - `source_damage(rect)` gives the surface pixels a change to `rect` of the
    source can affect. It is conservative for every filter, fit, and
    transform, and is tested against full re-renders on 240,000 random
    geometries.

```rust
// Mouse click at a terminal position -> pixel of the video frame.
let pixel = surface
    .cell_at_terminal(mouse_row, mouse_column)
    .and_then(|(column, row)| surface.source_mapping()?.cell_to_source(column, row));

// Dirty rectangle reported by a decoder or browser -> cells to update.
if let Some(damage) = source.mapping(surface.spec())?.source_damage(dirty) {
    if let Some(cells) = surface.encode_image_region(&source, damage)? {
        surface.present_region(cells, &mut stdout)?;
    }
}
```

Terminal-session concerns stay outside the core: raw mode, the alternate
screen, size queries and resize signals, mouse reporting and parsing, probing
for mode 2026 support, and cell pixel size queries. They belong in a future
`trend-term` crate.

`examples/procedural_source.rs` uses two surfaces (the same bytes it produced
before M4) and accepts `--encoder half|quadrant|sextant|braille|density`. `examples/surface_demo.rs` shows capability-based encoder
choice, damage regions, output options, and click-to-pixel mapping.

### Composition and text overlays (M5)

Two small operations let an application put a HUD, labels, or sprites over
rendered or decoded frames. TRend still holds no layers or scene: the host
calls these in whatever order it wants.

- `compose::blend_over(source, &mut target, x, y)` draws a `PixelFrameView`
  of any layout over a `PixelViewMut` with straight-alpha source-over
  compositing, exactly rounded. The source is clipped at any offset.
  Opaque pixels copy and transparent ones are skipped. It returns the
  touched `PixelRect`, which is the damage to pass to
  `Surface::encode_region`.
- `text::put_str(&mut cells, column, row, text, style)` writes text into a
  `CellViewMut`, one character per cell, clipped at the view's edge, and
  returns the number of cells written. `TextStyle { foreground, background,
  attributes }` has `new`, `with_background`, and `with_attributes`.
  `TextBackground::{Keep, Default, Color(_)}` chooses what is behind each
  character; `Keep` leaves the image's cell background in place.
- Characters that would shift the rest of the row are written as `?`:
  controls, tabs, newlines, and characters known to be wide (CJK, most
  emoji) or zero width (combining marks, joiners). Lay out wide text in the
  host if you need it.
- Draw text into a panel through `CellViewMut::sub`, and clear the panel
  first with `CellViewMut::fill`.

```rust
surface.draw(|pixels, spec| app.render(pixels, spec))?;
if let Some(damage) = blend_over(sprite.view(), &mut surface.pixels_mut(), x, y) {
    surface.encode_region(damage)?;
}
let style = TextStyle::new(Color::rgb(255, 230, 120)).with_attributes(Cell::BOLD);
put_str(&mut surface.cells_mut(), 1, 0, "HP 97", style);
surface.present(&mut stdout)?;
```

All of these allocate nothing. `examples/hud_3d.rs` shows a software-rendered
3D scene with a blended radar sprite and a text panel. See
[readback-recipes.md](readback-recipes.md) for handing frames from wgpu,
OpenGL, and CEF to a surface.

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

[`include/trend.h`](../include/trend.h) defines a caller-buffer interface exported
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

`scripts/c-consumer-smoke.sh` (M6) builds the library and compiles
`tests/c/consumer_smoke.c` as strict C11 against the header. It checks the
struct sizes and field offsets at compile time, then calls every export and
checks results, error codes, and buffer-length rules at run time.

The v1 ABI exposes only the legacy half-block `Renderer`. A v2 surface over
`Surface` (encoders, layouts, regions, overlays, and presentation) is
designed in [ffi-v2-design.md](design/ffi-v2-design.md) but not implemented; v1
stays unchanged.

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

Terminal session handling (raw mode, the alternate screen, size and resize,
input, and capability probing) is planned as an optional `trend-term` crate
that the core never depends on; see
[trend-term-design.md](design/trend-term-design.md).

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
[architecture.md](architecture.md).

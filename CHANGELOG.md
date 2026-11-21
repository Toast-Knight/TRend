# Changelog

All notable changes to TRend are documented here.

## Unreleased

### Removed and changed before the first release (breaking)

- Core: removed the `experimental` module (`ExperimentalGlyphRenderer`, `ExperimentalGlyphSet`,
  `ExperimentalGlyphOptions`, `glyph_coverage`). The glyph matcher failed acceptance in real
  terminals and nothing used it; the quadrant, sextant and Braille encoders cover what it tried
  to do. Its `fidelity_baseline` example and `tools/fidelity-font-validation.ps1` went with it.
  The core is about 1,060 lines smaller.
- Core: removed `BoundedQueue` (the `queue` module). Nothing in the workspace used it, and
  queueing frames is the host's scheduling, which the core leaves to the host.
- `trend-3d`: the format-specific `parse_gltf`, `parse_obj`, `parse_obj_with`, `parse_ply` and
  `parse_stl` are no longer exported; `load`, `load_with`, `load_from_bytes` and
  `load_from_bytes_with` are the four ways in. `ModelFormat`, `EnvironmentPreset`, `RenderMode`,
  `Background`, `FrameStats` and `MeshStats` are `#[non_exhaustive]`, so later additions do not
  break programs.
- `trend-term`: `SessionOptions` is built with `SessionOptions::new()` and `with_mouse`,
  `with_title`, `with_key_releases`, `with_alternate_screen` and `with_hidden_cursor`, like the
  core's `OutputOptions`. It, `Event`, `Key`, `MouseAction`, `MouseButton`, `TerminalInfo` and
  `FrameTiming` are `#[non_exhaustive]`.
- Output is unchanged: every golden test passes without re-recording, and the C ABI is the same.

### Documentation

- The README is rewritten for people using the library: what each crate gives and what it
  depends on, the two ways through the core, the cell encoders, requirements and examples.
- Records of how the project got here moved to `docs/history/`, design proposals to
  `docs/design/`, and `docs/README.md` indexes everything. The architecture, usage,
  getting-started and API pages no longer describe removed or outdated behavior.
- The library crates carry `keywords` and `categories` for a later crates.io release;
  publishing stays disabled.

### Added

- The test viewer now runs on `trend-term` on every platform. Its own Win32 console code
  (`unsafe` declarations for the console and `msvcrt`) and its direct crossterm use are gone,
  about 170 lines; behavior and controls are the same, a resize redraws at once, Ctrl+C quits,
  and the status line no longer leaves stray characters when it wraps. The Windows build of the
  viewer now includes crossterm (about 46 KB larger, measured with the GNU toolchain).
- `trend-term`: `Resize` is reported once per change with the size the terminal gives when
  asked, and the size is also checked a few times a second, so a resize is seen on terminals
  that do not announce it (the Windows console's own event describes its scroll-back buffer).
  Colours are reset when the session ends. On Windows, `Session::enter` fails cleanly on a
  console without escape-sequence support. The `showcase` example turns with a mouse drag and
  zooms with the wheel.
- Added three optional crates under `adapters/`; the core is unchanged and depends on none of
  them:
  - `trend-term`: a terminal session (raw mode, the alternate screen, restore on exit and
    panic, size and resize, keyboard with held-key state, mouse, timed frame writes) on a
    crossterm backend, with the `starter` and `showcase` examples.
  - `trend-3d`: dependency-free software 3D: glTF/GLB, OBJ, STL and PLY loading, physically
    based materials, image-based lighting and sky boxes, fog, orbit and first-person cameras.
  - `trend-text`: dependency-free scaled text drawn as pixels: a built-in pixel font and a
    TrueType rasterizer.
- Added the M1 rendering pipeline alongside the existing renderer:
  `PixelBuffer`, `RasterSpec`/`SubcellGrid`/`CellAspect`, `ImageSource`, the
  `CellEncoder` trait with `HalfBlock` and `Quadrant` encoders, and
  `CellView`/`CellViewMut`. Existing APIs and output are unchanged.
- Added recorded golden tests for `Renderer` cells and `AnsiPresenter` bytes,
  a 2,880-case differential test proving the half-block pipeline matches
  `Renderer` cell for cell, and Quadrant geometry tests.
- Added the `procedural_source` and `render_benchmark` examples.
- M2a: borrowed-frame layouts (`PixelLayout`, `PixelFormat::Bgra8`,
  `RowOrder`, `AlphaMode`) read through `PixelFrameView::rgba`; zero-copy
  encoding of exact-size frames (`RasterSpec::matches`); strided and
  sub-rectangle views (`PixelViewMut`, `PixelFrameView::sub`,
  `CellViewMut::sub`, row bands); `PixelRect`, `CellRect`, `RegionError`, and
  region mapping on `RasterSpec`. Existing output is unchanged.
- `PixelFormat`, `FrameError`, `GeometryError`, `RenderError`, and
  `EncodeError` are now `#[non_exhaustive]`.
- M2b: `Renderer` is now a facade over the pipeline's geometry, sampling,
  dither, and half-block rule, streamed row by row with no pixel buffer; its
  cells, bytes, cancellation, and allocation behavior are unchanged.
  `calculate_transformed` delegates to `calculate_for_spec`. Added
  `ImageSource::render_region` (regions and row bands),
  `CellViewMut::from_slice`, and `RenderError::Region`.
- M3: `Density` encoder (`DensityRamp::{Ascii, Shades}`, optional color) and
  encoder metadata (`CellEncoder::requirements`, `EncoderRequirements`,
  `GlyphRepertoire`, `ColorUse`, `supported_by`). In monochrome mode, the
  presenter now writes colorless cells with their own glyph instead of a
  blank.
- M3: `dither::ordered_4x4_region` dithers regions and row bands with global
  coordinates. Added `PixelViewMut::reborrow` and `CellViewMut::reborrow`.
- M3 performance, with byte-identical output: ANSI serialization 3.5–4.6×
  faster; unchanged-frame diffing about 20% faster; Nearest sampling about 2×
  faster; exact Bilinear axis math without `i128` division.
- Added `examples/workload_benchmark.rs` and `docs/benchmarks.md`.
- M4: `Surface`, a convenience owner of encoder, spec, pixel buffer, cells,
  and presenter, with `draw`, `encode_from` (exact size, zero-copy),
  `encode_image`, damage-driven `encode_region`/`encode_image_region`, and
  `present`/`present_region` to any `io::Write`. It is allocation-free after
  warm-up and produces the same cells and bytes as the manual steps.
- M4: opt-in presenter output (`OutputOptions`: synchronized updates, combined
  color sequences), `PresentStats` via `last_stats`, `present_view_at`,
  `present_region_at`, and `invalidate`. Default output is byte-identical.
- M4: input and damage mapping: `RasterSpec::cell_at`/`cell_pixels`,
  `ImageSource::mapping` returning `SourceMapping` (`to_source`,
  `cell_to_source`, `content`, `source_damage`), and
  `Surface::cell_at_terminal`.
- M4: `CellEncoder` is implemented for `&T` and `Box<T>`; added
  `RenderError::Encode`. `procedural_source` now uses `Surface` (same
  output); added `examples/surface_demo.rs`.
- M5: `Quadrant` encodes 3–4× faster with identical cells (exact subset-sum
  fitting), and every block encoder reads straight RGBA rows directly.
- M5: `Sextant` (2×3, Unicode 13 sextant mosaics) and `Braille` (2×4)
  encoders on a shared fast two-color fitter, with
  `GlyphRepertoire::{Sextants, Braille}` and
  `SubcellGrid::{SEXTANT, BRAILLE}`.
- M5: composition and text overlays. `compose::blend_over` does exactly
  rounded straight-alpha compositing from any layout. `text::put_str` with
  `TextStyle`/`TextBackground` draws text into cells. `CellViewMut::fill`,
  `Surface::cells_mut`, and `Surface::encode_from_region` round them out.
- M5: the presenter writes `Cell::attributes` (`Cell::BOLD`, `DIM`, `ITALIC`,
  `UNDERLINE`, `BLINK`, `REVERSE`, `STRIKETHROUGH`), which it previously
  ignored; `PresentStats::attribute_sequences`. Output for cells without
  attributes is unchanged.
- M5: `OutputOptions::compact_indexed_colors` skips color sequences that
  select the palette entry already set. The screen is identical, and 256-
  and 16-color output shrinks 3–5× on smooth images. Opt-in; the default
  bytes are unchanged.
- M5: `examples/hud_3d.rs` (software 3D with a blended sprite and text
  panel), `docs/readback-recipes.md` (wgpu, OpenGL, CEF), and recorded
  encoder goldens (`tests/golden_encoders.rs`).
- M6 performance, with byte-identical output and no new allocations:
  Bilinear resampling computes the vertical axis once per row and blends
  opaque pixels directly (about 2.4× faster); encoders and `blend_over` read
  borrowed rows through per-layout decoders instead of per-pixel `rgba`
  (HalfBlock and Density encode about 5× faster; BGRA, bottom-up,
  premultiplied readbacks 25–61% faster per frame); unpremultiplying skips
  the division for opaque pixels. Video frames cost 33–67% less TRend time.
- M6: `compact_indexed_colors` in 16-color mode writes sequences from the
  palette entry it already found and remembers the last lookup, so compact
  output is now cheaper to present than the default in every measured
  workload (previously up to 42% more). Output is unchanged. It stays
  opt-in and is planned to become the default at the next
  output-compatibility boundary (see `docs/api.md`).
- M6: `examples/frame_budget.rs` measures whole frames against the 30-FPS
  budget per stage, with bytes, output rate, and allocations.
- M6: `examples/external-renderer`, a two-crate end-to-end example: a
  standalone software 3D renderer (`tiny-raster`, no TRend dependency)
  producing BGRA, bottom-up, premultiplied, 256-byte-aligned frames, and a
  host that maps its frame description to a `PixelLayout` and encodes each
  frame in place.
- M6: a strict C11 consumer of `include/trend.h`
  (`tests/c/consumer_smoke.c`, `scripts/c-consumer-smoke.sh`). The C ABI is
  unchanged.
- M6 documentation: `docs/ffi-v2-design.md` (a proposed C surface over
  `Surface`, not implemented), `docs/trend-term-design.md` (the core vs an
  optional terminal-session crate), a framebuffer layout table in
  `docs/readback-recipes.md`, and the 30-FPS budget, `opt-level` comparison,
  and compact-color results in `docs/benchmarks.md`.
- After M6 (driven by an external WebGL renderer test): Area and Bilinear resampling
  read whole source rows through the per-layout decoders and step their
  column positions without dividing, with identical pixels. At 400×120
  cells, resampling a 640×480 frame for Sextant went from 14.2 to 2.2 ms
  (Area) and from 6.7 to 2.2 ms (Bilinear).
- After M6: the Quadrant and Sextant/Braille fitters return single-color
  blocks immediately (same split as before), and color means use the tested
  exact reciprocal division: encoding is 25–35% faster on rendered scenes.
- After M6: `Surface::encode_image_parallel` and
  `Surface::encode_from_parallel` split resampling and encoding into bands
  of cell rows on scoped threads, with the same cells, pixels, and mapping
  as the single-threaded methods (`tests/parallel.rs`). They are opt-in and,
  unlike the rest of `Surface`, allocate per call.

## 0.1.0 - 2026-09-06

- Reorganized fixtures, examples, tools, scripts, documentation, and generated
  packages into conventional top-level directories.
- Added discoverable help and version output to the viewer and Python TUI bridge.
- Added a minimal first-render example and a positioned terminal overlay example.
- Documented the bridge protocol and added deterministic Windows/Linux packages.
- Recorded device validation for Android image, animation, and terminal rendering;
  Android video playback remains unimplemented.
# Phase 0 plan: from media renderer to general terminal renderer

- Status: **plan only; nothing here is implemented**
- Date: 2026-09-25
- Builds on: [architecture-audit.md](architecture-audit.md),
  [architecture-proposal.md](architecture-proposal.md)

## Goal and scope

TRend's job is to turn *any* application's visual output into terminal
graphics. The application owns rendering, time, input, and experience. TRend
owns the steps after pixels exist:

```text
Render Source ──► PixelBuffer ──► CellEncoder ──► CellView ──► Terminal Backend
(app-owned)       (TRend)         (TRend)         (TRend)       (TRend core: ANSI; others optional)
```

Phase 0 introduces the middle of that pipeline (`PixelBuffer`, `CellEncoder`,
`CellView`) **alongside** the existing code. Nothing existing changes
behavior. It has two parts:

- **0.1 Baseline:** tests and a benchmark only; no `src/` changes.
- **0.2 Milestone M1, "pipeline proof":** additive core types, two encoders,
  and tests that prove the new path reproduces today's output exactly.

### Changes from the earlier proposal

These keep Phase 0 smaller than [architecture-proposal.md](architecture-proposal.md)
suggested:

- **The `PixelBuffer` is the contract, not a `RenderSource` trait.** A source
  is anything that fills a `PixelBuffer` or can lend an RGBA view. A trait is
  only needed once a compositor calls sources generically, so it's deferred
  to M3 and may turn out to be unnecessary.
- **No new read-only pixel view type.** The existing `PixelFrameView` already
  is a validated, strided, borrowed RGBA8 view. It serves as the encoder's
  input. Renaming it is deferred.

## 1. Smallest useful `PixelBuffer`

An owned, tightly packed, straight-alpha RGBA8 image that can be reused
across frames.

```rust
// src/pixel_buffer.rs
pub struct PixelBuffer {
    pixels: Vec<u8>,   // len == width * height * 4
    width: u32,        // > 0
    height: u32,       // > 0
}

impl PixelBuffer {
    /// Transparent black (all zero). Rejects zero or overflowing dimensions.
    pub fn new(width: u32, height: u32) -> Result<Self, FrameError>;
    /// Reuses the allocation when shrinking; contents reset to transparent.
    pub fn resize(&mut self, width: u32, height: u32) -> Result<(), FrameError>;
    pub const fn width(&self) -> u32;
    pub const fn height(&self) -> u32;
    pub const fn stride(&self) -> usize;             // width * 4, always
    pub fn pixels(&self) -> &[u8];
    pub fn pixels_mut(&mut self) -> &mut [u8];
    pub fn row_mut(&mut self, y: u32) -> &mut [u8];  // panics if out of range, like slice indexing
    pub fn clear(&mut self, rgba: [u8; 4]);
    /// Borrowed read view for encoders and ImageSource. Infallible because
    /// invariants are upheld by construction. Timestamp is zero.
    pub fn view(&self) -> PixelFrameView<'_>;
    pub fn allocated_bytes(&self) -> usize;
}
```

Deliberately left out: other pixel formats, custom strides, sub-rectangle
views, mutable views, blending, drawing primitives, and size limits. Hosts
already choose the viewport, and decoders already enforce their own limits.
Every one of these can be added later without breaking this API.

## 2. Smallest useful `CellEncoder`

### Terminal pixel geometry is data, not code

```rust
// src/geometry.rs (additions)
/// Pixels resolved per terminal cell by an encoder.
pub struct SubcellGrid { pub columns: u8, pub rows: u8 }
impl SubcellGrid {
    pub const HALF_BLOCK: Self = Self { columns: 1, rows: 2 };
    pub const QUADRANT:   Self = Self { columns: 2, rows: 2 };
}

/// Physical width:height of one terminal cell. Supplied by the host
/// (configuration, or a probe in an optional crate later). Default 1:2.
pub struct CellAspect { pub width: u16, pub height: u16 }

/// Everything a source needs to know to produce pixels for an encoder.
pub struct RasterSpec {
    pub viewport: Viewport,        // cells
    pub grid: SubcellGrid,
    pub width: u32,                // viewport.width  * grid.columns
    pub height: u32,               // viewport.height * grid.rows
    pub pixel_aspect: PixelAspect, // (cell.width * grid.rows) : (cell.height * grid.columns), reduced
}
impl RasterSpec {
    pub fn new(viewport: Viewport, grid: SubcellGrid, cell: CellAspect) -> Result<Self, GeometryError>;
}
```

| Encoder | Grid | Pixel aspect on a 1:2 cell |
| --- | --- | --- |
| `HalfBlock` | 1 × 2 | 1:1 (today's implicit assumption) |
| `Quadrant` | 2 × 2 | 1:2 |
| later: `Sextant` / `Braille` / `Density` | 2 × 3 / 2 × 4 / 1 × 1 | 3:4 / 1:1 / 1:2 |

The core never assumes a grid or aspect. `RasterSpec` is the single handshake
between "what the terminal needs" and "what the source renders".

### The trait

```rust
// src/encode/mod.rs
pub trait CellEncoder {
    /// Pixels this encoder resolves per cell.
    fn grid(&self) -> SubcellGrid;

    /// Encode `pixels` into `cells`. Requires
    /// pixels.width  == cells.width()  * grid.columns and
    /// pixels.height == cells.height() * grid.rows.
    fn encode(&self, pixels: PixelFrameView<'_>, cells: CellViewMut<'_>) -> Result<(), EncodeError>;
}

pub enum EncodeError { SizeMismatch { expected: (u32, u32), actual: (u32, u32) } }

pub struct HalfBlock;   // exact port of today's per-cell logic
pub struct Quadrant;    // new; proves grids other than 1×2
```

`encode` takes `&self`: encoders are pure functions of their input pixels.
That keeps them deterministic and lets hosts later encode row bands in
parallel without locks.

**Pixel rules every encoder follows:** alpha 0 means "no content", which
becomes `Color::DEFAULT`. Any nonzero alpha is treated as visible straight
RGB, exactly as today.

**`Quadrant` rules:**

- The 4-bit mask (top-left, top-right, bottom-left, bottom-right) selects a
  glyph from `' ' ▘ ▝ ▀ ▖ ▌ ▞ ▛ ▗ ▚ ▐ ▜ ▄ ▙ ▟ █`.
- If all four pixels are transparent, the cell is `Cell::default()`.
- If some are transparent, the opaque pixels form the foreground (their mean
  color) and the background is `DEFAULT`.
- If all are opaque, try the 8 two-color partitions, pick the lowest squared
  RGB error (ties go to the lower mask), and use each side's mean color. A
  uniform cell becomes `' '` with an RGB background.

### The cell view the encoder writes into

```rust
// src/cell.rs (additions)
pub struct CellView<'a>    { cells: &'a [Cell],     width: u16, height: u16, stride: usize }
pub struct CellViewMut<'a> { cells: &'a mut [Cell], width: u16, height: u16, stride: usize }

impl CellFrame {
    pub fn view(&self) -> CellView<'_>;
    pub fn view_mut(&mut self) -> CellViewMut<'_>;
}
impl CellViewMut<'_> {
    pub const fn width(&self) -> u16;
    pub const fn height(&self) -> u16;
    pub fn row_mut(&mut self, y: u16) -> &mut [Cell];
    pub fn fill(&mut self, cell: Cell);
}
```

`stride` exists from day one, so regions (`sub(rect)`, added in M3) never
change the trait signature. In M1, the stride always equals the width.

## 3. Boundary between encoding and terminal output

| | Encoder side | Backend side |
| --- | --- | --- |
| **Decides** | glyph per cell (from its own glyph set), 24-bit fg/bg or `DEFAULT` | cursor placement/origin, diffing, color-depth reduction (truecolor/256/16/mono), SGR state, attribute reset, last-resort glyph fallback, byte output |
| **Knows** | pixels, its grid | `TerminalCapabilities`, previous frame |
| **Never knows** | escape codes, capabilities, position on screen | where pixels came from, which encoder ran |

**The contract type is `CellView`.** The existing `AnsiPresenter` already
fits the backend column. In M1 it keeps taking `&CellFrame`, and encoders
write into a `CellFrame` via `view_mut()`. A `present_view(CellView)` entry
point and a `CellBackend` trait come later.

Two existing leaks are recorded here, not fixed in Phase 0:

- `AnsiPresenter`'s monochrome mode reinterprets `▀`/`▄` into a density ramp.
  That's encoding work sitting in the backend. It will become a `Density`
  encoder in M2; the presenter keeps the old behavior for legacy callers.
- Choosing an encoder from capabilities (for example, Quadrant needs block
  elements) is **host policy**. Font support can't be detected reliably, so
  the core may offer a helper but never decides on its own.

The only terminal fact the encoding side needs is `CellAspect`, and the host
supplies it.

## 4. How today's image/video pipeline maps

```text
TODAY   decoder ─► PixelFrameView ─► Renderer (geometry + sample + dither + ▀▄) ─► CellFrame ─► AnsiPresenter
M1      decoder ─► PixelFrameView ─► ImageSource ─► PixelBuffer ─► [dither] ─► HalfBlock ─► CellFrame ─► AnsiPresenter
                                     └ same geometry.rs + scale.rs math, written as pixels ┘
```

- **Decoders and `trend-media`:** unchanged. `DecodedImage::view`,
  `VideoFrame::view`, animation frames, and `MediaFrame.view` already produce
  `PixelFrameView`.
- **`ImageSource`** (`src/raster.rs`) is the "resample an existing RGBA
  frame" source. It uses the same sampling functions and writes RGBA into a
  `PixelBuffer` sized by `RasterSpec`:

  ```rust
  pub struct ImageSource<'a> { /* PixelFrameView<'a>, FitMode, SamplingFilter,
                                  RenderTransform, Option<Color> alpha_background */ }
  impl<'a> ImageSource<'a> {
      pub fn new(pixels: PixelFrameView<'a>) -> Self;          // Contain, Nearest, identity, no matte
      pub fn with_fit(self, fit: FitMode) -> Self;
      pub fn with_sampling(self, filter: SamplingFilter) -> Self;
      pub fn with_transform(self, transform: RenderTransform) -> Self;
      pub fn with_alpha_background(self, matte: Option<Color>) -> Self;
      /// Resizes `target` to the spec and fills every pixel.
      pub fn render_into(&self, spec: &RasterSpec, target: &mut PixelBuffer) -> Result<(), RenderError>;
  }
  ```

- **Geometry becomes aspect-aware.** `RenderGeometry::calculate_for_spec`
  generalizes today's fit math:
  - containment compares `target_w × aspect_w × source_h` with
    `target_h × aspect_h × source_w`;
  - pan converts cells to pixels with `grid.columns` and `grid.rows` instead
    of `1` and `2`.

  With the half-block spec (aspect 1:1, grid 1×2), every multiply is by 1, so
  the integer results are identical. In M1, the existing
  `calculate_transformed` stays untouched and a test checks the two agree on
  a large input grid. Only in M2 does the old function delegate to the new
  one. If it delegated earlier, the new code would be tested against itself.
- **Alpha written by `ImageSource`:**
  - out of bounds: `[0,0,0,0]`;
  - with a matte: composited RGB, alpha 255;
  - without a matte: straight RGB with alpha `max(1, round(sum/weight))`
    whenever the accumulated alpha is nonzero, and 0 otherwise.

  The `max(1, …)` keeps equivalence (today, any nonzero coverage is visible)
  while keeping real alpha for future compositing.
- **Dither** becomes an in-place pass over the `PixelBuffer`. It skips alpha-0
  pixels, and its coordinates are pixel coordinates, the same virtual-pixel
  coordinates as today.
- **Cancellation** (`render_to_slice_cancellable`) stays on the old
  `Renderer` in Phase 0. M2 reproduces it by rasterizing and encoding in cell
  row bands.

## 5. How a 2D/3D renderer feeds TRend

Two modes, and neither needs anything from TRend beyond M1:

**Native mode (best quality and speed).** The app renders at the encoder's
exact resolution:

```rust
let spec = RasterSpec::new(viewport, encoder.grid(), cell_aspect)?;
frame.resize(spec.width, spec.height)?;                    // app-owned PixelBuffer
my_renderer.draw(frame.pixels_mut(), spec.width, spec.height, spec.pixel_aspect); // app code: 3D, 2D, particles…
cells.resize(viewport.width, viewport.height)?;
encoder.encode(frame.view(), cells.view_mut())?;
out.write_all(presenter.present_at(&cells, row, column))?;
```

`pixel_aspect` goes into the app's projection matrix, so circles stay round
with any encoder. Depth buffers, cameras, meshes, physics, and the game loop
stay in the app.

**Scaled mode (any existing framebuffer).** The app already has a
framebuffer at some resolution (a 320×240 game, a GPU readback with padded
rows, an emulator). It wraps it as a `PixelFrameView`, which already supports
stride, so there's no copy. Then:

```text
PixelFrameView ─► ImageSource (fit/filter) ─► PixelBuffer ─► encoder
```

BGRA or other formats are converted by the app for now. A `Bgra8` variant of
`PixelFormat` is a later, compatible addition.

The app also owns frame pacing. TRend's presenter diffing keeps ANSI output
small when little changes. Terminal throughput (bytes per frame) is usually
the real limit and should be a tracked metric.

## 6. How an HTML/web renderer feeds TRend

It's exactly the same as section 5. TRend sees pixels only. An HTML host
program (an example or a separate project, **never the core, and not a
`trend-*` crate at first**) would:

1. Drive any browser engine: headless Chromium via DevTools screencast or
   screenshots, WebKit or Servo offscreen, or a lightweight HTML renderer.
   WebGL and Three.js content comes along for free through the browser.
2. Set the page viewport from `RasterSpec`: native size, or a multiple of it
   for downsampling with `SamplingFilter::Area`.
3. Receive frames as raw RGBA (wrap as `PixelFrameView`) or as PNG/JPEG
   (decode with `trend-image`), then use `ImageSource` and an encoder.
4. Map terminal mouse and keyboard to the page itself. A generic
   cell-to-source-pixel inverse of `RenderGeometry` is a sensible later
   helper for *any* interactive source. It isn't HTML-specific.

One honest limit: body text rendered as pixels at subcell resolution is
unreadable. Hybrid approaches, such as extracting text and writing it as real
cells over the image, belong to the HTML host. TRend's later cell-level text
and overlay support enables that without learning anything about HTML.

## 7. What must stay compatible during migration

Phase 0 rule: **no public item is removed, renamed, or changes signature or
output.** Everything is additive.

| Surface | Requirement |
| --- | --- |
| `Renderer` (all five `render_*` methods), `RenderRequest`, `RenderOptions`, `RenderStatus`, `RenderError` | Unchanged code and output |
| `PixelFrameView`, `PixelFormat`, `FrameError` | Unchanged; one crate-private unchecked constructor added |
| `Cell`, `Color` | 16-byte `#[repr(C)]` layout unchanged (`trend_cell_size` returns 16) |
| `CellFrame` | Existing methods unchanged; `view`/`view_mut` added |
| `AnsiPresenter`, `TerminalCapabilities`, `TerminalColorMode`, `TerminalEnvironment`, `PositionedRender*` | Unchanged bytes for every mode |
| `Viewport`, `FitMode`, `Alignment`, `RenderTransform`, `RenderGeometry::{calculate, calculate_transformed}`, `SamplingFilter` | Unchanged results |
| `timing`, `cache`, `queue`, `resize`, `experimental` modules | Unchanged |
| C ABI v1 and `include/trend.h` | Byte-identical header, identical behavior |
| All adapter crates, tools, bridge protocol, Python examples, packaging scripts | Untouched |

These stay compatible through M2 as well. In M2, the old `Renderer` becomes a
facade over the new path, and the Phase 0.1 golden tests are the gate for
that.

## 8. What not to implement yet

- A `RenderSource` trait, `Compositor`, layers, cell blending,
  `Color::TRANSPARENT`, text (`put_str`), styles, or attributes
- `CellViewMut::sub` or other regions, and `PixelBuffer` sub-views or
  mutable views
- Pixel formats other than RGBA8, premultiplied alpha, or size limits on
  `PixelBuffer`
- `Sextant`, `Braille`, or `Density` encoders; moving `experimental.rs`;
  glyph capability metadata
- Any change to `Renderer` internals, `AnsiPresenter`, `ffi.rs`, adapters, or
  tools
- A `CellBackend` trait or splitting out `DiffState`
- Renames or deprecations (`PixelFrameView` → `PixelView`,
  `CellFrame` → `CellBuffer`)
- `trend-term`, `trend-draw`, FFI v2, or graphics protocols (sixel, kitty)
- SIMD, threading, or fused fast paths (only measured in Phase 0)
- Any HTML, GPU, 3D, or game code in the workspace. The only new source code
  is a procedural example.
- New dependencies of any kind, including dev-dependencies of the core

## 9. Milestone M1: "pipeline proof"

**Claim to prove:** the new pipeline reproduces today's output exactly, and
supports a second, differently shaped encoder and an arbitrary pixel source
with no hard-coded terminal geometry.

**Done when:**

1. `tests/pipeline_equivalence.rs` passes. `ImageSource` + dither +
   `HalfBlock` gives **identical `CellFrame`s** to `Renderer` across a matrix
   of about 2,900 cases:
   - 5 generated patterns: opaque gradient, alpha checker with 0/partial/255
     alpha, 1×4 tall, 300×7 wide, and seeded noise with random alpha;
   - 4 viewports: 1×1, 3×2, 17×9, 80×24;
   - 3 fits × 3 filters;
   - 4 transforms: identity; zoom 250% with pan and start/end alignment;
     zoom 50%; cover with pan;
   - dither 0/16;
   - matte none/some.
2. A geometry agreement test passes: `calculate_for_spec(half-block, 1:2)`
   equals `calculate_transformed` over a dense grid of sizes, fits, and
   transforms.
3. `tests/quadrant_encoder.rs` passes:
   - glyph table, transparency, and partition choice on hand-built 2×2 cells;
   - an aspect test: a 100×100 source, contained in a 40×20-cell viewport
     with Quadrant on 1:2 cells, samples 80×40 pixels (physically square),
     while HalfBlock samples 40×40.
4. `cargo run --example procedural_source -- --encoder quadrant` animates a
   plasma in both native mode (rendered at `RasterSpec`) and scaled mode (a
   fixed 320×180 buffer through `ImageSource`), using only public APIs.
5. Every existing test passes unchanged, including the Phase 0.1 goldens.
   `fmt`, `clippy -D warnings`, and `cargo tree -p trend` still show zero
   dependencies. `include/trend.h` has no diff.
6. The benchmark reports old versus new half-block cost per filter. This is
   informational in M1 and becomes a ≤10% regression gate in M2.

Estimated size: about 600 lines of source and about 500 lines of tests.

## 10. Exact files

### Phase 0.1: baseline (tests and benchmark only)

| File | Change |
| --- | --- |
| `tests/common/mod.rs` | **New.** Deterministic pattern generators (shared with M1) and FNV-1a hashing of cells and bytes |
| `tests/golden_render.rs` | **New.** Recorded hashes of current `Renderer` output over the matrix above |
| `tests/golden_ansi.rs` | **New.** Recorded hashes of `AnsiPresenter` bytes across 4 color modes × Unicode on/off × `present`/`present_at` × incremental frames |
| `examples/render_benchmark.rs` | **New.** Dependency-free timing (`Instant`) of `Renderer` per filter and viewport, plus ANSI bytes per frame |

### Phase 0.2: milestone M1

| File | Change |
| --- | --- |
| `src/pixel_buffer.rs` | **New.** `PixelBuffer` |
| `src/raster.rs` | **New.** `ImageSource` |
| `src/encode/mod.rs` | **New.** `CellEncoder`, `EncodeError`, re-exports |
| `src/encode/half_block.rs` | **New.** `HalfBlock` + unit tests |
| `src/encode/quadrant.rs` | **New.** `Quadrant` + unit tests |
| `src/geometry.rs` | **Add** `SubcellGrid`, `CellAspect`, `PixelAspect`, `RasterSpec`, `RenderGeometry::calculate_for_spec`, agreement test. Existing functions untouched |
| `src/cell.rs` | **Add** `CellView`, `CellViewMut`, `CellFrame::{view, view_mut}` |
| `src/framebuffer.rs` | **Add** a `pub(crate)` unchecked constructor for `PixelBuffer::view` |
| `src/scale.rs` | **Add** `pub(crate) fn sample_rgba` (alpha-preserving output built on the existing `AlphaSample`). Existing functions untouched |
| `src/dither.rs` | **Add** an in-place ordered dither over `PixelBuffer` |
| `src/lib.rs` | **Add** `mod encode; mod pixel_buffer; mod raster;` and re-exports |
| `tests/pipeline_equivalence.rs` | **New.** Differential test (criterion 1) |
| `tests/quadrant_encoder.rs` | **New.** Encoder and aspect tests (criterion 3) |
| `examples/procedural_source.rs` | **New.** Native and scaled procedural source (criterion 4) |
| `examples/render_benchmark.rs` | **Extend** with new-pipeline timings |
| `docs/api.md` | **Add** a "Pixel buffers and cell encoders (preview)" section |
| `CHANGELOG.md` | **Add** an entry under Unreleased |
| `docs/development-status.md` | **Add** a dated event and the M2 next step |

**Explicitly untouched:** `Cargo.toml`, `Cargo.lock`, `src/renderer.rs`,
`src/terminal.rs`, `src/color.rs`, `src/ffi.rs`, `include/trend.h`,
`src/experimental.rs`, `src/timing.rs`, `src/cache.rs`, `src/queue.rs`,
`src/resize.rs`, everything under `adapters/`, `tools/`, `scripts/`,
`packaging/`, and the Python examples.

## After M1 (roadmap only)

> Superseded by [architecture-roadmap.md](architecture-roadmap.md), which
> revises this order based on M1's measurements.

| Milestone | Content |
| --- | --- |
| M2: switch-over | `Renderer` becomes a facade over the new path, with cancellation via row bands; `calculate_transformed` delegates; `Density` encoder; `AnsiPresenter::present_view(CellView)`; golden tests plus the benchmark are the gate |
| M3: composition | `CellViewMut::sub`, `Color::TRANSPARENT`, cell blit and text, `RenderSource` trait and `Compositor` only if generic sources prove necessary |
| M4: adapters | Decoders emit `PixelBuffer`; `VideoBackend` trait; merged animation decoder; tools route through `trend-media` |
| M5: encoders | `Sextant`, `Braille`; `GlyphMatch` from `experimental.rs` |
| M6: edges | `CellBackend` trait, `trend-term`, FFI v2, out-of-tree software-3D and HTML host examples |

The Android MP4/H.264 backend in [development-status.md](development-status.md)
doesn't depend on any of this and can be scheduled before or after M1.

## Decisions needed before M1 starts

1. **Priority.** M1 before or after the Android video milestone? They don't
   conflict.
2. **Second encoder.** `Quadrant` (recommended: it proves an x-subdivided
   grid and the aspect math) or the simpler `Density`.
3. **Golden storage.** Inline hash constants (recommended: no fixture
   churn) or checked-in snapshot files.

# Architecture proposal: general-purpose terminal rendering engine

- Status: **proposal only; nothing here is implemented**
- Date: 2026-09-25
- Baseline: [architecture-audit.md](architecture-audit.md)

## 1. Summary

TRend becomes a four-stage pipeline with **three extension points**. The core
stays dependency-free. Image and video playback become one kind of *source*
among many. 2D drawing and 3D scenes are also sources: they render into an
owned pixel buffer, and TRend turns that buffer into cells.

```text
          extension point                                   extension point              extension point
          ───────────────                                   ───────────────              ───────────────
host ──► RenderSource ──render──► PixelBuffer ──encode──► CellEncoder ──► CellViewMut ──► CellBackend
         (image, video,           (owned RGBA8,           (half-block,     (region of      (ANSI presenter,
          2D, 3D, procedural)      sized to the            quadrant,        the screen      ratatui, test
                                   encoder's grid)         sextant, …)      CellBuffer)     snapshot, …)
          │                        └──── RASTER ────┘      └── ENCODE ──┘   └─ COMPOSE ─┘   └─ PRESENT ─┘
          │
          └─ text, borders, UI chrome write cells directly into a CellViewMut (no trait needed)
```

Each stage owns exactly one conversion:

| Stage | Input | Output | Owns |
| --- | --- | --- | --- |
| Raster | a `RenderSource` | pixels in a `PixelViewMut` | sampling, fit, transform, 2D/3D drawing |
| Encode | a `PixelView` at subcell resolution | cells in a `CellViewMut` | glyph choice, per-cell color choice |
| Compose | cells from many layers | one screen `CellBuffer` | regions, z-order, clipping, cell transparency |
| Present | a screen `CellBuffer` | bytes or backend calls | diffing, color quantization, escape codes |

## 2. Design principles

1. **Two coordinate spaces, never mixed.** Pixel space (subcell resolution)
   and cell space. Each has its own rect type (`PixelRect`, `CellRect`).
   Only the encoder converts between them.
2. **Immediate mode.** The host calls `draw` each frame in the order it wants.
   There is no scene graph, no retained layer tree, and no hidden thread,
   event loop, or clock. This matches today's ownership rules.
3. **Owned buffers, borrowed views.** `PixelBuffer` and `CellBuffer` own
   memory and are reused frame to frame. `PixelView[Mut]` and `CellView[Mut]`
   are strided sub-rectangles, so regions, layers, and row-parallel work need
   no copies.
4. **Traits only at extension points:** `RenderSource`, `CellEncoder`,
   `CellBackend`. Everything else is concrete.
5. **The legacy API survives as a facade** built on the new pipeline, and
   byte-identical output is a hard gate before any old code is removed.
6. **The core never learns about codecs, files, terminals' raw modes, 3D
   math, or threads.**

## 3. Non-goals

TRend will not provide a scene graph, ECS, meshes, materials, cameras,
lighting, shaders, asset loading, physics, input handling, an event loop,
widgets or layout, audio, or networking. A 3D renderer, game engine, or UI
framework *integrates with* TRend by implementing `RenderSource` or by
writing cells. It never lives inside the core.

## 4. Subcell grids and aspect ratio

This is the central generalization. Every encoder declares how many pixels it
resolves per cell. The raster stage renders at exactly that resolution.

| Encoder | Subcells (x × y) | Colors per cell | Glyphs | Subpixel aspect on a 1:2 cell |
| --- | --- | --- | --- | --- |
| `HalfBlock` (today) | 1 × 2 | 2 | `▀ ▄` | 1.0 (square) |
| `Quadrant` | 2 × 2 | 2 | U+2596–U+259F | 0.5 |
| `Sextant` | 2 × 3 | 2 | U+1FB00–U+1FB3B (Unicode 13) | 0.75 |
| `Braille` | 2 × 4 | fg + bg | U+2800–U+28FF | 1.0 |
| `Density` | 1 × 1 | 1 + bg | ASCII or `░▒▓█` ramp | 0.5 |
| `GlyphMatch` (experimental) | 8 × 8 work grid | 2 | configured set | 0.5 |

`subpixel aspect = (cell_width / sx) / (cell_height / sy)`, where the physical
cell ratio `CellAspect` defaults to 1:2 and can be overridden by the host (or
measured later by `trend-term`). Today's geometry silently assumes the
half-block value of 1.0. Once the aspect is explicit:

- image fitting stays correct for every encoder, not only half-blocks;
- 3D projection gets the right aspect without guessing.

Aspect is kept as an integer ratio so geometry stays deterministic and
integer-only, like today.

## 5. Crates and modules

```text
trend/                      zero dependencies (std only)
  src/
    lib.rs                  re-exports + prelude
    geom.rs                 CellPoint/CellSize/CellRect, PixelPoint/PixelSize/PixelRect, CellAspect
    surface/                PixelFormat, PixelView, PixelViewMut, PixelBuffer, SurfaceLimits,
                            blend ops (fill, blit, over), ordered dither
    raster/                 RenderSource, RasterContext, RenderStatus,
                            ImageSource (fit + transform + sampling; today's geometry.rs + scale.rs),
                            SolidSource
    encode/                 CellEncoder, Subcells, GlyphRequirements, Encoding (enum dispatcher),
                            HalfBlock, Quadrant, Sextant, Braille, Density,
                            experimental::GlyphMatch (today's experimental.rs)
    cell/                   Color, Attrs, Style, Cell, CellBuffer, CellView, CellViewMut,
                            fill / blit / put_str
    compose.rs              Compositor (scratch owner), DrawOptions, CellBlend
    present/                DiffState, Damage, CellBackend, AnsiWriter, AnsiPresenter,
                            TerminalCapabilities (+ env detection), palette quantization
    time.rs                 PlaybackClock, AnimationTimeline, FrameDecision (unchanged)
    util/                   BoundedQueue, ResizeCoalescer, CellFrameCache (unchanged)
    compat.rs               Renderer, RenderRequest, RenderOptions, PixelFrameView, CellFrame
                            (legacy facade on the new pipeline)
    ffi.rs                  C ABI v1 (unchanged) + v2 (handles)

adapters/image              decodes into trend::PixelBuffer
adapters/animation          one AnimationDecoder (GIF, WebP) yielding PixelView + timing
adapters/video              internal `VideoBackend` trait; Windows / Linux / Android backends
adapters/media              detection + MediaSource; MediaSource implements RenderSource

NEW, optional:
  trend-draw                zero-dep 2D primitives into PixelViewMut (lines, rects, circles,
                            polygon fill, textured blit, bitmap-font text)
  trend-term                terminal session: raw mode, alt screen, size, resize, cell pixel
                            size, capability probing (replaces the tools' Win32/crossterm code)
  trend-ratatui (later)     CellBuffer ↔ ratatui Buffer backend/widget

examples/
  software-3d               hand-rolled rotating cube as a RenderSource (proves the contract;
                            not a library)
  layers                    video + text overlay + status bar in one screen
```

## 6. Core API shape

These are sketches that show responsibilities and ownership. Names and exact
signatures are open to change.

### 6.1 Surface (pixel space)

```rust
pub enum PixelFormat { Rgba8 }                     // straight alpha at API boundaries

pub struct SurfaceLimits { pub max_width: u32, pub max_height: u32,
                           pub max_pixels: u64, pub max_bytes: usize }

pub struct PixelView<'a>    { /* &[u8], size, stride, format — today's PixelFrameView minus timestamp */ }
pub struct PixelViewMut<'a> { /* &mut [u8], size, stride, format */ }

impl<'a> PixelView<'a> {
    pub fn new(pixels: &'a [u8], size: PixelSize, stride: usize, format: PixelFormat)
        -> Result<Self, SurfaceError>;
    pub fn sub(&self, rect: PixelRect) -> PixelView<'a>;              // clipped; may be empty
    pub fn row(&self, y: u32) -> &'a [u8];
}

impl PixelViewMut<'_> {
    pub fn sub(&mut self, rect: PixelRect) -> PixelViewMut<'_>;       // clipped; may be empty
    pub fn row_mut(&mut self, y: u32) -> &mut [u8];
    pub fn fill(&mut self, rgba: [u8; 4]);
    pub fn blit_over(&mut self, src: PixelView<'_>, at: PixelPoint);   // alpha "over"
}

pub struct PixelBuffer { /* Vec<u8>, size, stride, format; capacity is reused */ }
impl PixelBuffer {
    pub fn new() -> Self;
    pub fn resize(&mut self, size: PixelSize, limits: &SurfaceLimits) -> Result<(), SurfaceError>;
    pub fn view(&self) -> PixelView<'_>;
    pub fn view_mut(&mut self) -> PixelViewMut<'_>;
}
```

Decoders (image frames, video frames, animation frames) all produce
`PixelBuffer`. That removes three separate owned-pixel types, and
`SurfaceLimits` replaces the size parts of the three `DecodeLimits` structs.

### 6.2 Raster (sources)

```rust
pub struct RasterContext<'a> {
    pub cell_aspect: CellAspect,          // physical cell w:h, default 1:2
    pub subcells: Subcells,               // grid of the active encoder
    pub cancel: &'a dyn Fn() -> bool,     // today's cancellation, generalized
}
impl RasterContext<'_> { pub fn pixel_aspect(&self) -> (u32, u32); }

pub enum RenderStatus { Complete, Cancelled }

pub trait RenderSource {
    /// Natural pixel size, if the source has one (images, video). Procedural,
    /// 2D, and 3D sources return None and render at whatever size they're given.
    fn natural_size(&self) -> Option<PixelSize> { None }

    /// Fully overwrite `target`. Write alpha 0 where the source has no content.
    fn render(&mut self, target: PixelViewMut<'_>, ctx: &RasterContext<'_>)
        -> Result<RenderStatus, SourceError>;
}

/// Today's image path: fit + transform + sampling filter + optional matte.
pub struct ImageSource<'a> { /* PixelView<'a>, FitMode, RenderTransform, SamplingFilter, Option<matte> */ }
impl<'a> ImageSource<'a> {
    pub fn new(pixels: PixelView<'a>) -> Self;
    pub fn fit(self, fit: FitMode) -> Self;
    pub fn filter(self, filter: SamplingFilter) -> Self;
    pub fn transform(self, transform: RenderTransform) -> Self;
    pub fn matte(self, rgb: Option<[u8; 3]>) -> Self;
}
impl RenderSource for ImageSource<'_> { /* existing scale.rs math, aspect-aware geometry */ }
```

`render` takes `&mut self` so stateful sources (a video decoder, a 3D scene
with a depth buffer) need no interior mutability. `SourceError` wraps
`Box<dyn Error + Send + Sync>`, so adapter errors pass through without the
core knowing about them.

### 6.3 Encode (pixels to cells)

```rust
pub struct Subcells { pub x: u8, pub y: u8 }
pub struct GlyphRequirements { pub block_elements: bool, pub legacy_computing: bool, pub braille: bool }

pub trait CellEncoder {
    fn subcells(&self) -> Subcells;
    fn requirements(&self) -> GlyphRequirements;
    /// `pixels.size() == cells.size() * subcells`.
    fn encode(&self, pixels: PixelView<'_>, cells: CellViewMut<'_>) -> Result<(), EncodeError>;
}

pub struct HalfBlock;                                  // exact port of today's renderer output
pub struct Quadrant;                                   // best 2-color partition per cell
pub struct Sextant;
pub struct Braille { pub threshold: Threshold }        // fg dots over bg
pub struct Density { pub ramp: Ramp }                  // replaces the presenter's monochrome remap

pub enum Encoding { HalfBlock, Quadrant, Sextant, Braille, Density }  // config/FFI-friendly
impl CellEncoder for Encoding { /* match dispatch */ }
impl Encoding { pub fn best_for(caps: &TerminalCapabilities) -> Self; }
```

Rules shared by all encoders: a subpixel with alpha 0 is "no content". If a
cell's background partition is empty, its background color is
`Color::TRANSPARENT`. Choosing two colors per cell is an encoder detail:
exhaustive partitions for quadrant (8) and sextant (32), and a luma threshold
for braille.

### 6.4 Cells

```rust
#[repr(C)] pub struct Color { pub red: u8, pub green: u8, pub blue: u8, pub kind: u8 }
// kind: 0 = terminal default (as today), 1 = RGB (as today), 2 = indexed (red = index),
//       3 = transparent (compositing only; presented as default)

#[repr(C)] pub struct Cell { pub glyph: u32, pub foreground: Color, pub background: Color,
                             pub attributes: Attrs, flags: u8, reserved: [u8; 2] }
// Stays 16 bytes. Attrs: bold, dim, italic, underline, reverse, strike.
// flags: WIDE, WIDE_CONTINUATION (for CJK/emoji text overlays later).

pub struct CellBuffer { /* Vec<Cell>, CellSize, generation — today's CellFrame */ }
pub struct CellView<'a>;  pub struct CellViewMut<'a>;

impl CellViewMut<'_> {
    pub fn sub(&mut self, rect: CellRect) -> CellViewMut<'_>;          // clipped; may be empty
    pub fn reborrow(&mut self) -> CellViewMut<'_>;
    pub fn split_rows(&mut self, at: u16) -> (CellViewMut<'_>, CellViewMut<'_>);
    pub fn fill(&mut self, cell: Cell);
    pub fn blit(&mut self, src: CellView<'_>, at: CellPoint, blend: CellBlend);
    pub fn put_str(&mut self, at: CellPoint, text: &str, style: Style) -> u16;  // narrow chars first
}
```

The `Cell` layout stays FFI-compatible: today's `attributes` byte keeps its
position, and the reserved bytes were always zero.

### 6.5 Compose

```rust
pub enum CellBlend { Replace, Over }   // Over: transparent colors take the underlying cell's

pub struct DrawOptions<'a> {
    pub blend: CellBlend,
    pub cell_aspect: CellAspect,
    pub dither_strength: u8,
    pub cancel: Option<&'a dyn Fn() -> bool>,
    pub limits: SurfaceLimits,
}

/// Owns reusable scratch buffers. Not a scene: it keeps no layers between calls.
pub struct Compositor { pixels: PixelBuffer, cells: CellBuffer }

impl Compositor {
    /// source ─► scratch PixelBuffer (target size × subcells) ─► encoder ─► target region
    pub fn draw(&mut self, target: CellViewMut<'_>, source: &mut dyn RenderSource,
                encoder: &dyn CellEncoder, opts: &DrawOptions<'_>) -> Result<RenderStatus, ComposeError>;

    /// Several sources blended at pixel level (correct subcell alpha), encoded once.
    pub fn draw_stack(&mut self, target: CellViewMut<'_>, sources: &mut [&mut dyn RenderSource],
                      encoder: &dyn CellEncoder, opts: &DrawOptions<'_>) -> Result<RenderStatus, ComposeError>;

    /// Pixel size a source will be asked for, e.g. to size a GPU render target in advance.
    pub fn pixel_size_for(cells: CellSize, encoder: &dyn CellEncoder) -> PixelSize;
}
```

There's no `CellSource` trait. Text, borders, and UI chrome are ordinary
writes into a `CellViewMut`, so any widget library can take part without
depending on a TRend trait.

### 6.6 Present

```rust
pub struct DiffState { /* previous CellBuffer + origin, as in AnsiPresenter today */ }
pub struct Damage { /* runs of changed cells: row, column, start index, length */ }
impl DiffState {
    pub fn diff(&mut self, current: CellView<'_>, origin: CellPoint, out: &mut Damage);
    pub fn invalidate(&mut self);             // after foreign output touched the terminal
}

pub trait CellBackend {
    type Error;
    fn present(&mut self, frame: CellView<'_>, origin: CellPoint) -> Result<(), Self::Error>;
}

pub struct AnsiWriter { /* capabilities, SGR state, palette quantization, glyph fallback */ }
pub struct AnsiPresenter { diff: DiffState, writer: AnsiWriter, bytes: Vec<u8> }
impl AnsiPresenter {
    pub fn present(&mut self, frame: CellView<'_>) -> &[u8];                         // full screen
    pub fn present_at(&mut self, frame: CellView<'_>, origin: CellPoint) -> &[u8];   // positioned
}
pub struct AnsiBackend<W: std::io::Write> { presenter: AnsiPresenter, out: W }  // impl CellBackend
```

Other backends live outside the core and consume either `CellView` or
`Damage`: ratatui, crossterm queues, the Windows console API, HTML/xterm.js,
and snapshot tests. Pixel graphics protocols (sixel, kitty, iTerm2) aren't
cell encodings. If they're added later, they belong in a separate crate that
reserves a cell region and draws a `PixelView` into it, with no changes to
the core.

## 7. Data flow examples

### 7.1 Today's image preview on the new API

```rust
let image = trend_image::decode_png(&bytes, limits)?;               // PixelBuffer
let mut screen = CellBuffer::new(CellSize::new(80, 24));
let mut compositor = Compositor::default();
let mut presenter = AnsiPresenter::new(caps);

let mut source = ImageSource::new(image.view()).fit(FitMode::Contain).filter(SamplingFilter::Area);
compositor.draw(screen.view_mut(), &mut source, &HalfBlock, &DrawOptions::default())?;
stdout.write_all(presenter.present(screen.view()))?;
```

### 7.2 Video panel, text overlay, and status bar in one screen

```rust
screen.clear();
let mut root = screen.view_mut();
let (mut body, mut status) = root.split_rows(screen_height - 1);

if let Some(frame) = media.frame_at(clock.media_time(now))? {          // MediaFrame { view, timestamps }
    let mut panel = body.sub(video_rect);
    compositor.draw(panel.reborrow(), &mut ImageSource::new(frame.view), &Sextant, &opts)?;
    panel.put_str(CellPoint::new(1, 0), " LIVE ", Style::new().bg(Color::rgb(180, 0, 0)));
}
status.put_str(CellPoint::ORIGIN, &format!("{elapsed} / {total}"), Style::dim());
stdout.write_all(presenter.present_at(screen.view(), origin))?;
```

### 7.3 3D from an external or hand-written rasterizer

```rust
struct CubeScene { angle: f32, depth: Vec<f32> }        // lives in the app or an example crate

impl RenderSource for CubeScene {
    fn render(&mut self, mut target: PixelViewMut<'_>, ctx: &RasterContext<'_>)
        -> Result<RenderStatus, SourceError> {
        let (aspect_w, aspect_h) = ctx.pixel_aspect();  // correct projection for this encoder
        // resize depth buffer, project, rasterize triangles, write RGBA rows into `target`
        Ok(RenderStatus::Complete)
    }
}
compositor.draw(screen.view_mut(), &mut scene, &Braille::default(), &opts)?;
```

A GPU renderer (for example wgpu with a render-to-texture readback) asks
`Compositor::pixel_size_for` for its target size, renders, maps the texture,
and passes it through `ImageSource` at native size. Depth buffers, cameras,
and meshes stay in the application.

## 8. Transparency and compositing rules

- **Pixel level (exact).** Straight-alpha RGBA8 at API boundaries. `blit_over`
  and `draw_stack` use the standard "over" operator. Use this path when
  overlapping sources need correct subcell blending, such as a sprite over a
  3D scene.
- **Cell level (approximate, by design).** Encoded layers and text are blitted
  with `CellBlend::Over`. A `TRANSPARENT` foreground or background takes the
  color of the cell underneath. Subcell partial coverage is only
  approximated at this level. For exact blending, use `draw_stack`.
- **Presentation.** Any remaining `TRANSPARENT` is emitted as the terminal
  default color, which matches today's zero-alpha behavior.

## 9. Adapters in the new model

- `trend-image`, `trend-animation`, and `trend-video` decode into
  `PixelBuffer` and embed `SurfaceLimits` in their limits. They keep
  format-specific extras such as maximum input bytes and buffered frames.
- `trend-video` gets an internal `VideoBackend` trait. Each platform backend
  implements it, and the public `VideoDecoder` wraps `Box<dyn VideoBackend>`.
  This gives the planned Android backend a compile-time checked contract.
- `trend-animation` merges its three types into one `AnimationDecoder` with a
  per-codec frame provider.
- `trend-media` replaces its large `match` with an internal `FrameProvider`
  trait. `MediaSource` implements `RenderSource` by rendering its current
  frame through `ImageSource`, so media is just another source.
- Timestamps move from `PixelFrameView` to `MediaFrame` and `VideoFrame`.
  Pixel views carry no time.
- `test-viewer` and `ui-lab` go through `trend-media` only. Their terminal
  setup code moves to `trend-term`.

## 10. C ABI

- **v1 stays** (`trend_render_rgba8`, `trend_render_rgba8_ex`) and is
  implemented on the compatibility facade with identical output.
- **v2 (later)** adds opaque handles, for example `TrendScreen`,
  `TrendCompositor`, and `TrendPresenter`, with calls such as
  `trend_screen_draw_rgba8(screen, rect, pixels, …, encoding, options)`,
  `trend_screen_put_utf8(…)`, and `trend_presenter_present(…, &bytes, &len)`.
  That exposes regions, encoders, text, and ANSI output to C, Python, and
  other hosts. FFI sources stay RGBA buffers; callback-based sources can come
  later if needed.

## 11. Compared with the current architecture

| Concern | Today | Proposed |
| --- | --- | --- |
| Input | Borrowed `PixelFrameView` (+ timestamp) | Any `RenderSource`; `ImageSource` wraps a `PixelView` |
| Owned pixels | Each adapter has its own type | One `PixelBuffer` in the core |
| Scaling and fit | Fused into the renderer loop | `ImageSource` (same math), aware of aspect ratio |
| Cell encoding | Hard-coded half-block | `CellEncoder` trait with several implementations |
| Output target | Full `CellFrame` or contiguous slice | Any `CellViewMut` region |
| Layers and overlays | None | Pixel-level `draw_stack` + cell-level `blit`/`put_str` |
| Text and styles | None (`attributes` unused) | `put_str`, `Style`, `Attrs` |
| Presentation | `AnsiPresenter` diffs, encodes, and remaps monochrome | `DiffState` + `AnsiWriter`; `CellBackend` trait; monochrome is the `Density` encoder |
| Terminal setup | Duplicated in tools | Optional `trend-term` |
| Media dispatch | Enum `match` repeated; tools bypass it | `FrameProvider` internally; `MediaSource: RenderSource` |
| 3D | Not possible | External `RenderSource` + example |

### Stays (same algorithm, possibly a new module)

- `PixelFrameView` validation logic, as `PixelView`
- `scale.rs` sampling and alpha math, and `geometry.rs` fit and transform
  math, inside `ImageSource`
- `Cell` / `Color` 16-byte `#[repr(C)]` layout, extended compatibly
- `CellFrame`, as `CellBuffer` (old name kept as an alias)
- `AnsiPresenter` diffing and SGR-minimizing output behavior
- `color.rs` quantization, `dither.rs`, `timing.rs`, `cache.rs`, `queue.rs`,
  `resize.rs`
- C ABI v1, the adapter crate boundaries, and the rule that the host owns
  loop, threads, and audio

### Moves

| From | To |
| --- | --- |
| Monochrome density remap in `terminal.rs` | `Density` encoder (plus a legacy-mode flag in `AnsiWriter`) |
| `AnsiPresenter::render_at` (renderer + timing inside presenter) | `compat.rs`; the presenter no longer knows about the renderer |
| Env detection in `terminal.rs` | `present::caps`; active probing goes to `trend-term` |
| `experimental.rs` | `encode::experimental::GlyphMatch`, implementing `CellEncoder` |
| Tool terminal code (Win32 console, crossterm) | `trend-term` |
| Decoder-owned pixel vectors | `trend::PixelBuffer` |
| Timestamp on `PixelFrameView` | `MediaFrame` / `VideoFrame` |
| Size fields of the three `DecodeLimits` structs | `trend::SurfaceLimits` |
| Format dispatch in `test-viewer` and `ui-lab` | `trend-media` only |

### Replaced eventually (after the equivalence gate)

- `Renderer`'s fused sample-and-encode loop becomes `Compositor` +
  `ImageSource` + `HalfBlock`. `Renderer` remains as a thin deprecated facade.
- `render_to_slice*` becomes `CellViewMut`.
- `RenderRequest.viewport` "whole buffer only" becomes regions.
- `VideoDecoder`'s cfg-selected, convention-based API becomes an internal
  `VideoBackend` trait.
- The three animation decoder types become one.
- The `MediaSource` enum `match` becomes `FrameProvider`.

## 12. Compatibility gate

Before any old code path is removed, the facade must reproduce today's output
exactly:

- **Cells:** identical `CellFrame` for every fixture × filter (nearest,
  bilinear, area) × fit (contain, cover, stretch) × transform × dither ×
  matte combination, captured as golden snapshots from the current code
  first.
- **Bytes:** identical ANSI output for all four color modes, including the
  monochrome density remap.
- **Alpha subtlety:** `ImageSource` must store a nonzero alpha whenever the
  accumulated alpha is nonzero, even if 8-bit rounding would produce 0.
  Otherwise a nearly transparent sample that renders as a color today would
  become "no content".
- **Dither coordinates** must remain virtual-pixel coordinates within the
  viewport. The scratch buffer uses the same space, so this holds by
  construction.
- **Performance:** the new path adds one pass over the scratch buffer (about
  96 KB for 200×60 half-block cells, well within cache). The budget is at
  most +10% over today's benchmark per filter. If that's exceeded,
  `HalfBlock` + `ImageSource` gets a fused fast path behind the same API.
- **Tests:** existing tests, `fidelity_baseline`, the Python bridge tests,
  and the overlay smoke test pass unchanged.

## 13. Migration phases

Each phase is additive and leaves the workspace releasable.

0. **Baseline.** Golden cell and ANSI snapshots, render/ANSI benchmarks. No
   API change.
1. **Surface and views.** `geom`, `PixelView`/`PixelViewMut`/`PixelBuffer`,
   `CellView`/`CellViewMut`. `PixelFrameView` becomes a thin legacy wrapper.
2. **Raster + encode split.** `RenderSource`, `ImageSource`, `HalfBlock`,
   `Compositor::draw`. Reimplement `Renderer` on top and pass the gate.
3. **Presenter split.** `DiffState`, `AnsiWriter`, `CellBackend`, `Density`
   encoder. The `AnsiPresenter` API is unchanged.
4. **Composition.** `CellBlend`, `blit`, `put_str`, `Style`/`Attrs`,
   `draw_stack`, `Color::TRANSPARENT`.
5. **Adapters.** Decode into `PixelBuffer`, `VideoBackend` trait, merged
   animation decoder, `MediaSource: RenderSource`. Tools migrate to
   `trend-media`.
6. **New encoders.** `Quadrant`, `Sextant`, `Braille`; `GlyphMatch`
   graduates if it earns it.
7. **Optional crates and ABI.** `trend-draw`, `trend-term`, the
   `software-3d` and `layers` examples, C ABI v2.

The Android MP4/H.264 milestone in
[development-status.md](../development-status.md) is independent. It can land
before phase 5 against today's `VideoFrame` contract, then move behind
`VideoBackend` with the other backends.

## 14. Open decisions

1. **Alpha representation.** Keep straight alpha everywhere (simplest,
   matches today) or premultiply internally for cheaper blending?
   Recommendation: straight at boundaries, premultiplied only inside blend
   ops.
2. **Renames.** `PixelFrameView` to `PixelView` and `CellFrame` to
   `CellBuffer` with deprecated aliases, or keep the old names?
3. **`trend-term` dependencies.** Hand-rolled termios/Win32 (zero
   dependencies, more code) or `crossterm` behind a feature? Recommendation:
   hand-rolled by default, since the Win32 half already exists in the tools.
4. **Text scope.** Narrow characters only at first, or wide/emoji support
   (width tables are data, not dependencies) from the start?
5. **Where `time` and `util` live.** They stay in the core for now as
   zero-cost helpers. They could move to `trend-media` if the core should be
   strictly render-only.
6. **Pixel graphics protocols** (sixel, kitty): out of scope, or a planned
   `trend-graphics` crate?

# C ABI v2: design (not implemented)

Status: design only, written in M6. Nothing here is exported yet. The v1
ABI in `include/trend.h` is unchanged and stays supported alongside v2.

## 1. Why v1 is not enough

v1 exports four functions: `trend_version`, `trend_cell_size`,
`trend_render_rgba8`, and `trend_render_rgba8_ex`. They wrap the legacy
`Renderer`: straight RGBA8 in, half-block `TrendCell`s out, fitted and
resampled every call. That covers "decode a video frame, draw it", and
nothing TRend gained in M1–M5:

| Rust capability (stable since M4/M5) | v1 |
| --- | --- |
| Encoders: HalfBlock, Quadrant, Sextant, Braille, Density | half-block only |
| Borrowed layouts: BGRA, bottom-up, premultiplied, ignored alpha, stride | RGBA straight, top-down |
| Zero-copy encoding of exact-size frames (`Surface::encode_from`) | always resamples |
| Damage regions (`encode_region`, `encode_from_region`, `present_region`) | whole frame |
| Presentation to ANSI bytes, diffed, with `OutputOptions` | cells only; the host writes its own ANSI |
| Overlays: `blend_over`, `put_str` | none |
| Statistics: `PresentStats` | none |
| Pixel aspect for 3D projection (`RasterSpec::pixel_aspect`) | none |

A C, C++, Python (ctypes/cffi), C#, or Go host today reimplements the
presenter, or stays on the legacy path. The CEF host in
[readback-recipes.md](../readback-recipes.md) has to be written in Rust.

## 2. Goals and non-goals

Goals:

- Expose `Surface` — the one object that owns spec, pixels, cells, and
  presenter — rather than the pipeline's pieces one by one.
- Keep v1's guarantees: no allocation in steady-state frame calls, no
  panics crossing the boundary, every failure a `TrendResult`, and every
  struct extensible through a leading `struct_size`.
- Borrow the host's framebuffer exactly as Rust does: exact-size frames
  are read in place.
- Stay small enough to freeze. Each function added is a function kept
  forever.

Non-goals:

- Terminal I/O (raw mode, size, input). That belongs to `trend-term` (see
  [trend-term-design.md](trend-term-design.md)), which would get its own
  C surface if one is ever needed.
- Custom encoders implemented in C. `CellEncoder` stays a Rust trait; C
  selects a built-in encoder by id.
- Mirroring every Rust type. `RasterSpec`, `PixelBuffer`, `CellFrame`,
  `ImageSource`, and `AnsiPresenter` stay behind the surface.
- Replacing v1. v1 remains, frozen, for existing callers.

## 3. Proposed surface

All names are provisional. Every function returns `int32_t` (`TrendResult`)
unless it is a pure getter.

### 3.1 Lifetime

```c
typedef struct TrendSurface TrendSurface; /* opaque */

typedef struct TrendSurfaceConfig {
    uint32_t struct_size;     /* sizeof(TrendSurfaceConfig) */
    uint16_t columns, rows;   /* viewport in cells */
    uint8_t  encoder;         /* TREND_ENCODER_HALF_BLOCK, ... */
    uint8_t  encoder_flags;   /* Density: ramp and color bits */
    uint8_t  color_mode;      /* TREND_COLOR_TRUECOLOR, _256, _16, _MONO */
    uint8_t  unicode;         /* 0 or 1 */
    uint16_t cell_aspect_num, cell_aspect_den; /* 0/0 = default 1:2 */
    uint32_t output_flags;    /* TREND_OUTPUT_SYNC | _COMBINED | _COMPACT */
} TrendSurfaceConfig;

int32_t trend_surface_create(const TrendSurfaceConfig *config, TrendSurface **out);
void    trend_surface_destroy(TrendSurface *surface); /* NULL is a no-op */
int32_t trend_surface_configure(TrendSurface *surface, const TrendSurfaceConfig *config);
```

`configure` covers resize, encoder change, capabilities, and output
options in one call: the Rust setters are cheap and idempotent, so one
entry point keeps the export list short. It reallocates only when the
pixel buffer grows, as `Surface::resize` does.

Internally the handle is a `Box<Surface<Box<dyn CellEncoder + Send>>>`; the
blanket `impl CellEncoder for Box<T>` already supports this, and the
external-renderer example uses exactly that type. The one virtual call
is per frame, not per pixel.

### 3.2 Geometry

```c
typedef struct TrendSurfaceInfo {
    uint32_t struct_size;
    uint32_t pixel_width, pixel_height; /* the frame size encode_from accepts */
    uint16_t columns, rows;
    uint8_t  subcell_columns, subcell_rows;
    float    pixel_aspect;              /* width / height of one pixel */
} TrendSurfaceInfo;

int32_t trend_surface_info(const TrendSurface *surface, TrendSurfaceInfo *out);
```

This is what a renderer needs to size its render target and projection
(`spec().width()/height()`, `spec().pixel_aspect()`).

### 3.3 Frames in

```c
typedef struct TrendFrame {
    uint32_t struct_size;
    const uint8_t *pixels;
    size_t   length;          /* bytes readable from pixels */
    uint32_t width, height;
    size_t   stride;          /* bytes between rows in memory */
    uint8_t  format;          /* TREND_FORMAT_RGBA8, _BGRA8 */
    uint8_t  row_order;       /* TREND_ROWS_TOP_DOWN, _BOTTOM_UP */
    uint8_t  alpha;           /* TREND_ALPHA_STRAIGHT, _PREMULTIPLIED, _IGNORE */
    uint8_t  reserved;
} TrendFrame;

typedef struct TrendRect { uint32_t x, y, width, height; } TrendRect;

int32_t trend_surface_encode_frame(TrendSurface *s, const TrendFrame *frame,
                                   const TrendRect *damage /* NULL = all */);
int32_t trend_surface_encode_image(TrendSurface *s, const TrendFrame *frame,
                                   const TrendRenderOptions *fit /* v1 struct, reused */,
                                   const TrendRect *damage /* NULL = all */);
```

- `encode_frame` maps to `encode_from` / `encode_from_region`: exact size
  only, read in place, `TREND_ERROR_INVALID_FRAME` otherwise.
- `encode_image` maps to `encode_image` / `encode_image_region`, and
  reuses v1's `TrendRenderOptions` for sampling, dithering, alignment,
  zoom, and pan, plus the fit mode (a new field appended under
  `struct_size`).
- `TrendFrame` is `PixelLayout` plus a pointer. Validation is
  `PixelFrameView::from_layout`, which already rejects short buffers and
  bad strides.

### 3.4 Drawing into the surface

```c
typedef struct TrendPixels { uint8_t *pixels; uint32_t width, height; size_t stride; } TrendPixels;

int32_t trend_surface_pixels(TrendSurface *s, TrendPixels *out); /* straight RGBA, top-down */
int32_t trend_surface_encode(TrendSurface *s, const TrendRect *damage /* NULL = all */);
int32_t trend_surface_blend(TrendSurface *s, const TrendFrame *source,
                            int32_t x, int32_t y, TrendRect *damage_out);
```

`trend_surface_pixels` hands out the surface's own buffer for software
renderers (`Surface::draw` / `pixels_mut`). The pointer is valid until
the next `configure` or `destroy`: the one lifetime rule a C host must
follow, documented on the function.

### 3.5 Cells and text

```c
typedef struct TrendTextStyle {
    uint32_t struct_size;
    TrendColor foreground;
    uint8_t  background_mode;  /* KEEP, DEFAULT, COLOR */
    TrendColor background;
    uint8_t  attributes;
} TrendTextStyle;

int32_t trend_surface_put_text(TrendSurface *s, uint16_t column, uint16_t row,
                               const char *utf8, size_t length,
                               const TrendTextStyle *style, uint16_t *written);
int32_t trend_surface_cells(TrendSurface *s, TrendCell **cells,
                            uint16_t *columns, uint16_t *rows);
```

`put_text` takes a length, not a NUL-terminated string, and validates
UTF-8 (`TREND_ERROR_INVALID_TEXT`). `trend_surface_cells` exposes the
cell grid in the existing `TrendCell` layout for hosts that edit cells
directly or render them without ANSI.

### 3.6 Presentation

```c
typedef struct TrendPresentStats {
    uint32_t struct_size;
    size_t bytes, cells, runs, color_sequences, attribute_sequences;
} TrendPresentStats;

int32_t trend_surface_present(TrendSurface *s, const uint8_t **bytes, size_t *length,
                              const TrendRect *cells /* NULL = all */);
int32_t trend_surface_set_origin(TrendSurface *s, uint16_t row, uint16_t column);
int32_t trend_surface_invalidate(TrendSurface *s);
int32_t trend_surface_stats(const TrendSurface *s, TrendPresentStats *out);
```

`present` returns a pointer into the presenter's reused buffer
(`present_bytes` / `present_region_bytes`), valid until the next call on
the surface. TRend never writes to a file descriptor: the host owns
output, blocking, and back-pressure, as in Rust.

### 3.7 Constants and version

- `trend_version()` returns the fixed value `0x00010000`, and hosts may
  compare it for equality (the C smoke test does), so it stays unchanged.
  A new `trend_abi_features()` returns a bit mask (bit 0: v1, bit 1: v2),
  so a host that loads the library dynamically can check for v2 by
  looking the symbol up first.
- Encoder ids, color modes, formats, row orders, alpha modes, and output
  flags are `enum`s with explicit values, as in v1. Unknown values are
  `TREND_ERROR_INVALID_OPTIONS`.

In total: about 16 functions and 8 structs, all flat, fixed-size, and
`struct_size`-versioned.

## 4. Rules at the boundary

- **Panics.** The release profile uses `panic = "abort"`, so a panic
  cannot unwind into C; it terminates. v1 relies on validation so that
  no input can panic. v2 must do the same, and must be fuzzed at the C
  level (arbitrary configs, frames, rects, and UTF-8) before it is
  frozen. `catch_unwind` would only help with `panic = "unwind"`; if
  that profile is ever supported, each export gains a guard returning
  `TREND_ERROR_INTERNAL`.
- **Threads.** A `TrendSurface` must not be used from two threads at
  once; distinct surfaces are independent. Moving a surface between
  threads requires the boxed encoder to be `Send`, which all built-in
  encoders are, so the handle wraps `Box<dyn CellEncoder + Send>`.
- **Allocation.** `create` and growing `configure` allocate. Encoding,
  blending, text, and presentation do not, once the output buffer has
  grown to a frame's size (measured: 0 allocations per frame; see
  [benchmarks.md](../benchmarks.md)).
- **Borrowed memory.** Frames are read only during the call. Pointers
  returned by `pixels`, `cells`, and `present` are invalidated by the next
  `configure`, `present`, or `destroy` on that surface.
- **Struct growth.** Every input struct starts with `struct_size`; the
  library reads only the fields that size covers and zero-fills the rest.
  Output structs are filled up to the caller's `struct_size`.

## 5. What not to expose

- The legacy `Renderer`, `RenderRequest`, `CellFrame`, and generations:
  v1 already covers them.
- `RasterSpec`, `SubcellGrid`, and `CellAspect` as objects: their facts
  are in `TrendSurfaceInfo`.
- `ImageSource` as an object: `encode_image` takes its options.
- `SourceMapping` and `cell_at_terminal` until a C host needs mouse input;
  they are easy to add as getters later.
- Experimental APIs (`experimental.rs`) and anything behind features.

## 6. Staged plan

1. **Draft header** (`include/trend_v2.h`) with the declarations above,
   compiled by `tests/c/consumer_smoke.c`-style static assertions only.
   Review it with one real C host before any export exists.
2. **Implement behind a feature** (`ffi-v2`), in `src/ffi_v2.rs`, as thin
   wrappers over `Surface`. The existing four exports are untouched.
3. **Port the external-renderer example's host to C** as the acceptance
   test: the renderer's BGRA, bottom-up, premultiplied frame must go
   through `trend_surface_encode_frame` with zero copies and match the
   Rust host's bytes exactly.
4. **Fuzz** every export, then freeze: remove the feature gate and add
   the header to the release packages.

The design is justified by the gap in section 1, but freezing it before
step 3 would lock in guesses. M6 therefore stops at this document.

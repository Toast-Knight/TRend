# C ABI

The stable declarations live in `include/trend.h`. Build `trend` as a `cdylib`
to produce `trend.dll`, `libtrend.so`, or `libtrend.dylib` and link against the
matching platform artifact.

The ABI exports:

- `trend_version`
- `trend_cell_size`
- `trend_render_rgba8`
- `trend_render_rgba8_ex`

Call `trend_cell_size()` and compare it with `sizeof(TrendCell)` before rendering.
Input must be non-null RGBA8 with a stride of at least `source_width * 4` bytes.
Output capacity is measured in `TrendCell` elements and must be at least
`viewport_width * viewport_height`. Dimensions and enum values are validated;
all failures are returned as `TrendResult` values.

`trend_render_rgba8_ex` accepts `TrendRenderOptions`. Initialize the structure
to zero, set `struct_size = sizeof(TrendRenderOptions)`, then set sampling,
dithering, alignment, zoom, and pan fields. A zoom of zero is invalid.

Windows packages place the DLL in `bin/`, the MSVC import library in `lib/`, and
the header in `include/`. Keep the DLL beside the consuming executable or on
`PATH`. Rust callers should use the safe native API instead of the C boundary.

`scripts/c-consumer-smoke.sh` compiles and runs a strict C11 consumer of the
header (`tests/c/consumer_smoke.c`) that checks the struct layout at compile
time and every export at run time.

These four functions wrap the legacy half-block renderer. A C surface for
`Surface`, the other encoders, borrowed layouts, regions, overlays, and
presentation is designed in [ffi-v2-design.md](ffi-v2-design.md) and not yet
implemented; this ABI stays as it is.

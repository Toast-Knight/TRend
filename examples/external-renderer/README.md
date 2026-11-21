# External renderer → framebuffer → TRend → terminal

An end-to-end example of TRend's intended boundary: a renderer that knows
nothing about terminals produces frames in its own memory, and TRend reads
them in place and turns them into terminal output.

```text
renderer/  tiny-raster             a small software 3D renderer; no TRend dependency
host/      trend-external-renderer  the only code that knows both sides
```

## Run it

From the repository root:

```text
cargo run --release -p trend-external-renderer -- --encoder sextant --size 120x36
```

Options: `--encoder half|quadrant|sextant|braille`, `--size COLSxROWS`,
`--frames N` (default 300), `--fps N` (default 30),
`--colors truecolor|256|16`, and `--compact` (compact indexed colors).
Without `--size`, the size comes from `COLUMNS`/`LINES` (one row is kept
for the summary) or defaults to 100×30. Detecting the terminal size
belongs to a terminal-session layer (see
[docs/trend-term-design.md](../../docs/design/trend-term-design.md)), not to
TRend's core.

At exit it prints the median time of each stage against the 33.3 ms
budget of 30 frames per second, bytes per frame, and the number of
allocations TRend made while running (0).

## What each side does

`tiny-raster` renders a lit, depth-buffered torus and a translucent panel
into a framebuffer it owns, in conventions GPU readbacks commonly use:
BGRA byte order, bottom-left origin, premultiplied alpha with a
transparent background, and rows padded to 256 bytes. It describes that
memory with its own `FrameInfo` type.

The host:

1. Creates a `Surface` for the chosen encoder and sizes the renderer to
   `surface.spec().width()` × `height()`, passing
   `surface.spec().pixel_aspect()` so the projection is not stretched by
   non-square terminal pixels.
2. Maps `FrameInfo` onto a `PixelLayout` in one function, `layout_of`.
   That function is the entire integration.
3. Each frame: `renderer.render(...)`, then
   `surface.encode_from(PixelFrameView::from_layout(renderer.pixels(), layout)?)`,
   which reads the borrowed frame in place, then a status line with
   `put_str`, then `surface.present_bytes()`, written and flushed to stdout.

Nothing is copied or converted between the renderer and the encoder. The
host's test checks that the cells produced from the borrowed frame equal
those produced from an explicitly converted straight-RGBA copy.

## Measured (cloud container, output to a pipe)

100×30 sextant at 30 fps, medians per frame: render 0.30 ms, encode
0.14 ms, present 0.05 ms, write + flush 0.04 ms; about 9.3 KB per frame;
0 allocations in TRend. Writing to a real terminal costs more; that is
the stage to watch when a frame misses its budget.

## What stays out of TRend

The renderer, its scene, its math, and its frame description all live in
`renderer/`. TRend's core gained nothing for this example: it already
accepts the renderer's layout through `PixelLayout`.

# Readback recipes

How to hand frames from a graphics API or a browser engine to TRend without
copying or converting them. These are recipes, not code in the repository:
TRend's core stays dependency-free, and each graphics stack lives in the
application.

The pattern is the same everywhere:

1. Render at the size TRend asks for: `surface.spec().width()` ×
   `surface.spec().height()`. That size already includes the encoder's
   subcell grid, and `spec.pixel_aspect()` tells a 3D projection how wide
   each pixel is (see `examples/hud_3d.rs`).
2. Describe the mapped bytes with a `PixelLayout`: format, row stride, row
   order, and alpha mode.
3. Call `surface.encode_from(frame)`, which reads the bytes in place, or
   `surface.encode_from_region(frame, dirty)` for a changed rectangle.
4. Call `surface.present(&mut stdout)`.

A frame of any other size is rejected rather than stretched. Use
`surface.encode_image(&ImageSource::new(frame))` when you want TRend to fit
and resample it.

## wgpu

Texture-to-buffer copies need rows padded to 256 bytes
(`wgpu::COPY_BYTES_PER_ROW_ALIGNMENT`), and mapped buffers are read
asynchronously.

```rust,ignore
let (width, height) = (surface.spec().width(), surface.spec().height());
let padded = (width * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);

// Render into a `Rgba8UnormSrgb` (or `Bgra8UnormSrgb`) texture of that
// size, then copy it into a MAP_READ buffer of `padded * height` bytes with
// `bytes_per_row: Some(padded)`. (The copy's struct names changed in wgpu 24:
// `TexelCopyTextureInfo` / `TexelCopyBufferInfo` were `ImageCopyTexture` /
// `ImageCopyBuffer`.)

// Map the buffer and poll the device until the callback fires (the polling
// call's name also varies by wgpu version).
let mapped = readback.slice(..).get_mapped_range();
let layout = PixelLayout::new(width, height, PixelFormat::Rgba8, AlphaMode::Ignore)
    .with_stride(padded as usize);
surface.encode_from(PixelFrameView::from_layout(&mapped, layout)?)?;
drop(mapped);
readback.unmap();
surface.present(&mut stdout)?;
```

- **Format.** `Bgra8*` textures map to `PixelFormat::Bgra8`. Read `*Srgb`
  formats as they are: their bytes are already the sRGB values terminals
  expect. Linear `Rgba8Unorm` targets hold linear values and look dark
  unless the application encodes to sRGB itself.
- **Alpha.** Use `AlphaMode::Ignore` for opaque scenes; alpha left over from
  blending would otherwise punch holes. Use `AlphaMode::Premultiplied` when
  the scene is meant to be transparent and was rendered premultiplied.
- **Latency.** A ring of two or three readback buffers keeps the GPU busy
  while the previous frame is encoded. TRend only borrows the mapped slice
  for the duration of the encode.

## OpenGL

`glReadPixels` returns rows bottom-up. RGBA rows are always a multiple of 4
bytes, so the stride is `width * 4`.

```c
glPixelStorei(GL_PACK_ALIGNMENT, 4);
glReadPixels(0, 0, width, height, GL_RGBA, GL_UNSIGNED_BYTE, pixels);
```

```rust,ignore
let layout = PixelLayout::new(width, height, PixelFormat::Rgba8, AlphaMode::Ignore)
    .with_row_order(RowOrder::BottomUp);
surface.encode_from(PixelFrameView::from_layout(&pixels, layout)?)?;
```

- The default framebuffer's alpha is often undefined; keep
  `AlphaMode::Ignore` unless you render into an FBO with meaningful alpha.
- `GL_BGRA` reads map to `PixelFormat::Bgra8`, which some drivers return
  faster.
- For asynchronous reads, read into a pixel buffer object and map it a
  frame later. The mapped pointer is the slice TRend borrows.

## Chromium Embedded Framework (off-screen rendering)

With windowless rendering, CEF calls `CefRenderHandler::OnPaint` with a BGRA
buffer, upper-left origin and `width * 4` stride, plus the rectangles that
changed. Run the browser in a separate host program. That program links CEF
and uses TRend from Rust; the core's C ABI doesn't expose `Surface` yet
(see [ffi-v2-design.md](design/ffi-v2-design.md) for the planned C surface).

- In `GetViewRect`, report the surface's pixel size, and set the device
  scale factor to 1 in `GetScreenInfo`, so frames match the spec exactly.
- In `OnPaint`:

  ```rust,ignore
  let layout = PixelLayout::new(width, height, PixelFormat::Bgra8, AlphaMode::Premultiplied);
  let frame = PixelFrameView::from_layout(buffer, layout)?;
  for dirty in dirty_rects {
      surface.encode_from_region(frame, PixelRect::new(dirty.x, dirty.y, dirty.width, dirty.height))?;
  }
  surface.present(&mut stdout)?; // Only changed cells are written.
  ```

  Opaque pages have alpha 255 everywhere, so the alpha mode only matters
  for transparent backgrounds, which CEF renders premultiplied.
- **Input.** Map a terminal mouse position to page pixels, then send CEF a
  mouse event at those coordinates:

  ```rust,ignore
  if let Some((column, row)) = surface.cell_at_terminal(mouse_row, mouse_column) {
      let cell = surface.spec().cell_pixels(column, row).unwrap();
      let (x, y) = (cell.x + cell.width / 2, cell.y + cell.height / 2);
      // Send (x, y) to the browser.
  }
  ```

  Mouse reporting itself (enabling SGR 1006 mode and parsing the events)
  belongs to the terminal session, which is outside the core.

## Other framebuffers at a glance

Most software and GPU framebuffers are one of these. "Packed ARGB"
formats are 32-bit words in native byte order, which on little-endian
machines (x86, ARM as used by every desktop and phone OS) puts the bytes
in memory as B, G, R, A: that is `PixelFormat::Bgra8`.

| Source | `PixelFormat` | `RowOrder` | `AlphaMode` | Stride |
| --- | --- | --- | --- | --- |
| wgpu / Vulkan / Metal `*8Unorm(Srgb)` readback | `Rgba8` or `Bgra8` | `TopDown` | `Ignore` (opaque) or `Premultiplied` | the padded row size of the copy |
| OpenGL `glReadPixels` | `Rgba8` (`GL_RGBA`) or `Bgra8` (`GL_BGRA`) | `BottomUp` | `Ignore` | `width * 4` |
| Direct3D 11/12 staging texture, `B8G8R8A8_UNORM` | `Bgra8` | `TopDown` | `Ignore` | `RowPitch` from `Map` |
| Windows GDI DIB section, 32 bpp | `Bgra8` | `BottomUp` if `biHeight > 0`, else `TopDown` | `Ignore` (`Premultiplied` for layered windows) | `width * 4` |
| X11 `XGetImage` (`ZPixmap`, depth 24) | `Bgra8` | `TopDown` | `Ignore` | `bytes_per_line` |
| Cairo `CAIRO_FORMAT_ARGB32` / `RGB24` | `Bgra8` | `TopDown` | `Premultiplied` / `Ignore` | `cairo_image_surface_get_stride` |
| Skia `kN32` with `kPremul_SkAlphaType` | `Bgra8` | `TopDown` | `Premultiplied` | `rowBytes()` |
| Qt `QImage::Format_ARGB32_Premultiplied` / `RGB32` / `RGBA8888` | `Bgra8` / `Bgra8` / `Rgba8` | `TopDown` | `Premultiplied` / `Ignore` / `Straight` | `bytesPerLine()` |
| SDL surface `ARGB8888` / `RGBA32` | `Bgra8` / `Rgba8` | `TopDown` | `Straight` | `pitch` |
| FFmpeg `AV_PIX_FMT_BGRA` / `RGBA` / `BGR0` / `RGB0` | `Bgra8` / `Rgba8` / `Bgra8` / `Rgba8` | `TopDown` | `Straight`, or `Ignore` for the `0` formats | `linesize[0]` |
| CEF `OnPaint` | `Bgra8` | `TopDown` | `Premultiplied` | `width * 4` |

Formats TRend does not read — 24-bit RGB, 16-bit 565, YUV, 10-bit, and
floating point — are converted by the host, ideally by the producer itself
(ask the GPU or `sws_scale` for BGRA). The cost of reading a foreign layout
in place is small: in M6 a BGRA, bottom-up, premultiplied frame encodes in
about the same time as straight RGBA (see [benchmarks.md](benchmarks.md)).

### Showing part of a larger framebuffer

When the surface shows a window into something bigger — a viewport of an
editor's canvas, one half of a split screen, a crop of a capture — take a
sub-view of the exact size. It borrows the same memory, keeps the layout,
and is still read in place:

```rust,ignore
let framebuffer = PixelFrameView::from_layout(&bytes, layout)?;
let window = framebuffer.sub(PixelRect::new(x, y, surface.spec().width(), surface.spec().height()))?;
surface.encode_from(window)?;
```

Coordinates are always top-down image coordinates, whatever the row order
in memory.

### A complete example

`examples/external-renderer` is a two-crate example of the whole
boundary. `tiny-raster`, a small software 3D renderer that knows nothing
about TRend, renders BGRA, bottom-up, premultiplied frames with rows
padded to 256 bytes, as GPU readbacks commonly are. The host maps the
renderer's own frame description onto a `PixelLayout` in one function and
encodes each frame in place. See its README.

## Frames TRend should resample

Video decoders, screenshots, and anything else rendered at its own size go
through `ImageSource`:

```rust,ignore
surface.encode_image(&ImageSource::new(frame).with_sampling(SamplingFilter::Area))?;
let mapping = surface.source_mapping().unwrap(); // For input: cell_to_source.
```

`SourceMapping::source_damage(dirty)` turns a changed rectangle of the
source into the surface pixels to pass to `encode_image_region`.

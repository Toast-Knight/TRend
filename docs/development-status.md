# Development status

This is TRend's living maintainer handoff. Update it after significant project
events, validation runs, releases, or priority changes. Keep this document
focused on project facts, recent events, and actionable next work.

## Current baseline

- Branch: `main`
- Cleanup checkpoint: `4691d41` (`Overhaul repository developer experience`)
- Workspace version: `0.1.0`
- Release tag: not created or pushed
- Crates.io publishing: disabled intentionally
- Generated package root: `dist/`
- Known untracked file: `tests/fixtures/media/63173162a98b20f741f434b2ee1b5de4.gif`
  has unknown provenance and must not be committed or deleted without review.

## Current capabilities

| Area | State |
| --- | --- |
| Core RGBA8 to terminal rendering | Runtime-tested |
| TrueColor, ANSI 256/16, monochrome | Runtime-tested |
| Nearest, bilinear, and area sampling | Runtime-tested |
| Contain, cover, and stretch fitting | Runtime-tested |
| PNG, JPEG, GIF, WebP | Runtime-tested |
| Windows MP4/MKV/AVI/MOV/WebM routes | Runtime-tested where documented |
| Linux WebM/MKV VP8/VP9 and MKV H.264 | Runtime-tested in WSL2 |
| Python bridge and positioned rendering | Runtime-tested |
| 15x15 top-right overlay | Runtime-tested |
| C ABI | Built, covered by Rust tests and a strict C11 consumer smoke test |
| Android image/animation/terminal output | Device-validated by maintainer |
| Android video playback | Not implemented |

## Recent events

### 2026-11-21: public API review before a first release

- Removed from the core what nothing used: the `experimental` glyph matcher (failed real-viewer
  acceptance; with its example and font-validation script) and `BoundedQueue`. Output, goldens
  and the C ABI are unchanged.
- `trend-3d` exports four loading functions instead of nine; `trend-term`'s `SessionOptions`
  uses builder methods. Types in the three new crates that will grow are `#[non_exhaustive]`.
- Kept after review, although little used: `ResizeCoalescer` (pairs with render generations and
  cancellation), `FrameDecision` (part of the documented audio/video sync interface),
  `CellFrameCache`, the legacy `Renderer` path (the media crates, the C ABI and the goldens rest
  on it), and `tools/ui-lab`.
- The APIs of the core's remaining items and of the image, animation, video and media crates
  were not changed.
- Documentation reorganised: user guides at the top of `docs/`, design notes in `docs/design/`,
  historical records in `docs/history/`.
- Open for the maintainer: whether `tools/ui-lab` (a Windows-only GUI experiment) belongs in the
  public repository; a minimum supported Rust version (the code uses `let` chains, so at least
  1.88, not verified on older toolchains); `compact_indexed_colors` as a default (see
  [api.md](api.md)); crates.io names and publishing.

### 2026-11-21: the test viewer runs on `trend-term`

- `tools/test-viewer` uses `trend_term::Session` for everything that owns the terminal, on
  Windows and Unix from one piece of code; it has no `unsafe` and no platform-specific code
  left. Roadmap M7's first exit criterion is met in code.
- `trend-term` gained what the port showed was missing: resize detection that does not depend
  on the terminal announcing it, a colour reset on exit, and a clean failure on Windows consoles
  without escape-sequence support. The `showcase` example uses the mouse.
- The `external-renderer` host and the core's own examples stay without `trend-term` on
  purpose: the first is a measurement harness for the core alone, the others must build with no
  dependencies.
- Validation: 325 workspace tests pass; clippy and rustfmt are clean; the viewer, `starter` and
  `showcase` were driven in a Linux pseudo-terminal (keys, mouse, announced and unannounced
  resizes, exit, and restore after a panic with `panic = "abort"`); the Windows builds compile
  and link (`x86_64-pc-windows-gnu`).
- The maintainer ran the viewer and the `showcase` example on Windows on 2026-11-21 and reported
  them working. Which terminal (Windows Terminal or the classic console) was not recorded, and
  the full `scripts\visual-test-windows.bat` pass has not been repeated.
- Next: probing by query, real-terminal throughput (roadmap M7), and the public API review
  before a first release.

### 2026-11-20: optional crates for 3D, text and the terminal session

- New optional crates under `adapters/`, none of which the core depends on:
  - `trend-3d` (no dependencies besides `trend`): OBJ, STL, PLY and glTF/GLB loading, a software
    renderer with physically based materials, image-based lighting and sky boxes, a first-person
    camera, fog, and crisp textures.
  - `trend-text` (no dependencies besides `trend`): scaled text drawn as pixels, with a built-in
    pixel font and a TrueType rasterizer.
  - `trend-term` (crossterm backend): the terminal session of
    [trend-term-design.md](design/trend-term-design.md): enter and restore, size and resize, keys with
    held-key state, mouse, and timed frame writes. Examples: `starter` (a complete program in
    about 50 lines) and `showcase` (3D, text and the session together).
- The core (`src/`) is unchanged by these crates.
- Not verified on a real Windows terminal yet: `trend-term` was exercised in a Linux
  pseudo-terminal and type-checked for Windows.
- Next: port the test viewer and the remaining examples onto `trend-term`, add probing, and
  measure real-terminal throughput (roadmap M7).

### 2026-09-26: after M6, real-renderer tuning

- A private test harness outside the project fed an existing Three.js
  renderer's WebGL render target (RGBA8, bottom-up, alpha ignored) to
  `Surface`. The maintainer measured HalfBlock at 30+ FPS, Quadrant about
  25, and Sextant 15–20 at about 400 columns.
- Profiling its frames showed resampling, not fitting, dominated (Sextant
  400×120: 14 ms resampling, 3.7 ms encoding). Area and Bilinear now read
  rows through the per-layout decoders and walk column positions without
  dividing. The fitters return uniform blocks early, and means use the exact
  reciprocal. Same pixels and bytes (1,620-case comparison with M6). Sextant
  at 400×120 dropped from 16.9 to 5.5 ms on one thread.
- Added opt-in `Surface::encode_image_parallel`/`encode_from_parallel`
  (cell-row bands on scoped threads; identical output).
- New tests: Area row exactness, the division walk, and `tests/parallel.rs`.
  11 injected bugs were all caught. The core suite is 234 tests.
- Next: re-measure with the external renderer to see whether readback or
  the terminal write is now the limit.

### 2026-09-26: M6 measurement, integration, and design (checkpoint)

- Verified the workspace offline: core suite 229 tests; workspace tests
  pass with default features and with every feature that builds without
  system codec libraries. `--all-features` needs libvpx for `linux-webm` and
  `linux-mkv`, which the cloud container couldn't install. Clippy, rustfmt,
  and rustdoc are clean; the core still has no dependencies; the C ABI and
  header are unchanged.
- Added `examples/frame_budget.rs` and measured every encoder at 200×60 and
  320×90 against the 30-FPS budget (see [benchmarks.md](benchmarks.md)).
  Steady-state TRend cost is 0.3–17% of 33.3 ms at 200×60 and at most 42%
  at 320×90 (Braille video); 0 allocations per frame. Full-motion
  truecolor video writes 13–33 MB/s, which the terminal must absorb.
- Fixed the two bottlenecks the profile found, with byte-identical output:
  per-row Bilinear resampling (about 2.4× faster) and per-layout row
  decoders in encoders and `blend_over` (HalfBlock and Density encode about
  5× faster). Video frames cost 33–67% less; foreign-layout readbacks
  25–61% less.
- Compact indexed colors: fixed a 16-color CPU overhead (up to +42%); compact
  output now costs less CPU than the default in every workload and writes
  1.7–5.4× fewer bytes. Decision: opt-in now, default at the next
  output-compatibility boundary ([api.md](api.md)).
- `opt-level = 3` makes the block encoders 13–43% faster for +0.4%
  `libtrend.so` size (+18% for the media viewer). Recommended; the profile
  is left as a maintainer packaging decision.
- Added `examples/external-renderer` (a standalone renderer feeding
  borrowed BGRA, bottom-up, premultiplied frames through TRend), a strict
  C11 header consumer (`scripts/c-consumer-smoke.sh`), a framebuffer layout
  table in [readback-recipes.md](readback-recipes.md), and the designs
  [ffi-v2-design.md](design/ffi-v2-design.md) and
  [trend-term-design.md](design/trend-term-design.md). Neither design is
  implemented.
- 12 injected bugs in the M6 changes were all caught.
- Not measured here: real terminal throughput (no terminal in the
  container), and other CPUs.
- Next: review, then M7 (see [architecture-roadmap.md](architecture-roadmap.md)).

### 2026-09-25: M5 encoders, composition, and text (checkpoint)

- Recorded golden hashes for `Quadrant` and `Density` from M4 before
  changing anything. Quadrant now encodes 3–4× faster with identical cells.
- Added `Sextant` and `Braille` on a shared fast two-color fitter, measured
  within 0.03% of the exhaustive optimum. At 200×60 they encode in about
  1.6 and 2.1–3.0 ms (release profile), still below M4's Quadrant.
- Added `compose::blend_over`, `text::put_str`, cell attribute output,
  `Surface::cells_mut` and `encode_from_region`, and the opt-in
  `compact_indexed_colors` output (3–5× smaller indexed output).
- New example `hud_3d` and new doc `readback-recipes.md`. New tests:
  - `tests/golden_encoders.rs`;
  - `tests/fine_encoders.rs` (glyph tables checked against Unicode names);
  - `tests/overlays.rs`;
  - fitter and blend exactness tests;
  - allocation tests for every new path.

  18 injected bugs were all caught. The core suite is 225 tests.
- Next: review, then M6 (see [architecture-roadmap.md](architecture-roadmap.md)).

### 2026-09-25: M4 integration ergonomics (checkpoint)

- Added `Surface`, the convenience layer over the pipeline (see
  [api.md](api.md)). It owns encoder, spec, pixels, cells, and presenter; it
  is not a scene graph, compositor, or terminal session.
- Added opt-in presenter output (synchronized updates, combined colors),
  per-frame statistics, region presentation, and invalidation. Default bytes
  are unchanged and pinned by the golden tests.
- Added input and damage mapping: cell ↔ pixel on `RasterSpec`, terminal
  position → cell on `Surface`, surface ↔ source pixel and source damage →
  surface damage on `SourceMapping`.
- `procedural_source` was rewritten on `Surface` with byte-identical output;
  added `surface_demo`. Benchmarks are in [benchmarks.md](benchmarks.md):
  - damage regions cut a mostly static UI frame from about 165 µs to 2 µs of
    encode and present (200×60);
  - combined colors save 5% of truecolor bytes and 8% of 256-color bytes;
  - `Surface` adds no measurable cost.
- New tests: `tests/presenter_options.rs` (checked through a terminal screen
  model), `tests/surface.rs` (manual-pipeline equivalence, damage, and a
  randomized `source_damage` proof), and a `Surface` zero-allocation test.
  The core suite is 192 tests.
- Next: review, then M5 (see [architecture-roadmap.md](architecture-roadmap.md)).

### 2026-09-25: M3 performance, dithering, and Density (checkpoint)

- Added `examples/workload_benchmark.rs` (photo, pan, plasma, ui, readback,
  and band workloads at four terminal sizes, timed stage by stage). Results
  and method are in [benchmarks.md](benchmarks.md).
- The presenter emits identical bytes 3.5–4.6× faster, and its diff scan is
  about 20% faster. Nearest sampling is about 2× faster, and Bilinear no
  longer uses `i128` division. Area is bound by memory bandwidth and is
  unchanged. Byte counts are identical in every workload.
- Added `dither::ordered_4x4_region` so regions and bands dither exactly like
  whole surfaces.
- Added encoder requirements and the `Density` encoder. Monochrome
  presentation now writes colorless cells as-is.
- New tests: `tests/dither_regions.rs`, `tests/density_encoder.rs`, a
  partial-update ANSI golden recorded from M2b, presenter and Density
  allocation tests, and exactness tests for each sampling shortcut. The core
  suite is 166 tests.
- Next: review, then M4 (see [architecture-roadmap.md](architecture-roadmap.md)).

### 2026-09-25: M2b Renderer switch-over (checkpoint)

- `Renderer` now runs on the pipeline's single geometry and sampling
  implementation and the shared half-block cell rule. It streams one cell row
  at a time, so it still allocates nothing and cancels between rows exactly as
  before. Golden cells, golden bytes, cancellation goldens, and the 2,880-case
  differential test all pass unchanged.
- `calculate_transformed` delegates to `calculate_for_spec`; the original
  implementation is kept only as a test reference. The duplicated sampling
  entry points now share one `sample_point`.
- Added `ImageSource::render_region` (regions of a larger buffer, row bands,
  cancellation between bands, host threads), `CellViewMut::from_slice`, and
  `RenderError::Region`. The C ABI (`ffi.rs`, `trend.h`) is untouched.
- Benchmark gate: the legacy `Renderer` stays within ±10% of M2a for every
  filter and viewport. New tests: `tests/image_source_regions.rs`, plus
  allocation tests for the legacy renderer and banded rendering. The core
  suite is 146 tests.
- Next: review, then M3 (throughput presenter, resampler precomputation,
  `Density`, encoder metadata).

### 2026-09-25: M2a views and input layouts (checkpoint)

- Added borrowed-frame layouts (RGBA/BGRA, top-down/bottom-up, explicit
  straight/premultiplied/ignored alpha, padded strides), zero-copy
  exact-size encoding, pixel and cell sub-views, row bands, and region
  mapping. See the "Borrowed frames and layouts" section of
  [api.md](api.md).
- `Renderer` is not switched over. Its cells and bytes are unchanged (golden
  tests), and its resampling reads the canonical layout through a
  branch-free path.
- New contract tests: `tests/pixel_layout.rs`, `tests/sub_views.rs`, and
  `tests/zero_copy.rs` (no allocation when encoding exact-size readbacks,
  regions, or bands). The core suite is 135 tests.
- Next: review the M2a API, then M2b (the `Renderer` switch-over).

### 2026-09-25: M1 pipeline proof

- Decided that the renderer-architecture track (M1) precedes the Android video
  milestone. See [phase-0-plan.md](history/phase-0-plan.md).
- Recorded golden baselines of `Renderer` cells and `AnsiPresenter` bytes
  before any source change (`tests/golden_render.rs`, `tests/golden_ansi.rs`).
- Added `PixelBuffer`, `RasterSpec`, `ImageSource`, `CellEncoder`,
  `HalfBlock`, `Quadrant`, and cell views as additive APIs.
- `tests/pipeline_equivalence.rs`: the new half-block pipeline matches
  `Renderer` cell for cell and byte for byte across all 2,880 cases.
- Validated the core crate on Linux x64 (tests, clippy, fmt, rustdoc). Adapter
  and tool crates were not rebuilt in that environment because crates.io was
  unreachable; they do not reference the new APIs.
- Measured M1 per-stage costs and revised the M2–M5 order in
  [architecture-roadmap.md](architecture-roadmap.md). Next: M2 (views, input
  layout, and the `Renderer` switch-over).

### 2026-09-06: repository and release hardening

- Reorganized mixed test environments into `examples/`, `tools/`, `tests/`,
  `scripts/`, and `docs/`.
- Added viewer and bridge help/version output.
- Added first-render and terminal-overlay examples.
- Documented the bridge protocol, C ABI, packaging, platforms, and releases.
- Added deterministic Windows, Linux, and Android package manifests.
- Added one-command repository validation and tag-gated GitHub releases.
- Validated Windows and Linux media suites, Android ARM64 cross-build/package,
  and maintainer-reported Android image/animation runtime behavior.
- Created cleanup checkpoint commit `4691d41`.

## Next milestone: Android MP4/H.264

Implement Android video as a separate `trend-video` backend without changing
the renderer or desktop backends.

### Phase 1: backend foundation

- [ ] Audit the Android NDK Media APIs available at API 21 and choose the
  minimum API needed for extractor, codec, image, and output-buffer access.
- [ ] Add Android-only bindings or a narrowly scoped maintained crate.
- [ ] Add `android_backend.rs` behind `target_os = "android"`.
- [ ] Advertise only MP4/H.264 combinations the backend can actually open.
- [ ] Open a source and expose width, height, duration, frame rate when known,
  audio presence, and seek accuracy through existing public types.

### Phase 2: frame decoding

- [ ] Decode one software-accessible frame into the existing top-down RGBA8
  `VideoFrame` contract.
- [ ] Handle common YUV 4:2:0 layouts, row stride, pixel stride, crop rectangle,
  and color conversion without assuming tightly packed planes.
- [ ] Reuse bounded buffers and enforce `VideoLimits` before allocation.
- [ ] Return explicit unsupported errors when a codec only supports a Surface
  output path unavailable to the terminal renderer.

### Phase 3: playback and seeking

- [ ] Implement timestamp-driven frame selection and end-of-stream handling.
- [ ] Implement keyframe-then-forward seeking with documented accuracy.
- [ ] Preserve decoder epochs, timestamps, cancellation expectations, and the
  current borrowed-frame lifetime model.
- [ ] Exercise playback through `trend-media`, viewer, and bridge paths.

### Phase 4: device validation

- [ ] Add redistributable Android MP4/H.264 fixtures with provenance.
- [ ] Test API-level and device-codec capability reporting.
- [ ] Test malformed/truncated input, resize, seek, repeated open/close, and
  memory limits on a real ARM64 device.
- [ ] Record tested devices, Android versions, codec names, and outcomes in the
  Android platform guide.
- [ ] Update package docs and capability tables only after runtime validation.

## Later refinements

- [x] Add a compiled C/C++ consumer smoke test for `include/trend.h` (M6).
- [ ] Establish benchmark baselines for render filters, decoder throughput, ANSI
  generation, and bounded memory behavior. (Render filters, encoders, and
  ANSI generation are covered by `docs/benchmarks.md` since M3–M6; decoder
  throughput is not.)
- [x] Add convenience builders only where they reduce repeated integration code
  without hiding ownership or allocation (M4 `Surface`).
- [ ] Resolve the provenance of the untracked hash-named GIF.
- [ ] Review the `0.1.0` changelog and push `v0.1.0` only when the maintainer is
  ready to publish the generated release assets.

## Updating this note

After a work session:

1. Move completed tasks out of the active phase or mark them complete.
2. Add a dated event summarizing behavior changes and validation performed.
3. Update capability states only with evidence.
4. Record new blockers and exact reproduction commands.
5. Keep this file concise; link to detailed design documents rather than
   duplicating them here.
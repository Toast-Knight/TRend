# Benchmarks

How TRend's performance is measured, and the results that shaped M3–M6 and
the work after it.

## Harnesses

| Example | Measures |
| --- | --- |
| `examples/render_benchmark.rs` | The legacy `Renderer` against the staged pipeline (`ImageSource` + dither + encoder) on one 1920×1080 image. This is the regression gate for the existing renderer. |
| `examples/workload_benchmark.rs` | Realistic workloads, timed stage by stage (see below). |
| `examples/frame_budget.rs` | M6: whole frames against the 30-FPS budget, per stage, for every encoder, with bytes, output rate, and allocations counted by a global allocator. |

```text
cargo run --release --example render_benchmark -- [iterations]
cargo run --release --example workload_benchmark -- [frames]
cargo run --release --example frame_budget -- [frames] [--colors truecolor|256|16] [--compact]
```

`workload_benchmark` scenes:

| Scene | What it models |
| --- | --- |
| `photo` | A static 1920×1080 photograph with grain, resampled every frame (Nearest, Bilinear, Area) |
| `pan` | The same photo zoomed and panned, like video: most cells change every frame |
| `plasma` | A procedural effect drawn natively at the encoder's resolution: every cell changes |
| `ui` | A mostly static interface with one small moving sprite |
| `readback` | A BGRA, bottom-up, premultiplied, 256-byte-aligned frame: zero-copy encode vs resampling at 1:1 |
| `bands` | Area resampling whole, in 8-row bands, and in four host-thread bands |
| `output` | M4 output options on full-change plasma frames (truecolor and 256 colors) |
| `surface` | `Surface` against the same steps by hand, and a mostly static UI presented as full frames or as damage regions |

Stages are `source` (resampling or the app's own drawing), `encode`, `diff`
(presenting an unchanged frame: the diff scan alone), and `present` (diff plus
ANSI serialization). Each row also reports bytes per frame. Terminal sizes are
80×24, 120×40, 200×60, and 320×90 cells.

## Environment and method

M3 numbers come from a Linux x64 cloud container with 2 vCPUs (Xeon, 2.1 GHz),
release builds.

- Before/after comparisons alternate the two builds and report the median of 5
  runs (20 frames each for workloads, 40 iterations for `render_benchmark`).
- Individual workload rows move by ±10–15% between runs on this machine. Any
  row that looked regressed was re-checked with a dedicated best-of-40 A/B
  test.
- Byte counts are exact and deterministic.
- Terminal throughput itself is not measured, because there's no real
  terminal in the loop.

## M3 results (M2b → M3)

**Every workload produced byte-for-byte identical output before and after.**
The golden ANSI tests, including a new partial-update golden recorded from
M2b, pass unchanged.

### Legacy `Renderer` (`render_benchmark`, µs per frame)

| Filter | 80×24 | 200×60 |
| --- | --- | --- |
| Nearest | 145 → 70 (0.48×) | 916 → 433 (0.47×) |
| Bilinear | 230 → 232 (1.01×) | 1480 → 1450 (0.98×) |
| Area | 3300 → 3340 (1.01×) | 4830 → 4720 (0.98×) |

### Presentation, truecolor (µs per frame)

| Workload | Cells | `present` | `diff` only | Bytes/frame |
| --- | --- | --- | --- | --- |
| plasma (every cell changes) | 80×24 | 405 → 89 | 9 → 6 | 73,025 |
| plasma | 200×60 | 2403 → 558 | 54 → 42 | 468,389 |
| plasma | 320×90 | 5860 → 1397 | 131 → 106 | 1,130,102 |
| plasma, 256 colors | 200×60 | 1467 → 431 | 54 → 42 | 293,629 |
| pan (video-like) | 200×60 | 2632 → 566 | 54 → 41 | 469,938 |
| photo, static after first frame | 200×60 | 185 → 89 | 57 → 42 | 22,027 |
| ui, mostly static | 200×60 | 77 → 53 | 54 → 42 | 3,835 |

### Resampling and zero-copy input (µs per frame, 200×60 half-block)

| Workload | Before | After |
| --- | --- | --- |
| photo, Nearest (`ImageSource`) | 611 | 196 |
| photo, Bilinear | 1211 | 1200 |
| photo, Area | 4918 | 4708 |
| readback, zero-copy encode | 291 | 288 |
| readback, resampled at 1:1 then encoded | 993 + 127 | 596 + 125 |

### `Density` (new in M3; µs per frame and bytes, 200×60)

| Workload | Source | Encode | Present | Bytes |
| --- | --- | --- | --- | --- |
| photo, Nearest | 98 | 92 | 49 | 621 |
| pan, Bilinear | 664 | 98 | 140 | 20,668 |
| plasma, native | 303 | 99 | 90 | 9,314 |

Colorless density output is small because no color codes are needed.

## M4 results (M3 → M4)

Same machine and method. The default presenter output is byte-identical to
M3 in every workload, and the legacy `Renderer` is unchanged (the
`render_benchmark` runs are within noise, with identical ANSI bytes).

### Presenter cost of statistics and options (µs per frame, 200×60)

A dedicated best-of-200 A/B microbenchmark against the M3 build, over
pseudo-random half-block frames:

| Workload | M3 | M4 |
| --- | --- | --- |
| truecolor, every cell changes | 555–565 | 565–600 |
| 256 colors, every cell changes | 412 | 403–413 |
| unchanged frame (diff scan) | 41.3 | 37.2 |
| sparse changes (1 in 97 cells) | 48.3 | 44.5 |

Per-frame statistics first cost 4% and a larger stack buffer another 10% on
full-change truecolor frames. Counting in locals and sizing the buffer per
sequence brought this down to 0–5%, within this machine's noise band. The
scan of unchanged cells is about 10% faster than M3.

### Output options (plasma, every cell changes)

| Cells | Mode | Default bytes | Combined colors | Synchronized |
| --- | --- | --- | --- | --- |
| 80×24 | truecolor | 73,025 | 69,203 (−5.2%) | +16 bytes |
| 200×60 | truecolor | 468,389 | 444,557 (−5.1%) | +16 bytes |
| 200×60 | 256 colors | 293,629 | 269,797 (−8.1%) | +16 bytes |
| 320×90 | truecolor | 1,130,102 | 1,072,855 (−5.1%) | +16 bytes |

Combined colors cost about the same serialization CPU (corrected in M5: the
15–20% saving first reported here came from a noisy mean-of-20 row; a
best-of-300 A/B measures −9% to +3%). Synchronized updates cost only
their 16 bytes; they don't make the terminal draw faster, but they remove
tearing where supported.

### `Surface` and damage regions (µs per frame)

| Workload | Cells | Encode | Present | Bytes |
| --- | --- | --- | --- | --- |
| plasma, `Surface::draw` + `present` | 200×60 | (in draw) 653 | 537 | 468,393 |
| plasma, same steps by hand | 200×60 | 653 | 604 | 468,393 |
| ui, full frames | 200×60 | 124 | 41 | 515 |
| ui, damage regions | 200×60 | 0.8 | 0.9 | 515 |
| ui, full frames | 320×90 | 303 | 95 | 515 |
| ui, damage regions | 320×90 | 1.5 | 1.0 | 515 |

`Surface` adds no measurable cost; the present difference is run-to-run
noise. Damage regions write the same bytes as full frames but skip encoding
and scanning unchanged cells: about 100× less CPU for a small moving sprite.

### Found for M5: redundant color sequences in indexed modes

The presenter tracks the pen by RGB color. In 256- and 16-color modes, many
neighboring colors map to the same index, so it re-sends a sequence the
terminal already has. For a smooth photograph at 200×60:

- 82% of 256-color bytes are such no-op sequences;
- 75% of 16-color bytes are.

Tracking the pen by the emitted index would shrink indexed output several
times over. It changes the default bytes, so it's an M5 decision, not an M4
option.

## M5 results (M4 → M5)

Same machine and method. Byte counts match M4 in every shared workload, and
`render_benchmark` shows the legacy renderer unchanged within noise.

`Quadrant` and `Density` goldens were recorded from M4 before any change
(`tests/golden_encoders.rs`). `Quadrant`'s rewrite keeps its cells
identical: it passes those goldens and a 300,000-cell comparison against the
original code.

### Encoding (µs per frame, 200×60, repository release profile)

| Workload | M4 Quadrant | M5 Quadrant | Sextant | Braille | HalfBlock |
| --- | --- | --- | --- | --- | --- |
| photo, Bilinear | 3866 | 1338 | 1835 | 2717 | 139 |
| photo, Area | 4610 | 1294–1616 | 1755 | 2510–2971 | 135 |
| pan (video-like) | 4042 | 1460 | 1965–2019 | 2895–2964 | 138 |
| plasma, native | 3910 | 1329 | 1586–1598 | 2080–2154 | 132 |
| ui, native | 3759 | 1157 | — | — | 131 |

- **Where Quadrant's time went.** 85% was fitting, not reading pixels. The
  rewrite keeps the exact criterion (squared error against each part's
  rounded mean, first minimum wins) but computes it from sums:
  - pairs use a closed form;
  - popcounts come from a table, since the baseline x86-64 target has no
    popcount instruction;
  - rounded division multiplies by a reciprocal, tested exact for every
    block sum.
- **Choosing the fitter for larger blocks.** An exhaustive search over the
  128 splits of a 2×4 block took 541–607 ns per cell. Candidates were
  measured on the project's photo fixtures and a noise image, against the
  best rounded-mean split:

  | Fitter | 2×2 | 2×3 | 2×4 |
  | --- | --- | --- | --- |
  | Exhaustive (exact-mean score) | +0.045% | +0.014% | +0.012% |
  | Luma projection | +93% | +22% | +33% |
  | Farthest-pair projection | +0.043% | +0.028% | +0.023% |
  | Two-pass projection (kept) | — | +0.032% | +0.024% |

  Luma fails on colors of equal brightness. Projecting onto the axis
  between two far-apart colors is near-optimal, and finding them in two
  passes instead of all pairs cut Braille's encode by 16%.
- **Optimization level.** The repository's release profile is
  `opt-level = "s"`. At `opt-level = 3`, the fitters run about 2× faster
  (Sextant 1.0 ms, Braille 1.15 ms, Quadrant 0.65 ms at 200×60). Crates
  that depend on TRend use their own profile.
- **Exit bound for M5.** Quadrant encode at 200×60 is at most 1.7 ms under
  the release profile on this machine, 3× under M4.

Sextant output is 2–3% larger than Quadrant's (4-byte UTF-8 glyphs).
Braille's is about the same as HalfBlock's.

### Presentation

| Workload (µs per frame, 200×60) | M3 | M4 | M5 |
| --- | --- | --- | --- |
| truecolor, every cell changes (best of 300) | 608–617 | 595–613 | 631–637 |
| unchanged frame (diff scan) | 49.6 | 45.3 | 45.3 |
| 256 colors, every cell changes | 412 | 380–389 | 380–388 |

Attribute output costs 4–5% on full-change truecolor frames, one compare per
changed cell. It first measured 9%; keeping the check in the emit loop
with a local copy of the pen, and moving the attribute writer out of line,
brought it down. That is 3–4% over M3 cumulatively, inside the 10% gate.

### Compact indexed colors (opt-in)

| Workload | Default | Compact | Smaller by |
| --- | --- | --- | --- |
| smooth photo, 256 colors | 283,234 B | 51,407 B | 5.5× |
| smooth photo, 16 colors | 156,661 B | 39,594 B | 4.0× |
| plasma, 256 colors | 293,629 B | 100,247 B | 2.9× |
| plasma, 16 colors | 163,941 B | 46,758 B | 3.5× |

Presenting also gets faster (photo, 256 colors: 0.66 → 0.28 ms), because far
fewer bytes are formatted. Screen-model tests confirm every mode shows the
same screen as the default.

## M6 results: the 30-FPS budget

M6 asked where a real frame's time goes before optimizing anything.
`examples/frame_budget.rs` runs four workloads at 200×60 and 320×90 cells
with every encoder, and reports per-stage medians against 33.3 ms (30 fps):

| Workload | What it models | Stages |
| --- | --- | --- |
| `video` | A 1920×1080 frame, `Cover`, Bilinear, zoomed 115% and panned every frame | resample, encode, present |
| `photo 1st` | A static 1920×1080 photo, Area filter: the first frame | resample, encode, present |
| `photo` | The same photo on later frames: nothing changes | present (diff only) |
| `scene3d` | The application draws a 3D-like scene into `Surface` pixels | app, encode, present |
| `readback` | The same scene as a BGRA, bottom-up, premultiplied, 256-byte-aligned GPU readback, read in place by `encode_from` | app, encode, present |

"TRend" is resample + encode + present, the part TRend owns. The
application's own drawing (`app`) and the terminal's write are not
included: the first is the host's, and the second can't be measured in
this container (see section "Not measured" below). Same machine as M3–M5,
release profile (`opt-level = "s"`), best of two runs of 30 frames.

### Where the time goes (µs per frame, truecolor)

| Workload | Cells | Encoder | Resample | Encode | Present | TRend | Of 33.3 ms | Bytes/frame | At 30 fps |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| video | 200×60 | HalfBlock | 517 | 22 | 484 | 1024 | 3.1% | 433,202 | 13.0 MB/s |
| video | 200×60 | Quadrant | 1017 | 1516 | 496 | 3030 | 9.1% | 440,508 | 13.2 MB/s |
| video | 200×60 | Sextant | 1541 | 2183 | 528 | 4255 | 12.8% | 453,692 | 13.6 MB/s |
| video | 200×60 | Braille | 2056 | 3193 | 517 | 5766 | 17.3% | 439,169 | 13.2 MB/s |
| video | 200×60 | Density | 254 | 26 | 99 | 379 | 1.1% | 12,762 | 0.4 MB/s |
| video | 320×90 | HalfBlock | 1244 | 53 | 1176 | 2472 | 7.4% | 1,036,524 | 31.1 MB/s |
| video | 320×90 | Quadrant | 2465 | 3669 | 1193 | 7327 | 22.0% | 1,051,128 | 31.5 MB/s |
| video | 320×90 | Sextant | 3711 | 5208 | 1225 | 10144 | 30.4% | 1,082,238 | 32.5 MB/s |
| video | 320×90 | Braille | 4993 | 7715 | 1259 | 14059 | 42.2% | 1,050,216 | 31.5 MB/s |
| video | 320×90 | Density | 616 | 62 | 248 | 927 | 2.8% | 30,966 | 0.9 MB/s |
| scene3d | 200×60 | HalfBlock | — | 22 | 139 | 161 | 0.5% | 68,428 | 2.1 MB/s |
| scene3d | 200×60 | Braille | — | 1988 | 193 | 2181 | 6.5% | 107,805 | 3.2 MB/s |
| scene3d | 320×90 | Braille | — | 4677 | 453 | 5132 | 15.4% | 242,058 | 7.3 MB/s |
| readback | 200×60 | HalfBlock | — | 54 | 137 | 191 | 0.6% | 68,428 | 2.1 MB/s |
| readback | 200×60 | Braille | — | 2070 | 198 | 2269 | 6.8% | 107,805 | 3.2 MB/s |
| readback | 320×90 | Braille | — | 4926 | 445 | 5387 | 16.2% | 242,058 | 7.3 MB/s |
| photo 1st | 200×60 | HalfBlock | 4567 | — | 490 | 5058 | 15.2% | 345,816 | once |
| photo 1st | 320×90 | Braille | 18405 | — | 1199 | 19820 | 59.5% | 1,010,212 | once |
| photo, later frames | any | any | — | — | 37–91 | 37–91 | 0.1–0.3% | 0 | — |

Every workload allocates nothing per frame. Findings:

- **Every encoder fits the budget at both sizes** in steady state, with
  at least 57% of the frame left for the application and the terminal.
  The most expensive steady case is Braille video at 320×90, 14.1 ms.
- **Resampling and block fitting dominate video.** With Quadrant, Sextant,
  and Braille, encoding is 40–55% of TRend's time and resampling 30–40%.
  HalfBlock and Density spend almost nothing encoding: HalfBlock's time
  splits evenly between resampling and presentation, and Density's is
  two-thirds resampling.
- **Output rate, not CPU, is the likely limit for full-motion truecolor
  video:** 13 MB/s at 200×60 and 31–33 MB/s at 320×90. Whether a terminal
  sustains that is a property of the terminal. Native 3D-like scenes with
  flat shading write 2–7 MB/s.
- **Reading a foreign layout in place is cheap.** `readback` (BGRA,
  bottom-up, premultiplied, padded) costs 30–255 µs more per frame than
  `scene3d` (straight RGBA in TRend's own buffer), from swizzling and
  unpremultiplying each pixel: 3–5% more for the block encoders, and a
  larger share of HalfBlock's and Density's otherwise tiny encode.
- **Area on a large static image costs one frame, once:** up to 18 ms at
  320×90 Braille, then 0.1–0.3% per unchanged frame. Hosts that show a
  static image should encode it once, as the harness does.

### What changed from M5 (TRend µs per frame, best of two, same bytes)

The first M6 profile found two real bottlenecks, both in reading pixels
rather than in the algorithms:

1. **Bilinear resampling** recomputed the vertical interpolation for every
   pixel and went through the general per-pixel sampler. It now computes
   the vertical axis once per row, and blends four opaque corners directly
   (the general path remains for translucent pixels). A test compares the
   row path with the per-pixel sampler across alpha styles, layouts,
   geometries, and mattes.
2. **Row reading in encoders and `blend_over`** went through the checked,
   layout-generic `PixelFrameView::rgba` per pixel. Encoders now borrow each
   row once and decode it with a decoder specialized per layout (six,
   selected once per frame). Unpremultiplying skips the division for opaque
   pixels, which is exact. Tests compare each decoder with `rgba` for every
   layout and value class.

| Workload | Cells | Encoder | Resample | Encode | TRend | Change |
| --- | --- | --- | --- | --- | --- | --- |
| video | 200×60 | HalfBlock | 1273 → 517 | 121 → 22 | 1889 → 1024 | −46% |
| video | 200×60 | Braille | 5014 → 2056 | 3230 → 3193 | 8768 → 5766 | −34% |
| video | 320×90 | Sextant | 9083 → 3711 | 5251 → 5208 | 15597 → 10144 | −35% |
| video | 320×90 | Braille | 12195 → 4993 | 7814 → 7715 | 21288 → 14059 | −34% |
| video | 200×60 | Density | 869 → 254 | 144 → 26 | 1164 → 379 | −67% |
| readback | 200×60 | HalfBlock | — | 290 → 54 | 429 → 191 | −55% |
| readback | 200×60 | Sextant | — | 2323 → 1527 | 2510 → 1718 | −32% |
| readback | 320×90 | Braille | — | 7402 → 4926 | 7844 → 5387 | −31% |
| scene3d | 200×60 | Quadrant | — | 1333 → 1323 | 1490 → 1485 | 0% |
| scene3d | 320×90 | Braille | — | 4942 → 4677 | 5394 → 5132 | −5% |

No workload regressed. Presentation is unchanged within noise: an
interleaved A/B against M5 puts every mode's mean within ±1.5%. The
block fitters themselves were not touched: at `opt-level = "s"` they are
the largest remaining cost, and the next table shows what the optimization
level buys.

### `opt-level = "s"` vs `opt-level = 3`

Same code, built with `CARGO_PROFILE_RELEASE_OPT_LEVEL=3`, best of two:

| Workload | Cells | Encoder | Encode s → 3 | TRend s → 3 | Change |
| --- | --- | --- | --- | --- | --- |
| video | 200×60 | HalfBlock | 22 → 18 | 1024 → 1034 | +1% (noise) |
| video | 200×60 | Quadrant | 1516 → 924 | 3030 → 2432 | −20% |
| video | 200×60 | Sextant | 2183 → 1666 | 4255 → 3668 | −14% |
| video | 200×60 | Braille | 3193 → 2511 | 5766 → 5007 | −13% |
| scene3d | 200×60 | Quadrant | 1323 → 685 | 1485 → 853 | −43% |
| readback | 200×60 | Sextant | 1527 → 1038 | 1718 → 1237 | −28% |
| readback | 200×60 | Braille | 2070 → 1406 | 2269 → 1612 | −29% |
| video | 320×90 | Braille | 7715 → 6030 | 14059 → 12001 | −15% |
| readback | 320×90 | Braille | 4926 → 3374 | 5387 → 3845 | −29% |
| scene3d | 320×90 | Density | 61 → 91 | 191 → 223 | +17% |

| Artifact | `"s"` | `3` | Change |
| --- | --- | --- | --- |
| `libtrend.so` (C ABI) | 319,520 B | 320,872 B | +0.4% |
| `trend-ui-lab` | 317,560 B | 331,504 B | +4% |
| `trend-test-viewer` | 1,236,488 B | 1,463,160 B | +18% |

Level 3 makes the block encoders 13–43% faster per frame, leaves
HalfBlock and resampling unchanged, and makes Density's tiny native encode
slower (26 → 38 µs at 200×60). The size cost is negligible for the library
and 18% for the media viewer, which links codecs. This is a packaging
decision rather than a code change, so the profile is unchanged; M6
recommends `opt-level = 3` for the library and the tools. Applications
that depend on TRend as a crate already use their own profile.

### Compact indexed colors (256 and 16 colors)

M6 first found that in 16-color mode compact output cost 26–42% more CPU
on flat-shaded scenes: the 16-entry nearest-color search ran for each
written cell's colors to build the pen key, and again to write the
sequence. Now the sequence is written from the entry already found, and
the last 16-color lookup for each of foreground and background is
remembered (exact: the entry depends only on the color). The same cache
cost 2–15% in 256-color mode, whose index is a little arithmetic, so it
applies to 16 colors only. Output is byte-identical to M5 in all 16
combinations of mode and options.

Present µs (best of three) and bytes per frame, default vs compact:

| Workload | Cells | Encoder | 256: bytes | 256: CPU | 16: bytes | 16: CPU |
| --- | --- | --- | --- | --- | --- | --- |
| video | 200×60 | HalfBlock | 280,382 → 73,805 (3.8×) | 419 → 259 | 156,398 → 57,366 (2.7×) | 960 → 843 |
| video | 200×60 | Braille | 283,746 → 73,285 (3.9×) | 442 → 249 | 158,094 → 56,462 (2.8×) | 943 → 855 |
| scene3d | 200×60 | Sextant | 69,103 → 33,308 (2.1×) | 175 → 147 | 43,484 → 25,052 (1.7×) | 282 → 266 |
| readback | 200×60 | Braille | 74,834 → 34,915 (2.1×) | 184 → 154 | 45,380 → 25,301 (1.8×) | 312 → 297 |
| video | 320×90 | Sextant | 708,729 → 203,516 (3.5×) | 1056 → 626 | 406,606 → 163,727 (2.5×) | 2358 → 2066 |
| scene3d | 320×90 | Braille | 168,632 → 76,757 (2.2×) | 421 → 350 | 103,040 → 58,429 (1.8×) | 693 → 665 |

Across all block-encoder workloads: 256 colors 2.1–5.4× fewer bytes and
9–57% less CPU; 16 colors 1.7–3.4× fewer bytes and 3–15% less CPU. Density
output has no color changes to skip and is unchanged. See
[api.md](api.md#ansi-presentation) for the decision on the default.

The 16-color default path costs about twice the 256-color one (960 vs
419 µs for 200×60 video) because of that nearest-color search. At 7% of the
budget in the worst case it isn't worth changing yet; 16-color terminals
are rare.

### Not measured

- **The terminal.** There is no terminal in this container. The
  `external-renderer` example times `write_all` + `flush` per frame; on a
  pipe it is 0.04 ms for 9 KB. Run it in real terminals at 200×60 and
  320×90 to find the throughput limit the output-rate figures above
  point at.
- **Other CPUs.** All numbers are from 2 vCPUs of a 2.1 GHz Xeon. Relative
  results (which stage dominates, s vs 3) should carry over; absolute
  ones won't.

## After M6: a real WebGL renderer at large sizes

A private test harness (an existing Three.js renderer, read back from its
WebGL render target and fed to `Surface`; not part of TRend) ran HalfBlock at
30+ FPS, Quadrant at about 25, and Sextant at 15–20 in a terminal about 400
columns wide. Profiling its real frames (640×480, RGBA8, bottom-up) showed
that **resampling, not fitting, dominated**. At 400×120 Sextant, resampling
took 14 ms and encoding 3.7 ms:

- Area and Bilinear read each source pixel through the layout-generic
  `PixelFrameView::rgba`, since M6's row decoders only reached the encoders.
- Each output pixel recomputed its column position with 64-bit divisions,
  although it is the same for every row.
- At that size the image is shrunk only slightly, so Area's box covers
  about 2×2 source pixels. That is little arithmetic but many reads.

Changes, all with identical pixels (per-pixel exactness tests for both row
paths, and a 1,620-case M6-versus-now comparison of pixels and ANSI bytes
over real captured WebGL frames and synthetic frames in every layout):

1. **Row resampling.** `area_row` is new and `bilinear_row` reworked. Both
   borrow source rows once and decode them with the per-layout readers.
   They step column positions with an exact quotient/remainder walk instead
   of dividing, and skip letterbox columns up front.
2. **Uniform blocks.** Both fitters return single-color blocks immediately,
   with the same split they found before. Over half of a dark rendered
   scene is flat.
3. **Exact reciprocal division** in the fitters' color means (the table was
   already tested exact for every block sum).
4. **Opt-in parallel bands.** `Surface::encode_image_parallel` and
   `encode_from_parallel` split resampling and encoding into cell-row bands
   on scoped threads. The output is identical (`tests/parallel.rs`).

Captured WebGL frames (a dark 3D scene), `Surface`-equivalent steps, medians in ms (standalone
build at `opt-level = 3`; best of two):

| Size | Encoder | Filter | Resample | Encode | Present | Total |
| --- | --- | --- | --- | --- | --- | --- |
| 200×60 | HalfBlock | Area | 2.15 → 0.72 | 0.02 | 0.23 | 2.40 → 0.96 |
| 200×60 | Quadrant | Area | 2.65 → 1.00 | 0.57 → 0.38 | 0.24 | 3.46 → 1.62 |
| 200×60 | Sextant | Area | 3.83 → 1.61 | 0.95 → 0.73 | 0.25 | 5.03 → 2.62 |
| 400×120 | HalfBlock | Area | 3.62 → 1.38 | 0.07 | 0.89 → 0.84 | 4.58 → 2.29 |
| 400×120 | Quadrant | Area | 8.30 → 1.51 | 2.32 → 1.53 | 0.89 | 11.51 → 3.96 |
| 400×120 | Quadrant | Bilinear | 4.34 → 1.38 | 2.34 → 1.51 | 0.88 | 7.56 → 3.77 |
| 400×120 | Sextant | Area | 12.37 → 2.01 | 3.66 → 2.61 | 0.90 → 0.87 | 16.93 → 5.49 |
| 400×120 | Sextant | Bilinear | 6.87 → 2.02 | 3.70 → 2.63 | 0.89 | 11.46 → 5.53 |

Area now costs the same as Bilinear at these sizes, so the higher-quality
filter stays the default.

With `encode_image_parallel` on this 2-vCPU machine, the whole `Surface`
path (including presentation, which stays on one thread) at 400×120 went
from 5.68 to 3.72 ms for Sextant, 3.83 to 2.72 for Quadrant, and 2.35 to
1.83 for HalfBlock. Resampling and encoding scale with cores; presentation
(about 0.9 ms at 400×120) does not.

`frame_budget` at the repository's `opt-level = "s"` (TRend µs per frame,
best of two, bytes identical):

| Workload | Cells | Encoder | Resample | TRend | Change |
| --- | --- | --- | --- | --- | --- |
| video | 200×60 | HalfBlock | 520 → 234 | 1026 → 752 | −27% |
| video | 200×60 | Sextant | 1544 → 684 | 4230 → 3408 | −19% |
| video | 320×90 | Quadrant | 2449 → 1053 | 7296 → 5748 | −21% |
| video | 320×90 | Braille | 4970 → 2210 | 13894 → 11147 | −20% |
| video | 320×90 | Density | 617 → 268 | 927 → 574 | −38% |
| photo 1st | 320×90 | Sextant | 14345 → 11143 | 15534 → 12338 | −21% |
| readback | 320×90 | Sextant | — | 4125 → 3719 | −10% |
| readback | 200×60 | HalfBlock | — | 192 → 193 | +1% (noise) |

Every other row is within −38% to +5%; the positive ones are noise on
paths that didn't change (`encode_from` with HalfBlock or Density).

For hosts, the matching pattern is to overlap the stages: read back the
next frame while TRend encodes this one, and write the previous frame's
bytes on another thread. The frame time is then the slowest stage, not the
sum.

## What the measurements showed

- **Presentation was dominated by formatting, not diffing.** `write!` and
  `core::fmt` accounted for about 60% of instructions when presenting a
  full-change frame. Hand-assembled escape sequences with identical bytes cut
  presentation CPU by 3.5–4.6×. A row-slice diff scan cut the unchanged-frame
  scan by about 20–25%.
- **Bytes, not CPU, are now the real-time limit.** In a full-change truecolor
  frame:
  - 92% of the bytes are color SGR sequences;
  - cursor moves are 0.1%;
  - combining foreground and background into one SGR would save about 8%;
  - 256-color output is 37% smaller than truecolor.

  Larger savings need lossy choices, such as color tolerance or temporal
  thresholds. Those are an M4+ decision, not an optimization.
- **Area is bound by memory, not arithmetic.** It reads every source pixel
  once. A single linear pass over the 8 MB source costs 1.5–2.4 ms on this
  machine, and Area costs about 3.3 ms. A tighter row-slice inner loop cut
  instructions by about 45% but didn't change wall time, so it was not kept.
  The remedy for static images is host caching (render once, reuse the
  cells), not faster arithmetic.
- **Nearest and Bilinear were bound by division.** Nearest spent most of its
  time in per-channel `u64` divisions (plus one for coverage) that resolve a
  single pixel back to the color it already had. Bilinear spent most of its time in an `i128` division
  (a library call). Exact shortcuts removed both, and property tests prove the
  shortcuts are identical to the originals.
- **Quadrant encoding is expensive**: 3.5 ms at 200×60, from its exhaustive
  two-color partition search. It was not changed in M3.
- **Host threads barely help here.** With only 2 vCPUs and a memory-bound
  filter, four-thread Area bands gain about 25%.

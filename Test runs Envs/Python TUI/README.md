# Terminal Video Integration Laboratory

This is a small YouTube-like terminal application that consumes TRend. It is
an example and compatibility test, not TRend core or a reusable UI dependency.
The Python side uses only the standard library.

## Run

From the workspace root:

```text
python "Test runs Envs/Python TUI/trend_tui.py"
```

The script builds `trend-python-tui-bridge` when its debug executable is absent
or older than the bridge source. A packaged example would ship that executable
and would not require Rust on the user's machine.

The full grid requires at least 72x22 terminal cells. Smaller terminals show a
bounded fallback until resized.

Controls:

- Arrow keys: move through the 2x2 video grid; selection is terminal hover and
  starts a three-second looping preview.
- Enter: reuse the selected session for full-range terminal playback.
- Escape: leave full playback, stop it, and restore the thumbnail grid.
- Space: stop preview and release its bridge/decoder.
- `1`/`2`/`3`: FAST/BALANCED/QUALITY rendering.
- `C`: cycle truecolor, 256 color, ANSI16, and monochrome.
- `U`: switch Unicode and ASCII glyph output.
- `Q`: quit.

Explicit terminal policy can override conservative environment detection:

```text
python "Test runs Envs/Python TUI/trend_tui.py" --color 256 --glyphs ascii
```

## Architecture

The Python application owns catalog data, thumbnails, keyboard input, 2D
selection, layout, terminal size, capability policy, scheduling, generation
tokens, preview lifetime, and the preview rectangle.

The separate Rust bridge owns no UI. It uses the optional `trend-media`
`MediaSource` and `PreviewSession` layer above the existing image, animation,
and backend-neutral `VideoDecoder` adapters; lends decoded RGBA8 to TRend; and
returns `AnsiPresenter::present_at()` output for an application-owned origin.
On Windows, `VideoDecoder` selects Media Foundation. The application does not
import or name that backend.

Two application workers keep input responsive:

- the preview worker owns one active bridge/decoder and publishes frames tagged
  with the latest selection/resize generation;
- the thumbnail worker retains a bounded four-entry source cache and renders
  representative MP4 frame 0 through TRend independently of preview playback.

The UI accepts a result only when its generation is current. A selection change
immediately blanks only the preview rectangle, invalidates the old generation,
and opens the selected MP4. Old work may finish, but cannot overwrite a newer
selection. The one-slot request queues converge on the newest pending work.

Terminal color state is an explicit boundary. Each positioned TRend payload
ends with `SGR 0` and invalidates the presenter's cached foreground/background,
so the next incremental frame reestablishes every color it needs. Every
application text write begins with `SGR 0;39;49`, making default foreground and
background an application policy instead of inheriting preview state.

Resize advances the generation and sends a new viewport to the existing preview
bridge. Because the media path is unchanged, the worker retains its decoder and
rerenders decoded RGBA8 at the new dimensions. The application redraws its shell
on a geometry change; normal video frames update only the positioned preview.

## Repeatable Checks

```text
python -m unittest discover -s "Test runs Envs/Python TUI" -p "test_*.py" -v
python "Test runs Envs/Python TUI/trend_tui.py" --probe --size 80x24 --color truecolor --glyphs unicode
python "Test runs Envs/Python TUI/trend_tui.py" --probe --size 120x40 --color mono --glyphs ascii
python "Test runs Envs/Python TUI/trend_tui.py" --stress --color 16 --glyphs ascii
python "Test runs Envs/Python TUI/trend_tui.py" --compatibility
python "Test runs Envs/Python TUI/trend_tui.py" --benchmark --color truecolor --glyphs unicode
python "Test runs Envs/Python TUI/trend_tui.py" --workflow
python "Test runs Envs/Python TUI/trend_tui.py" --seek-demo --color truecolor --glyphs unicode
```

`--probe` treats unavailable required formats as failures and renders PNG,
JPEG, GIF, MP4, and all four catalog thumbnails. `--stress` performs the exact
1 -> 2 -> 3 -> 4 -> 1 sequence. `--compatibility` compares two fresh MP4
renders byte-for-byte for every color/glyph combination and verifies that WebP
remains unsupported. `--workflow` uses one bridge PID for representative frame,
short preview, resize, full-range playback, and thumbnail restoration.
`--seek-demo` requests metadata, seeks to an application-selected 83-second
master-clock position, requests the timed frame, and renders it at row 5,
column 20 in a 60x20 viewport. It reports requested versus actual presentation
time plus seek, acquisition, render, ANSI, and mock-write timings. The bridge
also accepts `META`, `SEEK <timestamp_ns>`, and
`FRAME_AT ... <timestamp_ns>` commands; this remains a test protocol rather
than a stable ABI.

## Windows x64 Baseline

Measured 2026-09-03 with Python 3.10.6, Rust 1.98.1, a debug bridge, the long
500x500 H.264 fixture, BALANCED rendering, and an asynchronous two-second run:

| Measurement | Result |
| --- | ---: |
| Preview output rate | 6.5 FPS |
| Decode p50 / p95 | 51.52 / 56.32 ms |
| TRend render p50 / p95 | 0.44 / 0.58 ms |
| Positioned ANSI median | 9,869 bytes |
| UI-loop p95 | 16.00 ms |
| Decoder-reported dropped frames | 44 |
| Adapter retained RGBA storage | 3,279,360 bytes |
| Bridge peak working set | 52.3 MiB |
| Python application working set | 18.9 MiB |
| Resize response, 80x24 to 120x40 | 47.00 ms |
| Decoded count across resize | 58 -> 62, same live decoder |
| Rapid 1 -> 2 -> 3 -> 4 -> 1 convergence | 125.00 ms |

The low debug preview rate and dropped-frame count are baseline findings, not
hidden failures. Decode dominates render time. The application input loop stays
separate and remained below one 16.7 ms display interval at p95. Re-run
`--benchmark` for current Python RSS and machine-specific values.

Initial `ColorTest.mp4` at 80x24 measured 28.07 ms decode, 0.41 ms TRend render,
7,605 ANSI bytes, and 36.4 MiB bridge working set. Thumbnail render times at
that size were 0.04-0.07 ms.

## Compatibility Result

Fresh MP4 renders were deterministic in all eight combinations:

| Color | Unicode bytes | ASCII bytes |
| --- | ---: | ---: |
| Truecolor | 7,593 | 6,809 |
| 256 color | 5,443 | 4,659 |
| ANSI16 | 3,520 | 2,736 |
| Monochrome | 1,426 | 642 |

The compatibility check also asserts that each payload ends with `SGR 0`.
This fixes the navigation color corruption discovered during the human run.
The root cause was retained terminal SGR state: `present_at()` emitted media
foreground/background changes, then application redraws emitted text without
establishing UI colors. It was not stale output, rectangle overlap, cursor
positioning, selection logic, video decoding, or TRend's color conversion.

PNG, JPEG, GIF, and Media Foundation MP4 passed at 80x24 truecolor/Unicode and
120x40 monochrome/ASCII. Catalog thumbnails are representative MP4 first
frames; PNG/JPEG/GIF remain supported through the same media layer. WebP, MKV,
and WebM are not accepted by this bridge. Unknown
extensions return an explicit error; no fallback image is substituted.

## Known Limitations

- Video is validated only on Windows x64. Linux, macOS, ARM, ARM64, Termux,
  SSH, multiplexers, and other terminal emulators were unavailable here.
- Non-Windows MP4 reports that no video backend is available; no portable
  support is claimed.
- Audio is intentionally not played.
- Selection is keyboard hover; terminals do not provide pointer hover here.
- Terminal resizing requires an application shell redraw to remove old layout
  geometry, while steady preview frames remain incremental and positioned.
- Working-set memory includes Media Foundation and process runtime allocations;
  adapter `retained_bytes` covers reusable RGBA storage only.
- Automated tests validate geometry, bytes, media flow, cancellation, and
  timing. Final visual appearance still requires a human terminal run because
  fonts and terminal rendering cannot be proven by byte tests.

## Readiness

This is ready to serve as the reusable integration example for the currently
validated Windows video backend. It proves an independent terminal application
can consume TRend for live media while retaining UI, input, layout, scheduling,
and terminal policy. Its UI code remains replaceable; a Music Terminal or file
manager can reuse the bridge/adapter/rendering pattern without importing this
catalog or screen layout.

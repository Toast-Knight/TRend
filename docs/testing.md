# TRend Media Test Harness

Run `scripts/visual-test-windows.bat` or `scripts/visual-test-linux-wsl2.bat` from the repository
root. Each script recursively discovers files in `tests/fixtures/media`, builds the
developer viewer once, and dispatches only formats supported by that platform.
Intentionally corrupt regression fixtures are excluded from these visual menus.
`scripts/test-media.bat` remains available as the original Windows harness.
See [`../tests/fixtures/media/README.md`](../tests/fixtures/media/README.md) for fixture provenance rules.

Current viewer support:

- PNG through `trend-image`
- JPEG/JPG through `trend-image`
- GIF through `trend-animation`
- MP4/H.264 through `trend-video` and Windows Media Foundation

WebP, WebM/VP9, MKV/H.264, AVI/MJPEG, and MOV/H.264 or HEVC are experimental
formats. Build the viewer with the corresponding feature before direct use;
Test All continues past unavailable entries. Video audio streams are detected
for diagnostics but intentionally not decoded or played.

Linux H.264 regression fixtures under `tests/fixtures/media/MKV Experimental` use the
default-off `linux-h264` feature and system OpenH264. They include 320x192 and
1280x720 AVC, AVC with Opus audio, and truncated input.

The JPEG fixtures cover color bars, a synthetic scene, dark detail, smooth
gradients, and intentionally severe compression artifacts. Regenerate the four
`Jpeg*.jpg` files with `scripts/fixtures/generate-jpeg.ps1`.

After selecting one supported file, choose a quality path:

| Harness mode | Renderer path | Status |
| --- | --- | --- |
| FAST | Production half-block with nearest sampling | Production |
| BALANCED | Production half-block with bilinear sampling | Production |
| QUALITY | Production half-block with area sampling | Production |

`COMPARE ALL` opens the selected media at FAST and lets `M` cycle through all
three paths without reopening the source. GIF and video start paused in comparison
mode so every quality path can render the identical decoded frame and viewport.
Space resumes playback when desired. Direct viewer invocations are also
available:

```powershell
target\debug\trend-test-viewer.exe --compare-all "tests\fixtures\media\ColorTest.mp4"
target\release\trend-test-viewer.exe --probe 80x24 21 "tests\fixtures\media\JpegPhotoScene.jpg"
```

The status row reports active quality, source or frame information, viewport,
render time, and emitted ANSI byte count. The non-interactive `--probe` command
reports median/minimum rendering time, full-frame ANSI generation time and byte
count, deterministic fingerprints, and glyph usage for the three accepted modes.
These measurements belong only to the developer viewer and are not production
APIs.

Viewer controls:

| Key | Action |
| --- | --- |
| Q | Return to the menu |
| C / V / S | Contain / cover / stretch |
| 1 / 2 / 3 | FAST / BALANCED / QUALITY modes |
| M | Cycle to the next quality mode |
| N / B / A | Legacy aliases for FAST / BALANCED / QUALITY |
| Space | Pause or resume GIF playback |
| R | Restart GIF playback |
| Left / Right | Seek one GIF frame or five seconds in video |
| Esc | Quit video playback |

Resize the terminal window at any time to exercise rerendering from the same
decoded frame. The viewer is intentionally a developer harness, not an
application UI.

The experimental glyph matcher failed real-viewer acceptance and is not exposed
by this harness. Its ideal-mask benchmark does not establish actual terminal
glyph quality.

## Python Application Integration Lab

`examples/python-tui/trend_tui.py` is a separate stdlib-only example application. It
owns terminal input, navigation, layout, capability configuration, resize
handling, and preview lifecycle. The optional `trend-python-tui-bridge` process
uses the real PNG/JPEG/GIF/video adapters and TRend renderer, then returns
positioned ANSI for the application-owned preview panel.

See `examples/python-tui/README.md` and `architecture.md`. The native
Win32/GDI lab under `tools/ui-lab` remains a separate Windows GUI integration experiment;
neither laboratory is part of the core architecture.

## Library tests and checks

`cargo test` runs the core's unit, golden, property, and differential tests.
The workspace (`cargo test --workspace`) adds the decoder crates, tools, and
`examples/external-renderer`. Features that need system codec libraries
(`linux-webm` and `linux-mkv` need libvpx) are tested only where those
libraries are installed.

`scripts/c-consumer-smoke.sh` (Linux and macOS) builds the release `cdylib`
and compiles `tests/c/consumer_smoke.c` against `include/trend.h` as strict
C11 (`-Wall -Wextra -Werror -pedantic`). The program checks struct sizes and
field offsets with static assertions, then exercises every exported
function, including error codes and buffer-length rules. Run it after any
change to `src/ffi.rs` or the header.

`cargo run --release --example frame_budget` measures whole frames against
the 30-FPS budget; see [benchmarks.md](benchmarks.md).

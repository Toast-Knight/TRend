# TRend Media Test Harness

Run `visual_test_windows.bat` or `visual_test_linux_wsl2.bat` from the repository
root. Each script recursively discovers files in `Materials`, builds the
developer viewer once, and dispatches only formats supported by that platform.
Intentionally corrupt regression fixtures are excluded from these visual menus.
`test_media.bat` remains available here as the original Windows harness.
See [`Materials/README.md`](Materials/README.md) for fixture provenance rules.

Current viewer support:

- PNG through `trend-image`
- JPEG/JPG through `trend-image`
- GIF through `trend-animation`
- MP4/H.264 through `trend-video` and Windows Media Foundation

WebP, WebM/VP9, MKV/H.264, AVI/MJPEG, and MOV/H.264 or HEVC are experimental
formats. Build the viewer with the corresponding feature before direct use;
Test All continues past unavailable entries. Video audio streams are detected
for diagnostics but intentionally not decoded or played.

Linux H.264 regression fixtures under `Materials/MKV Experimental` use the
default-off `linux-h264` feature and system OpenH264. They include 320x192 and
1280x720 AVC, AVC with Opus audio, and truncated input.

The JPEG fixtures cover color bars, a synthetic scene, dark detail, smooth
gradients, and intentionally severe compression artifacts. Regenerate the four
`Jpeg*.jpg` files with `generate_jpeg_fixtures.ps1`.

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
target\debug\trend-test-viewer.exe --compare-all "Test runs Envs\Materials\ColorTest.mp4"
target\release\trend-test-viewer.exe --probe 80x24 21 "Test runs Envs\Materials\JpegPhotoScene.jpg"
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

`Python TUI/trend_tui.py` is a separate stdlib-only example application. It
owns terminal input, navigation, layout, capability configuration, resize
handling, and preview lifecycle. The optional `trend-python-tui-bridge` process
uses the real PNG/JPEG/GIF/video adapters and TRend renderer, then returns
positioned ANSI for the application-owned preview panel.

See `Python TUI/README.md` and `../APPLICATION_INTEGRATION.md`. The native
Win32/GDI lab under `UI` remains a separate Windows GUI integration experiment;
neither laboratory is part of the core architecture.
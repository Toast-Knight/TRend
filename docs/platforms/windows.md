# Windows

## Prerequisites and runtime

Windows video decoding uses Media Foundation. MP4/H.264 is the primary path;
default-off features add the combinations documented in [../api.md](../api.md).
TRend does not bundle codecs or FFmpeg, so optional formats depend on transforms
installed on the host. Use Windows 10 or newer, a current stable Rust toolchain,
and an ANSI/VT-capable terminal. Packaged binaries may require the current x64
Microsoft Visual C++ Redistributable.

## Build and test

Build, test, or open the visual test menu from the repository root:

```powershell
.\scripts\build-windows.bat
.\scripts\test-windows.bat
.\scripts\visual-test-windows.bat
```

The build creates `dist/windows` and validates its manifest, executables, and
sample probes.

## Supported scope and limitations

The core renderer, PNG, JPEG, GIF, and MP4/H.264 paths are supported. Feature
flags add WebP and Media Foundation routes for WebM, MKV, MJPEG AVI, and
H.264/HEVC MOV when the required host transform is installed. TRend does not
decode audio, and unsupported codec/container pairs return explicit errors.

## Example

```powershell
.\target\release\trend-test-viewer.exe --probe 20x10 2 tests\fixtures\media\ColorTest.mp4
.\examples\terminal-overlay\play-top-right.bat
```
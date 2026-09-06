# Linux

## Prerequisites and runtime

The core and image/animation adapters use portable Rust paths. Optional VP8/VP9
video requires system libvpx; optional H.264 requires a compatible OpenH264
shared library. No FFmpeg dependency is introduced.

## Build and test

On Debian/Ubuntu or WSL2:

```bash
sudo apt-get install pkg-config libvpx-dev
./scripts/build-linux.sh
```

From Windows, the wrappers use the `Debian` WSL distribution by default:

```powershell
.\scripts\build-linux-wsl2.bat
.\scripts\test-linux-wsl2.bat
.\scripts\visual-test-linux-wsl2.bat
```

Set `TREND_WSL_DISTRO` to another installed distribution when needed. The build
creates `dist/linux` with a sorted SHA-256 manifest.

## Supported scope and limitations

The core renderer and image/animation paths are portable. Linux WebM and MKV
VP8/VP9 use the `linux-webm` and `linux-mkv` features; MKV H.264 uses
`linux-h264`. AVI and MOV video are not advertised on Linux. TRend does not
decode audio, and OpenH264 package names and sonames vary by distribution.

## Example

```bash
./target/release/trend-test-viewer --probe 20x10 2 tests/fixtures/media/ColorTest.gif
```
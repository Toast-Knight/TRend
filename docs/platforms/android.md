# Android ARM64 and Termux

TRend uses the same renderer, frame representation, timing, resize, color, and
ANSI presentation pipeline on every platform. Android-specific code belongs in
adapter backends; there is no separate Android renderer.

## Build

The supported target is `aarch64-linux-android` with Android API 21 or newer.
Install the Rust target and an Android NDK, then run from the repository root:

```powershell
.\scripts\build-android.bat
```

The script finds `ANDROID_NDK_HOME`, `ANDROID_NDK_ROOT`, or the first
project-local `android-ndk-*` directory. Set `ANDROID_API_LEVEL` to override API
21. It builds the complete workspace with all Cargo features and packages:

```text
dist/android/aarch64-linux-android/lib/libtrend.so
dist/android/aarch64-linux-android/bin/trend-test-viewer
dist/android/aarch64-linux-android/include/trend.h
dist/android/aarch64-linux-android/manifest.sha256
```

The NDK is a build dependency only. The core, PNG, JPEG, GIF, WebP, BMP, and
TIFF paths do not require an additional native library at runtime.

## Termux Validation

Copy the package and test media to a device without root, then in Termux run:

```sh
chmod +x trend-test-viewer
./trend-test-viewer --probe 80x24 10 /storage/emulated/0/Pictures/example.png
./trend-test-viewer --probe 80x24 10 "$HOME/example.gif"
./trend-test-viewer --probe 80x24 10 "$HOME/example.webp"
```

The probe exercises content detection, decoding, representative-frame or
thumbnail selection, resize/color conversion, terminal-cell rendering, and ANSI
generation. Interactive rendering additionally exercises raw mode, alternate
screen, cursor control, terminal dimensions, and resize events through
`crossterm`.

`TerminalCapabilities::detect()` reads `TERM`, `COLORTERM`, and `NO_COLOR`.
`TerminalEnvironment::detect()` also reports stdin/stdout TTY state. Unknown or
`dumb` terminals fall back to monochrome ASCII output.

TRend accepts paths supplied by its host and does not hard-code Termux home or
shared-storage locations. Android storage permissions remain the host
application's responsibility.

## Detection and Resources

`trend-media` classifies bounded file headers before MIME and extension hints.
It distinguishes image, animation, video, audio, document, archive, model, and
unknown media, then reports adapter capabilities separately. Recognition does
not imply decoding support.

Unified GIF and animated WebP playback scan timeline metadata without retaining
every decoded frame. GIF keeps compressed input, one RGBA compositing canvas,
and at most one disposal backup; WebP reuses one output canvas. Both rewind
their decoder for backward seeks. `max_decoded_bytes` therefore limits the
decoded working set rather than `width * height * frame_count`.
Android defaults to a 96 MiB decoded animation limit; desktop defaults to 256
MiB. Frame count, dimensions, pixels, decoder memory, and minimum frame duration
remain independently configurable through `trend_animation::DecodeLimits`.

TRend creates no decoder, thumbnail, animation, or video worker threads. Hosts
own concurrency and should bound their worker pools for the target device.

## Native Dependency Audit

| Component | Android ARM64 | Runtime requirement |
| --- | --- | --- |
| Core renderer and C ABI | linked | Android system C runtime |
| PNG | compiled, pure Rust | none |
| JPEG | compiled, pure Rust | none |
| GIF | compiled, pure Rust | none |
| Static/animated WebP | compiled, pure Rust | none |
| BMP/TIFF | compiled, pure Rust | none |
| Terminal viewer | linked | Termux-compatible terminal |
| Windows Media Foundation | excluded | Windows only |
| Linux libvpx/OpenH264 backends | excluded | desktop Linux only |
| Android video decoder | not implemented | planned Media NDK backend |

Android is deliberately not treated as desktop Linux. Linux codec features and
their `pkg-config`, libvpx, and OpenH264 dependencies remain guarded by
`target_os = "linux"` and are not linked into Android packages.

## Verified Capability Matrix

"Cross" means compilation and final ARM64 ELF linking were verified. "Runtime"
means the project maintainer has exercised the packaged viewer on Android.

| Capability | Windows | Linux | Android ARM64 | Notes |
| --- | --- | --- | --- | --- |
| Core rendering | tested | tested | runtime | Shared renderer |
| PNG | tested | tested | runtime | Pure Rust |
| JPEG | tested | tested | runtime | Pure Rust |
| GIF | tested | tested | runtime | Bounded incremental unified path |
| WebP | tested | tested | runtime | Feature `webp` |
| Animated WebP | tested | tested | runtime | Bounded incremental unified path |
| BMP/TIFF | tested | tested | runtime | Pure Rust |
| SVG | detection only | detection only | detection only | No decoder advertised |
| MP4 | tested | unsupported | detection only | Android decoder pending |
| MKV | tested | tested with features | detection only | Android decoder pending |
| AVI | tested | unsupported | detection only | Android decoder pending |
| MOV | tested | unsupported | detection only | Android decoder pending |
| WebM | tested | tested with features | detection only | Android decoder pending |
| Thumbnails | tested for decoded media | tested for decoded media | runtime for images/animations | Representative-frame API |
| Metadata | tested for decoded media | tested for decoded media | runtime for images/animations | Video metadata pending |
| Truecolor/256/16/mono | tested | tested | runtime | Environment detection |
| Terminal resizing | tested | tested | runtime | Viewer uses `crossterm` |

Android image, animation, rendering, and terminal presentation are runtime
validated. Video playback is not implemented: recognized video containers are
detection-only until an Android Media NDK backend and device codec tests exist.
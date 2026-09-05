# TRend Video Adapter

`trend-video` is an optional decoder package. It does not add video code or
dependencies to the `trend` core.

## Backend Decision

The Linux experiment adds a narrow optional backend without changing the public
API or core timing system. It uses Matroska demuxing with system libvpx for
VP8/VP9 and a runtime-loaded system OpenH264 library for H.264.

The initial backend is Windows Media Foundation Source Reader. Its default MP4
path is stable; default-off `webm` and `mkv` features permit those containers
through the same decoder and timing contract.

| Approach | Coverage | Footprint and delivery | Decision |
| --- | --- | --- | --- |
| Media Foundation | MP4/H.264 plus system-supported WebM/MKV codecs | Windows system DLLs; no bundled decoder files | Stable for MP4; experimental for WebM/MKV |
| Matroska + libvpx | WebM/MKV VP8/VP9 on Linux | Two Rust FFI/demuxer crates plus system libvpx | Experimental behind `linux-webm`/`linux-mkv` |
| Matroska + OpenH264 | MKV H.264 on Linux | Existing demuxer plus runtime-loaded system OpenH264 | Experimental behind `linux-h264` |
| FFmpeg libraries | Broad MP4/WebM/MKV and codec support | Multiple native libraries; LGPL/GPL build and redistribution obligations | Deferred |
| External FFmpeg executable | Broad support | Requires process spawning and a separately installed executable | Rejected |
| Narrow Rust/C codec stack | Codec-specific; separate demuxers and color conversion required | Smaller per codec but substantial integration and seek complexity | Candidate for future portable backends |

The installed `ffmpeg`/`ffprobe` programs were used only to inspect development
fixtures. TRend never invokes them.

## Current Contract

- Capabilities: `Container`, `VideoCodec`, and `BackendCapabilities` describe
  supported pairs before decode. AVI has one verified Windows entry; MOV has
  two verified Windows entries.
- Input: local MP4 on Windows, feature-gated WebM containing VP8/VP9 on Windows
  or Linux, or feature-gated MKV containing H.264/VP9 on Windows and
  VP8/VP9/H.264 on Linux.
- Output: timestamped, top-down RGBA8 `VideoFrame` borrowing through
  `PixelFrameView`.
- Scheduling: synchronous calls made by the host. The adapter creates no worker,
  timer, polling loop, or async runtime. Media Foundation codecs may use internal
  operating-system implementation threads.
- Timing: source presentation timestamps support variable frame rates. The test
  viewer uses the core `PlaybackClock` for pause, resume, restart, and seeking.
- Seeking: keyframe-based source seek followed by forward decode to the first
  frame at or after the requested timestamp. It is not promised frame-accurate.
- Audio: presence is reported; audio is intentionally not decoded or played.
- Resize: the currently borrowed RGBA frame is rerendered without reopening the
  decoder.

## Bounds

Defaults permit at most two retained RGBA frames, 128 MiB of adapter-owned frame
storage, 8192 pixels per dimension, and 16 megapixels per frame. Evicted RGBA
buffers are reused. Media Foundation's internal codec allocations are owned by
Windows and are not included in `retained_bytes()`.

## Dependencies and Delivery

`trend-video` depends on `trend` and `windows` 0.62.2 with selected Media
Foundation/COM features. The optimized Rust rlib measured 170,836 bytes. The
complete release test viewer, including PNG, GIF, video, and terminal code,
measured 345,088 bytes.

The adapter is statically linked into its Rust consumer. At runtime on Windows,
the tested executable imports system libraries including `mfplat.dll`,
`mfreadwrite.dll`, `ole32.dll`, `oleaut32.dll`, `propsys.dll`, and normal Windows
kernel/UCRT libraries. It also imports `VCRUNTIME140.dll`; portable packaging
must provide the matching Microsoft Visual C++ Redistributable when absent.
No FFmpeg or third-party codec DLL is required.

On Linux, `linux-webm` or `linux-mkv` adds `matroska-demuxer` 0.8.1 and
`libvpx-native-sys` 5.0.17. Debian/Ubuntu builds require `pkg-config` and
`libvpx-dev`; execution requires the corresponding libvpx runtime. The tested
Debian packages were libvpx 1.15.0, measuring 3,545 KiB for `libvpx9`, 5,128
KiB for `libvpx-dev`, and 29 KiB for `pkg-config`.

The `linux-h264` feature adds `openh264` 0.9.8 with runtime loading and reuses
`matroska-demuxer`. It requires a compatible OpenH264 shared library at runtime;
the tested Debian package is `libopenh264-8` 2.6.0 (about 1.2 MiB installed).
No OpenH264 development headers are required. `TREND_OPENH264_LIBRARY` can name
an explicit compatible shared object when the platform soname differs.

The experimental WebM path has been exercised with VP8 and VP9, varied frame
rates and dimensions, keyframe-then-forward seeking, a corrupt file, an
oversized-frame rejection, and a file containing Opus audio. Audio presence is
reported but the audio stream is not selected or decoded. AV1 is not claimed.

The experimental MKV path uses the same backend per platform. Real fixtures
cover H.264 and VP9 on Windows, VP9 and H.264 on Linux, audio metadata,
presentation timestamps, seeking, bounds, corruption, and rendering. Neither
backend claims AV1.

Platform status:

| Platform | x86 | x64 | ARM/ARM64 |
| --- | --- | --- | --- |
| Windows with Media Foundation/H.264 | Binding supports target; untested | Implemented and tested | Binding supports ARM64; untested |
| Windows with Media Foundation/VP8 or VP9 WebM | Binding supports target; untested | Experimental and tested | Binding supports ARM64; untested |
| Windows with Media Foundation/H.264 or VP9 MKV | Binding supports target; untested | Experimental and tested | Binding supports ARM64; untested |
| Linux WebM VP8/VP9 with `linux-webm` | Untested | Experimental and tested under WSL2 | Untested |
| Linux MKV VP8/VP9 with `linux-mkv` | Untested | Experimental and tested under WSL2 | Untested |
| Linux MKV H.264 with `linux-h264` | Untested | Experimental and tested under WSL2 | Untested |
| macOS/other | No backend | No backend | No backend |

The package compiles a clear unsupported-platform stub when no platform backend
is enabled. Linux Matroska metadata owns packet timestamps, duration, audio
presence, and seeking; libvpx is only the VP8/VP9 pixel decoder and OpenH264 is
only the H.264 pixel decoder. All backends
produce the same timestamped borrowed-RGBA contract.

Declared matrices:

| Backend | Container | Codecs |
| --- | --- | --- |
| Windows Media Foundation | MP4 | H.264, HEVC, VP9 |
| Windows Media Foundation | MKV | H.264, VP9 |
| Windows Media Foundation | WebM | VP8, VP9 |
| Windows Media Foundation | AVI | MJPEG |
| Windows Media Foundation | MOV | H.264, HEVC |
| Linux libvpx | WebM | VP8, VP9 |
| Linux libvpx | MKV | VP8, VP9 |
| Linux OpenH264 | MKV | H.264 |

Media Foundation entries describe backend routes and still depend on codecs
installed in Windows. Linux validates Matroska codec IDs against its matrix and
returns `VideoError::UnsupportedCombination` for known unsupported pairs.
Linux reports AVI and MOV as `VideoError::UnsupportedContainer` before
demuxing.

The Linux H.264 experiment accepts AVC-formatted Matroska packets, converts
their length-prefixed NAL units to Annex B inside the codec adapter, and sends
them to system OpenH264. Permanent 320x192 and 1280x720 fixtures plus an Opus
audio case and truncated input cover dimensions, exact frame durations,
one-second duration, seeking, metadata, bounds, corruption, buffer accounting,
unified dispatch, and deterministic terminal rendering. MP4, MOV, AVI, HEVC,
10-bit H.264, B-frame/reordered streams, and non-AVC Matroska H.264 are not
claimed by this backend. The permanent fixtures are High-profile 8-bit 4:2:0
AVC with no B-frames.

The AVI experiment uses Media Foundation through the existing decoder and
packed RGBA8 frame path. Real MJPEG fixtures cover 320x180 and 1280x720 video,
PCM audio metadata, timestamps, one-second duration, seeking, dimension and byte
limits, truncated input, and nonblank terminal rendering. MPEG-4 Part 2,
Microsoft MPEG-4 v3, and raw AVI probes also decoded locally, but remain outside
the declared matrix until they have permanent fixtures and regression tests.

The MOV experiment also uses the existing Media Foundation and packed RGBA8
path. Real fixtures cover H.264 and HEVC at 320x192, HEVC at 1280x720, H.264
with AAC audio, and truncated input. Tests cover timing, duration, seeking,
bounds, corruption, unified dispatch, and rendering. VP9 is not advertised:
the available FFmpeg MOV muxer rejected VP9, so no valid fixture entered the
Media Foundation qualification path.

## Measured Baseline

Windows x64 desktop, release build, 500x500 H.264 fixture at 20 FPS:

- decode throughput: approximately 316 FPS;
- core render throughput at 80x24: approximately 33,067 FPS;
- seek to midpoint and decode forward: 19.4 ms;
- three resize renders (40x12, 120x36, 80x24): 0.25 ms total;
- retained adapter RGBA storage after seek: 1,000,000 bytes;
- buffered frames after seek: 1.

These are development measurements, not cross-machine guarantees. CPU
utilization was not reliable for the 650 ms fixture, and no lower-resource or
ARM machine was available. Buffer-address tests verify reuse during steady-state
decode, but allocator-call instrumentation remains future benchmark work.
# TRend Media Presentation

`trend-media` is an optional application-neutral layer above the existing image,
animation, and video adapters. It adds no dependency or media policy to the
`trend` core and does not change the C ABI.

## Architecture Assessment

1. Existing borrowed RGBA views, caller-owned `CellFrame`, render options,
   `PlaybackClock`, `AnimationTimeline`, `VideoDecoder`, resize generations, and
   `AnsiPresenter::present_at()` already provide rendering, timing, bounded
   decoding, cancellation integration, and positioned output.
2. The missing pieces were one codec-neutral representative-frame entry point
   and one host-driven bounded clip state machine.
3. The layer is entirely above the existing four C exports; their signatures and
   16-byte `Cell` ABI are unchanged.
4. `AnimationTimeline` already supports variable frame durations, finite or
   infinite loops, pause, resume, restart, and seek. `PreviewSession` supplies an
   application-selected clip range around it.
5. `VideoDecoder::frame_for_time(Duration::ZERO)` obtains frame 0 into its
   existing bounded two-frame reuse pool. Repeated representative requests reuse
   that frame until media position changes.
6. Positioned presenter calls overwrite bounded terminal rectangles. They do not
   provide alpha compositing between terminal cells; applications establish
   draw order and redraw covered regions.
7. `PreviewSession` lives here, above codec adapters and below applications. It
   owns only a decoded source, `PlaybackClock`, clip range, and state enum.
8. Applications retain hover/select meaning, duration, looping choice, layout,
   terminal policy, scheduling, generations, workers, and fullscreen behavior.
9. Session construction adds no heap allocation beyond existing decoded media.
   Video remains bounded by `VideoLimits`; GIF and animated WebP use bounded
   incremental decoders; static images retain one RGBA frame.
10. Rapid hover changes are application requests. `cancel()` prevents subsequent
    session frames immediately; application generations prevent already-running
    synchronous work from being presented after it becomes stale.
11. Resize changes only `RenderRequest`/position. The same borrowed frame and
    decoder can be rendered into a new viewport. Moving a presenter origin now
    forces a bounded redraw even when cells are unchanged.
12. Preview and fullscreen are application presentation choices over the same
    session. Calling `start()` with a full-source range rebases the existing clock
    and seeks the existing decoder; it does not reopen media.

## API Model

`MediaSource::open()` uses bounded magic-byte detection first, followed by an
optional MIME hint and the file extension. It recognizes PNG, JPEG/JPG, GIF,
BMP, TIFF, and MP4. Default-off features add static/animated WebP (`webp`),
WebM (`webm`/`linux-webm`), and MKV (`mkv`/`linux-mkv`/`linux-h264`), plus
Windows MJPEG AVI (`avi`) and H.264/HEVC MOV (`mov`).
It exposes common metadata, `representative_frame()`, `frame_at()`,
bounded-memory counters, and a codec-neutral seek operation.

`MediaInfo` reports kind, dimensions, optional duration, optional nominal frame
rate, audio-stream presence, and optional seek accuracy. Nominal frame rate is
metadata, not a constant-FPS scheduling promise.

For an external master clock, use `frame_at_with_timing(Duration)` or
`PreviewSession::frame_at_media_time(Duration)`. The returned `MediaFrame`
contains:

- `requested_timestamp`: the caller's codec-neutral media time;
- `presentation_timestamp`: the actual source timestamp carried by the RGBA
   frame;
- `duration`: the known source frame/sample duration, when available;
- `view`: the existing borrowed `PixelFrameView` accepted by TRend core.

`seek_with_timing(Duration)` and `PreviewSession::seek_media_time(Duration)`
return the requested timestamp, the first post-seek frame timestamp when known,
and backend seek accuracy. The Windows backend reports
`KeyframeThenForward`: it seeks to a backend-selected decode point, discards
decoded frames before the target, and returns the first available presentation
frame at or after the target. This is not a frame-perfect seek guarantee.
Existing `frame_at()` and `seek()` remain convenience APIs.

`PreviewSession` owns a `MediaSource` and exposes:

- `start(PreviewRange, wall_now)` with arbitrary nonzero duration, offset, and
  loop policy;
- `frame_for_time(wall_now)` using a host-supplied monotonic clock;
- `pause`, `resume`, `seek`, `restart`, `stop`, and `cancel`;
- `state()` for stopped, playing, paused, completed, and cancelled results;
- `into_source()` when an application wants to retain decoded media outside the
  session.

There is no hidden timer, thread, async runtime, queue, terminal, or UI state.
A synchronous decoder call already in progress is not forcibly interrupted;
applications discard its result using their current generation after cancel.

TRend does not decode or play audio, select devices, control volume, buffer
audio, or implement an audio backend. An external audio system may act as the
master clock and pass its current position to `frame_at_media_time()`.

## Capability Matrix

| Format | Representative frame | Bounded preview | Full playback | Validated platform |
| --- | --- | --- | --- | --- |
| PNG | yes | N/A | N/A | Windows x64 |
| JPEG/JPG | yes | N/A | N/A | Windows x64 |
| BMP/TIFF | yes | N/A | N/A | Portable Rust path; Android ARM64 device-validated |
| GIF | yes | yes | yes | Windows x64 |
| WebP static/animated (`webp`) | yes | yes for animation | yes for animation | Bounded portable Rust path; Android ARM64 device-validated |
| MP4/H.264 | yes | yes | yes | Windows x64 Media Foundation |
| WebM VP8/VP9 | yes | yes | yes | Experimental Windows `webm`; Linux `linux-webm` |
| MKV H.264 | yes | yes | yes | Experimental Windows `mkv`; Linux `linux-h264` |
| MKV VP8/VP9 | yes | yes | yes | Experimental Windows `mkv`; Linux `linux-mkv` |
| AVI MJPEG | yes | yes | yes | Experimental Windows `avi`; unsupported on Linux |
| MOV H.264/HEVC | yes | yes | yes | Experimental Windows `mov`; unsupported on Linux |

Linux VP8/VP9 support requires system libvpx; Linux H.264 requires system
OpenH264. Both paths are validated on x64 WSL2.
Unsupported codec/container combinations retain the explicit unsupported-video
result. Android ARM64 image and animation paths are device-validated. Android
video playback is not implemented and recognized containers remain detection-only.

## Memory and Caching

A representative frame is borrowed from the decoded source. Repeated rendering
or resize does not decode it again. Applications may cache `MediaSource` by their
own media identity and bound that cache according to product needs; this crate
does not create a global cache or cache rendered ANSI for every viewport.

Preview/full playback retains only the adapter's existing bounded decoded state.
It never decodes an entire video. Unified GIF playback retains one compositing
canvas and at most one disposal backup rather than every full decoded frame.
Unified animated WebP playback also retains one output canvas and resets its
incremental decoder for backward seeks. Standalone eager APIs remain available
for compatibility.

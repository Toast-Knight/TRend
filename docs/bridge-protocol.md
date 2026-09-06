# Bridge protocol

`trend-python-tui-bridge` is a subprocess that opens one media file, reads
ASCII commands terminated by LF from standard input, and writes response
headers plus optional raw ANSI bytes to standard output. Start it with:

```powershell
.\target\release\trend-python-tui-bridge.exe <MEDIA>
```

The protocol is currently tied to the TRend package version. Hosts should keep
the executable and client from the same release.

## Framing

Text headers are UTF-8 lines terminated by LF. `OK` and `FRAME_OK` headers
announce a byte count. After reading such a header, the host must:

1. Parse `byte_count` as a non-negative integer.
2. Read exactly that many bytes from standard output.
3. Write those bytes unchanged to an ANSI-capable terminal.
4. Read the next response only after consuming the complete payload.

A byte count of zero is valid for an incremental frame with no changed cells.

## Metadata

Request:

```text
META
```

Response:

```text
META_OK kind width height duration_ns fps_numerator fps_denominator audio seek_accuracy
```

- `kind`: `image`, `animation`, or `video`.
- `width`, `height`: source pixel dimensions.
- `duration_ns`: duration in nanoseconds, or `-1` when unknown.
- `fps_numerator`, `fps_denominator`: nominal frame-rate ratio, or `0 0`.
- `audio`: `1` when an audio stream is present, otherwise `0`. The bridge does
  not decode or play audio.
- `seek_accuracy`: `keyframe_then_forward` or `not_applicable`.

## Frame request

```text
FRAME WIDTH HEIGHT ROW COLUMN QUALITY COLOR GLYPHS ELAPSED_MS PRESENTATION START_MS DURATION_MS LOOP
```

- `WIDTH`, `HEIGHT`: nonzero terminal-cell viewport dimensions.
- `ROW`, `COLUMN`: 1-based terminal origin; zero is clamped to one.
- `QUALITY`: `fast`, `balanced`, or `quality`.
- `COLOR`: `truecolor`, `256`, `16`, or `mono`.
- `GLYPHS`: `unicode` or `ascii`.
- `ELAPSED_MS`: host-supplied elapsed time for clip playback.
- `PRESENTATION`: `representative` or `clip`.
- `START_MS`, `DURATION_MS`: clip range. Duration must be nonzero for `clip`.
- `LOOP`: `loop` or `once`.

Response:

```text
OK byte_count decode_us render_us ansi_us retained_bytes decoded_frames dropped_frames buffered_frames
<exactly byte_count raw ANSI bytes>
```

`WAIT` means no frame is available yet. `ERR message` reports a rejected
command or media/rendering failure.

## Exact timestamp and seeking

```text
FRAME_AT WIDTH HEIGHT ROW COLUMN QUALITY COLOR GLYPHS TIMESTAMP_NS
```

Response:

```text
FRAME_OK byte_count requested_ns presentation_ns duration_ns acquisition_us render_us ansi_us retained_bytes decoded_frames dropped_frames buffered_frames seek_discarded_frames
<exactly byte_count raw ANSI bytes>
```

`duration_ns` is `-1` when unknown.

Seek independently with:

```text
SEEK timestamp_ns
SEEK_OK requested_ns first_frame_ns accuracy seek_us discarded_frames
```

`first_frame_ns` is `-1` when unavailable. Seeking may decode forward from a
keyframe and is not guaranteed to be frame exact.

## Playback clock

The host owns wall-clock scheduling. Supported commands are:

```text
CLOCK POSITION wall_ns
CLOCK PAUSE wall_ns
CLOCK PLAY wall_ns
CLOCK SEEK media_ns wall_ns
CLOCK RESTART wall_ns
```

Success returns `CLOCK_OK media_ns paused`, where `paused` is `0` or `1`.

## Presenter state and shutdown

`RESET` clears incremental ANSI presenter state and returns `RESET_OK`.
`QUIT` closes the bridge without a response.

## Reference client

[`examples/terminal-overlay/play-top-right.ps1`](../examples/terminal-overlay/play-top-right.ps1)
is the minimal reference client. It demonstrates ASCII command writes, exact
payload reads, zero-byte incremental frames, positioned output, and cleanup.
# TRend Python Video Player

A standalone terminal video-player reference application. TRend decodes timestamped
RGBA video frames and produces positioned ANSI output. Python owns the terminal,
layout, controls, playback policy, progress bar, mouse handling, generations, and
external audio process.

## Run

From the workspace root on Windows:

```powershell
python "examples\python-video-player\player.py"
```

Pass one or more videos as positional arguments to create an ordered playlist. With
no paths, the player uses the bundled MP4 fixtures. Use `--no-audio` to disable
external audio, `--color truecolor|256|16|mono`, and `--glyphs unicode|ascii` to
select terminal capabilities.

Audio is optional and requires `ffplay` on `PATH`. It runs as a separate process;
TRend never decodes, buffers, or plays audio. The player uses TRend's
`PlaybackClock` as its authoritative presentation clock and restarts ffplay at the
requested position after a seek or resume. Pausing stops ffplay, ensuring external
audio cannot continue while the video clock is paused.

## Controls

| Input | Action |
| --- | --- |
| Space | Play or pause |
| Left / Right | Seek -5 / +5 seconds |
| Shift+Left / Shift+Right | Seek -30 / +30 seconds |
| Mouse click on progress bar | Seek to position |
| T / Y | Decrease / increase volume by 5% |
| N | Play the next video, wrapping at the end |
| R | Restart |
| F | Toggle fullscreen |
| 1 / 2 / 3 | Fast / balanced / quality rendering |
| C | Cycle color capability |
| G | Toggle Unicode / ASCII glyphs |
| Q | Quit |

Resize and fullscreen rerender through the same bridge process and media session;
they do not reopen the decoder. The terminal's alternate screen, cursor, mouse
tracking, and prior console mode are restored on exit.

## Validation

```powershell
python -m unittest "examples\python-video-player\test_player.py"
python "examples\python-video-player\player.py" --self-test --no-audio
```

The self-test exercises metadata, representative and preview frames, deterministic
`PlaybackClock` transitions, exact timestamp seek/render, three terminal sizes,
fullscreen, render modes, rapid seeks, end of media, progress seeking, no-audio
fallback, and one-decoder reuse.

Windows MP4/H.264 video uses Media Foundation. Linux video playback is currently
unsupported; codec-neutral timing and media APIs still compile there, and the
self-test reports that limitation without pretending to render video.

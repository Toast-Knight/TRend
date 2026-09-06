#!/usr/bin/env python3
"""Terminal video-player reference application using TRend and external audio."""

from __future__ import annotations

import argparse
import os
import queue
import re
import select
import shutil
import subprocess
import sys
import threading
import time
from dataclasses import dataclass
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
TUI_MODULE = ROOT / "examples" / "python-tui"
sys.path.insert(0, str(TUI_MODULE))

from trend_tui import (  # noqa: E402
    BridgeClient,
    Capabilities,
    FrameUnavailable,
    Layout,
    PreviewRequest,
    TimedFrameMetrics,
    ensure_bridge,
)

DEFAULT_VIDEO = ROOT / "tests" / "fixtures" / "media" / "ColorTest.mp4"
DEFAULT_PLAYLIST = (
    DEFAULT_VIDEO,
    ROOT / "tests" / "fixtures" / "media" / "ColorTest.mp4",
    ROOT / "tests" / "fixtures" / "media" / "ColorTest.mp4",
    ROOT / "tests" / "fixtures" / "media" / "ColorTest.mp4",
    ROOT / "tests" / "fixtures" / "media" / "ColorTest.mp4",
)
COLOR_MODES = ("truecolor", "256", "16", "mono")
QUALITY_MODES = ("fast", "balanced", "quality")
NEXT_VIDEO = 10
VOLUME_STEP = 5


@dataclass(frozen=True)
class PlayerLayout:
    columns: int
    rows: int
    outer_row: int
    outer_column: int
    outer_width: int
    outer_height: int
    video: Layout
    progress_row: int
    controls_row: int | None
    status_row: int | None
    debug_row: int | None
    fullscreen: bool


@dataclass(frozen=True)
class RenderJob:
    generation: int
    timestamp: float
    layout: Layout
    quality: str
    capabilities: Capabilities
    seek: bool = False


@dataclass(frozen=True)
class RenderResult:
    generation: int
    payload: bytes
    metrics: TimedFrameMetrics | None
    seek_accuracy: str | None
    seek_us: int
    error: str | None


class BridgeClock:
    """Python-owned policy driving TRend's PlaybackClock through the bridge."""

    def __init__(self, client: BridgeClient, lock: threading.Lock, duration: float) -> None:
        self.client = client
        self.lock = lock
        self.duration = duration
        with self.lock:
            self.client.clock_seek(0.0)

    def position(self) -> float:
        with self.lock:
            position_ns, _ = self.client.clock_position()
        return min(self.duration, position_ns / 1_000_000_000)

    def pause(self) -> float:
        with self.lock:
            position_ns, _ = self.client.clock_pause()
        return min(self.duration, position_ns / 1_000_000_000)

    def play(self) -> float:
        with self.lock:
            position_ns, _ = self.client.clock_play()
        return min(self.duration, position_ns / 1_000_000_000)

    def seek(self, target: float) -> float:
        target = max(0.0, min(self.duration, target))
        with self.lock:
            position_ns, _ = self.client.clock_seek(target)
        return position_ns / 1_000_000_000

    def restart(self) -> float:
        with self.lock:
            position_ns, _ = self.client.clock_restart()
        return position_ns / 1_000_000_000


class RenderWorker:
    def __init__(self, client: BridgeClient, lock: threading.Lock) -> None:
        self.client = client
        self.lock = lock
        self.requests: queue.Queue[RenderJob | None] = queue.Queue(maxsize=1)
        self.results: queue.Queue[RenderResult] = queue.Queue(maxsize=2)
        self.thread = threading.Thread(target=self._run, name="trend-video-player-render", daemon=True)
        self.thread.start()

    def update(self, job: RenderJob) -> None:
        while True:
            try:
                self.requests.get_nowait()
            except queue.Empty:
                break
        self.requests.put_nowait(job)

    def take_results(self) -> list[RenderResult]:
        output = []
        while True:
            try:
                output.append(self.results.get_nowait())
            except queue.Empty:
                return output

    def close(self) -> None:
        try:
            self.requests.put_nowait(None)
        except queue.Full:
            self.requests.get_nowait()
            self.requests.put_nowait(None)
        self.thread.join(timeout=3)

    def _publish(self, result: RenderResult) -> None:
        try:
            self.results.put_nowait(result)
        except queue.Full:
            self.results.get_nowait()
            self.results.put_nowait(result)

    def _run(self) -> None:
        while True:
            job = self.requests.get()
            if job is None:
                return
            try:
                seek_accuracy = None
                seek_us = 0
                with self.lock:
                    if job.seek:
                        seek = self.client.seek(job.timestamp)
                        seek_accuracy = seek.accuracy
                        seek_us = seek.seek_us
                    payload, metrics = self.client.frame_at(
                        job.timestamp, job.layout, job.quality, job.capabilities
                    )
                self._publish(
                    RenderResult(
                        job.generation,
                        payload,
                        metrics,
                        seek_accuracy,
                        seek_us,
                        None,
                    )
                )
            except FrameUnavailable as error:
                self._publish(RenderResult(job.generation, b"", None, None, 0, str(error)))
            except Exception as error:
                self._publish(
                    RenderResult(job.generation, b"", None, None, 0, f"render error: {error}")
                )


class AudioProvider:
    name = "unavailable"

    def __init__(self, volume: int = 100) -> None:
        self.volume = max(0, min(100, volume))

    def set_volume(self, volume: int) -> None:
        self.volume = max(0, min(100, volume))

    def start(self, position: float) -> None:
        pass

    def pause(self) -> None:
        pass

    def resume(self, position: float) -> None:
        pass

    def seek(self, position: float, playing: bool) -> None:
        pass

    def stop(self) -> None:
        pass

    def reported_position(self, master_position: float) -> float:
        return master_position

    @property
    def detail(self) -> str:
        return "no audio provider; TRend PlaybackClock is master"


class FfplayAudioProvider(AudioProvider):
    name = "ffplay"

    def __init__(self, executable: str, path: Path) -> None:
        super().__init__()
        self.executable = executable
        self.path = path
        self.process: subprocess.Popen[bytes] | None = None

    @property
    def detail(self) -> str:
        return "ffplay external audio; position estimated from TRend PlaybackClock"

    def start(self, position: float) -> None:
        self.stop()
        command = [
            self.executable,
            "-nodisp",
            "-autoexit",
            "-loglevel",
            "quiet",
            "-volume",
            str(self.volume),
            "-ss",
            f"{position:.6f}",
            "-vn",
            str(self.path),
        ]
        flags = subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0
        self.process = subprocess.Popen(
            command,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            creationflags=flags,
        )

    def pause(self) -> None:
        self.stop()

    def resume(self, position: float) -> None:
        if not self._running():
            self.start(position)

    def seek(self, position: float, playing: bool) -> None:
        self.stop()
        if playing:
            self.start(position)

    def stop(self) -> None:
        if self._running():
            assert self.process is not None
            self.process.terminate()
            try:
                self.process.wait(timeout=1)
            except subprocess.TimeoutExpired:
                self.process.kill()
                self.process.wait(timeout=1)
        self.process = None

    def _running(self) -> bool:
        return self.process is not None and self.process.poll() is None


class TerminalSession:
    def __init__(self) -> None:
        self.posix_settings = None
        self.windows_handle = None
        self.windows_mode = None

    def __enter__(self) -> "TerminalSession":
        if not sys.stdin.isatty() or not sys.stdout.isatty():
            raise RuntimeError("interactive player requires a terminal; use --self-test for CI")
        if os.name == "nt":
            self._enable_windows_vt()
        else:
            import termios
            import tty

            self.posix_settings = termios.tcgetattr(sys.stdin.fileno())
            tty.setcbreak(sys.stdin.fileno())
        write_ui("\x1b[?1049h\x1b[?25l\x1b[?1000h\x1b[?1006h\x1b[2J")
        return self

    def __exit__(self, *_: object) -> None:
        write_ui("\x1b[?1000l\x1b[?1006l\x1b[0m\x1b[?25h\x1b[?1049l")
        if os.name == "nt" and self.windows_handle is not None:
            import ctypes

            ctypes.windll.kernel32.SetConsoleMode(self.windows_handle, self.windows_mode)
        elif self.posix_settings is not None:
            import termios

            termios.tcsetattr(sys.stdin.fileno(), termios.TCSADRAIN, self.posix_settings)

    def read_event(self, timeout: float) -> str | tuple[str, int, int] | None:
        if os.name == "nt":
            return self._read_windows(timeout)
        ready, _, _ = select.select([sys.stdin], [], [], timeout)
        if not ready:
            return None
        data = os.read(sys.stdin.fileno(), 64).decode("utf-8", "ignore")
        return parse_input(data)

    def _read_windows(self, timeout: float) -> str | tuple[str, int, int] | None:
        import ctypes
        import msvcrt

        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            if not msvcrt.kbhit():
                time.sleep(0.003)
                continue
            key = msvcrt.getwch()
            if key in ("\x00", "\xe0"):
                code = msvcrt.getwch()
                shifted = bool(ctypes.windll.user32.GetAsyncKeyState(0x10) & 0x8000)
                if code == "K":
                    return "seek_large_back" if shifted else "seek_back"
                if code == "M":
                    return "seek_large_forward" if shifted else "seek_forward"
                return None
            if key == "\x1b":
                sequence = key
                end = time.monotonic() + 0.02
                while time.monotonic() < end and msvcrt.kbhit():
                    sequence += msvcrt.getwch()
                return parse_input(sequence)
            return parse_input(key)
        return None

    def _enable_windows_vt(self) -> None:
        import ctypes

        kernel = ctypes.windll.kernel32
        handle = kernel.GetStdHandle(-11)
        mode = ctypes.c_uint32()
        if not kernel.GetConsoleMode(handle, ctypes.byref(mode)):
            raise RuntimeError("stdout is not a Windows console with ANSI support")
        if not kernel.SetConsoleMode(handle, mode.value | 0x0004):
            raise RuntimeError("could not enable Windows virtual-terminal processing")
        self.windows_handle = handle
        self.windows_mode = mode.value


def parse_input(data: str) -> str | tuple[str, int, int] | None:
    mouse = re.search(r"\x1b\[<0;(\d+);(\d+)M", data)
    if mouse:
        return "mouse", int(mouse.group(1)), int(mouse.group(2))
    if data.startswith("\x1b[1;2D"):
        return "seek_large_back"
    if data.startswith("\x1b[1;2C"):
        return "seek_large_forward"
    if data.startswith("\x1b[D"):
        return "seek_back"
    if data.startswith("\x1b[C"):
        return "seek_forward"
    if data == " ":
        return "toggle"
    key = data.lower()
    return {
        "q": "quit",
        "n": "next",
        "t": "volume_down",
        "y": "volume_up",
        "r": "restart",
        "f": "fullscreen",
        "c": "color",
        "g": "glyph",
        "1": "quality_fast",
        "2": "quality_balanced",
        "3": "quality_quality",
    }.get(key)


def calculate_layout(columns: int, rows: int, fullscreen: bool) -> PlayerLayout:
    columns = max(20, columns)
    rows = max(10, rows)
    if fullscreen:
        outer_row, outer_column = 1, 1
        outer_width, outer_height = columns, max(3, rows - 2)
        controls_row = status_row = debug_row = None
        progress_row = rows
    else:
        outer_row, outer_column = 2, 2
        outer_width, outer_height = max(3, columns - 2), max(3, rows - 8)
        progress_row = rows - 5
        controls_row = rows - 4
        status_row = rows - 3
        debug_row = rows - 2
    video = Layout(
        columns,
        rows,
        outer_row + 1,
        outer_column + 1,
        max(1, outer_width - 2),
        max(1, outer_height - 2),
        False,
    )
    return PlayerLayout(
        columns,
        rows,
        outer_row,
        outer_column,
        outer_width,
        outer_height,
        video,
        progress_row,
        controls_row,
        status_row,
        debug_row,
        fullscreen,
    )


def progress_bar(position: float, duration: float, columns: int) -> tuple[str, int, int]:
    current = format_time(position)
    total = format_time(duration)
    percent = 0.0 if duration <= 0 else max(0.0, min(1.0, position / duration))
    width = max(5, columns - len(current) - len(total) - 13)
    marker = min(width - 1, round(percent * (width - 1)))
    bar = "=" * marker + "o" + "-" * (width - marker - 1)
    text = f" {'||' if position < duration else '[]'} {current} {bar} {total} {percent * 100:5.1f}%"
    bar_start = 5 + len(current) + 1
    return text[:columns].ljust(columns), bar_start, width


def progress_seek(mouse_column: int, bar_start: int, bar_width: int, duration: float) -> float:
    relative = max(0, min(bar_width - 1, mouse_column - bar_start))
    return duration * relative / max(1, bar_width - 1)


def format_time(seconds: float, milliseconds: bool = False) -> str:
    seconds = max(0.0, seconds)
    minutes = int(seconds) // 60
    whole_seconds = int(seconds) % 60
    if milliseconds:
        millis = int((seconds - int(seconds)) * 1000)
        return f"{minutes:02d}:{whole_seconds:02d}.{millis:03d}"
    return f"{minutes:02d}:{whole_seconds:02d}"


def clipped(value: str, width: int) -> str:
    return value[: max(0, width)].ljust(max(0, width))


def write_ui(value: str) -> None:
    sys.stdout.write("\x1b[0;39;49m" + value)
    sys.stdout.flush()


def write_video(payload: bytes) -> int:
    started = time.perf_counter_ns()
    sys.stdout.buffer.write(payload)
    sys.stdout.buffer.flush()
    return (time.perf_counter_ns() - started) // 1_000


def draw_shell(layout: PlayerLayout, title: str, glyphs: str) -> None:
    unicode = glyphs == "unicode"
    horizontal, vertical = ("─", "│") if unicode else ("-", "|")
    corners = ("┌", "┐", "└", "┘") if unicode else ("+", "+", "+", "+")
    write_ui("\x1b[2J\x1b[H")
    if not layout.fullscreen:
        write_ui(f"\x1b[1;1H{clipped(' TRend VIDEO PLAYER  ' + title, layout.columns)}")
    top, left = layout.outer_row, layout.outer_column
    write_ui(f"\x1b[{top};{left}H{corners[0]}{horizontal * (layout.outer_width - 2)}{corners[1]}")
    for row in range(top + 1, top + layout.outer_height - 1):
        write_ui(f"\x1b[{row};{left}H{vertical}")
        write_ui(f"\x1b[{row};{left + layout.outer_width - 1}H{vertical}")
    bottom = top + layout.outer_height - 1
    write_ui(f"\x1b[{bottom};{left}H{corners[2]}{horizontal * (layout.outer_width - 2)}{corners[3]}")


def draw_information(
    layout: PlayerLayout,
    position: float,
    duration: float,
    playing: bool,
    metadata: object,
    metrics: TimedFrameMetrics | None,
    status: str,
    quality: str,
    capabilities: Capabilities,
    audio: AudioProvider,
    write_us: int,
    fps: float,
) -> tuple[int, int]:
    line, bar_start, bar_width = progress_bar(position, duration, layout.columns)
    symbol = ">" if playing else "||"
    line = line.replace("||", symbol, 1)
    write_ui(f"\x1b[{layout.progress_row};1H{line}")
    if layout.fullscreen:
        return bar_start, bar_width
    assert layout.controls_row is not None
    assert layout.status_row is not None
    assert layout.debug_row is not None
    controls = " Space play/pause  Left/Right seek  T/Y volume  N next  R restart  F fullscreen  Q quit"
    write_ui(f"\x1b[{layout.controls_row};1H{clipped(controls, layout.columns)}")
    write_ui(f"\x1b[{layout.status_row};1H{clipped(' ' + status, layout.columns)}")
    if metrics is None:
        timing = " waiting for first timed frame"
        resources = ""
    else:
        requested = metrics.requested_ns / 1_000_000_000
        actual = metrics.presentation_ns / 1_000_000_000
        drift_ms = (actual - position) * 1000
        frame_duration = 0 if metrics.duration_ns is None else metrics.duration_ns / 1_000_000
        timing = (
            f" AUDIO {format_time(audio.reported_position(position), True)}  "
            f"REQUEST {format_time(requested, True)}  ACTUAL {format_time(actual, True)}  "
            f"DRIFT {drift_ms:+.1f}ms  FRAME {frame_duration:.1f}ms"
        )
        resources = (
            f" FPS {fps:.1f}  acquire {metrics.acquisition_us / 1000:.2f}ms  "
            f"render {metrics.render_us / 1000:.2f}ms  ANSI {metrics.ansi_us / 1000:.2f}ms  "
            f"write {write_us / 1000:.2f}ms  drop {metrics.dropped_frames}  "
            f"buffer {metrics.buffered_frames}  retained {metrics.retained_bytes}B"
        )
    write_ui(f"\x1b[{layout.debug_row};1H{clipped(timing, layout.columns)}")
    if layout.debug_row + 1 <= layout.rows:
        write_ui(f"\x1b[{layout.debug_row + 1};1H{clipped(resources, layout.columns)}")
    metadata_line = (
        f" {quality.upper()} {capabilities.color}/{capabilities.glyphs}  "
        f"FPS {metadata.nominal_fps or 'variable'}  seek {metadata.seek_accuracy}  "
        f"volume {audio.volume}%  audio {'present' if metadata.audio_present else 'none'}: {audio.detail}"
    )
    if layout.debug_row + 2 <= layout.rows:
        write_ui(f"\x1b[{layout.debug_row + 2};1H{clipped(metadata_line, layout.columns)}")
    return bar_start, bar_width


def choose_audio(path: Path, audio_present: bool, disabled: bool) -> AudioProvider:
    executable = shutil.which("ffplay")
    if not disabled and audio_present and executable:
        return FfplayAudioProvider(executable, path)
    return AudioProvider()


def reset_and_draw(
    client: BridgeClient,
    lock: threading.Lock,
    layout: PlayerLayout,
    title: str,
    capabilities: Capabilities,
) -> None:
    with lock:
        client.reset_presenter()
    draw_shell(layout, title, capabilities.glyphs)


def playlist_from_args(videos: list[Path]) -> tuple[Path, ...]:
    return tuple(videos) if videos else DEFAULT_PLAYLIST


def run_video(args: argparse.Namespace, video: Path) -> int:
    executable = ensure_bridge()
    client = BridgeClient(executable, video)
    lock = threading.Lock()
    try:
        with lock:
            metadata = client.metadata()
    except Exception as error:
        client.close()
        print(f"Video backend unavailable on this platform: {error}")
        print("TRend media/timing APIs are available; MP4 decoding is not.")
        return 2
    if metadata.duration_ns is None:
        client.close()
        print("Selected media has no playback duration.")
        return 2

    duration = metadata.duration_ns / 1_000_000_000
    clock = BridgeClock(client, lock, duration)
    worker = RenderWorker(client, lock)
    audio = choose_audio(video, metadata.audio_present, args.no_audio)
    audio.start(0.0)
    playing = True
    fullscreen = False
    quality = "balanced"
    capabilities = Capabilities(args.color, args.glyphs)
    generation = 1
    status = "playing; audio is external to TRend"
    metrics = None
    write_us = 0
    frame_count = 0
    fps_started = time.monotonic()
    last_size = None
    bar_start = bar_width = 0
    next_request = 0.0
    worker.update(RenderJob(generation, 0.0, calculate_layout(80, 24, False).video, quality, capabilities))

    try:
        with TerminalSession() as terminal:
            while True:
                terminal_size = shutil.get_terminal_size((80, 24))
                size = (terminal_size.columns, terminal_size.lines)
                layout = calculate_layout(*size, fullscreen)
                if size != last_size:
                    generation += 1
                    position = clock.position()
                    reset_and_draw(client, lock, layout, video.name, capabilities)
                    worker.update(RenderJob(generation, position, layout.video, quality, capabilities))
                    status = f"resized to {layout.video.preview_width}x{layout.video.preview_height}; decoder PID unchanged"
                    last_size = size

                position = clock.position()
                if playing and position >= duration:
                    position = clock.pause()
                    playing = False
                    audio.pause()
                    status = "end of media"
                now = time.monotonic()
                if playing and now >= next_request:
                    worker.update(RenderJob(generation, position, layout.video, quality, capabilities))
                    next_request = now + 1 / 30

                for result in worker.take_results():
                    if result.generation != generation:
                        continue
                    if result.error:
                        status = result.error
                    elif result.metrics is not None:
                        if result.payload:
                            write_us = write_video(result.payload)
                        metrics = result.metrics
                        frame_count += 1
                        if result.seek_accuracy:
                            requested = result.metrics.requested_ns / 1_000_000_000
                            actual = result.metrics.presentation_ns / 1_000_000_000
                            status = (
                                f"seek requested {requested:.6f}s, actual {actual:.6f}s, "
                                f"{result.seek_accuracy}, {result.seek_us / 1000:.2f}ms"
                            )

                elapsed = max(0.001, time.monotonic() - fps_started)
                bar_start, bar_width = draw_information(
                    layout,
                    position,
                    duration,
                    playing,
                    metadata,
                    metrics,
                    status,
                    quality,
                    capabilities,
                    audio,
                    write_us,
                    frame_count / elapsed,
                )
                event = terminal.read_event(0.015)
                if event == "quit":
                    return 0
                if event == "next":
                    return NEXT_VIDEO
                if event == "toggle":
                    generation += 1
                    if playing:
                        position = clock.pause()
                        audio.pause()
                        playing = False
                        status = "paused"
                    else:
                        position = clock.play()
                        audio.resume(position)
                        playing = True
                        status = "playing"
                    worker.update(RenderJob(generation, position, layout.video, quality, capabilities))
                elif event in ("volume_down", "volume_up"):
                    change = -VOLUME_STEP if event == "volume_down" else VOLUME_STEP
                    audio.set_volume(audio.volume + change)
                    audio.seek(position, playing)
                    status = f"volume {audio.volume}%"
                elif event in ("seek_back", "seek_forward", "seek_large_back", "seek_large_forward"):
                    offsets = {
                        "seek_back": -5.0,
                        "seek_forward": 5.0,
                        "seek_large_back": -30.0,
                        "seek_large_forward": 30.0,
                    }
                    generation += 1
                    target = clock.seek(position + offsets[event])
                    audio.seek(target, playing)
                    worker.update(RenderJob(generation, target, layout.video, quality, capabilities, True))
                    status = f"seeking to {target:.3f}s"
                elif event == "restart":
                    generation += 1
                    target = clock.restart()
                    audio.seek(target, playing)
                    worker.update(RenderJob(generation, target, layout.video, quality, capabilities, True))
                    status = "restarted"
                elif event == "fullscreen":
                    generation += 1
                    fullscreen = not fullscreen
                    layout = calculate_layout(*size, fullscreen)
                    reset_and_draw(client, lock, layout, video.name, capabilities)
                    worker.update(RenderJob(generation, position, layout.video, quality, capabilities))
                    status = "fullscreen" if fullscreen else "player view"
                elif event in ("quality_fast", "quality_balanced", "quality_quality"):
                    generation += 1
                    quality = event.removeprefix("quality_")
                    worker.update(RenderJob(generation, position, layout.video, quality, capabilities))
                    status = f"quality {quality}"
                elif event == "color":
                    generation += 1
                    color = COLOR_MODES[(COLOR_MODES.index(capabilities.color) + 1) % len(COLOR_MODES)]
                    capabilities = Capabilities(color, capabilities.glyphs)
                    reset_and_draw(client, lock, layout, video.name, capabilities)
                    worker.update(RenderJob(generation, position, layout.video, quality, capabilities))
                elif event == "glyph":
                    generation += 1
                    glyphs = "ascii" if capabilities.glyphs == "unicode" else "unicode"
                    capabilities = Capabilities(capabilities.color, glyphs)
                    reset_and_draw(client, lock, layout, video.name, capabilities)
                    worker.update(RenderJob(generation, position, layout.video, quality, capabilities))
                elif isinstance(event, tuple) and event[0] == "mouse":
                    _, mouse_column, mouse_row = event
                    if mouse_row == layout.progress_row:
                        generation += 1
                        target = clock.seek(progress_seek(mouse_column, bar_start, bar_width, duration))
                        audio.seek(target, playing)
                        worker.update(RenderJob(generation, target, layout.video, quality, capabilities, True))
                        status = f"progress click seek to {target:.3f}s"
    finally:
        audio.stop()
        worker.close()
        client.close()


def run_player(args: argparse.Namespace) -> int:
    playlist = playlist_from_args(args.videos)
    index = 0
    while True:
        result = run_video(args, playlist[index])
        if result != NEXT_VIDEO:
            return result
        index = (index + 1) % len(playlist)


def run_self_test(args: argparse.Namespace) -> int:
    video = playlist_from_args(args.videos)[0]
    executable = ensure_bridge()
    client = BridgeClient(executable, video)
    process_id = client.process.pid
    capabilities = Capabilities("truecolor", "unicode")
    stage = "metadata"
    try:
        metadata = client.metadata()
        if metadata.kind != "video" or metadata.duration_ns is None:
            raise RuntimeError("fixture did not expose video metadata and duration")
        duration = metadata.duration_ns / 1_000_000_000

        stage = "playback clock"
        client.clock_seek(0.0, 1_000_000_000)
        position, _ = client.clock_position(2_000_000_000)
        assert position == 1_000_000_000
        client.clock_pause(2_000_000_000)
        assert client.clock_position(9_000_000_000)[0] == 1_000_000_000
        client.clock_play(10_000_000_000)
        assert client.clock_position(11_000_000_000)[0] == 2_000_000_000
        client.clock_restart(12_000_000_000)
        assert client.clock_position(13_000_000_000)[0] == 1_000_000_000

        stage = "representative and preview"
        normal = calculate_layout(80, 24, False)
        representative, _ = client.frame(
            PreviewRequest(
                video,
                normal.video,
                "balanced",
                capabilities,
                1,
                presentation="representative",
                duration_ms=0,
                looping=False,
            ),
            0,
        )
        preview, _ = client.frame(
            PreviewRequest(video, normal.video, "balanced", capabilities, 2), 250
        )

        stage = "exact seek"
        target = min(83.0, duration * 0.5)
        seek = client.seek(target)
        exact, metrics = client.frame_at(target, normal.video, "balanced", capabilities)
        assert seek.first_frame_ns == metrics.presentation_ns
        assert exact.endswith(b"\x1b[0m")

        stage = "resize"
        for columns, rows in ((40, 12), (80, 24), (120, 40)):
            client.reset_presenter()
            region = calculate_layout(columns, rows, False).video
            payload, resized_metrics = client.frame_at(target, region, "balanced", capabilities)
            assert payload.endswith(b"\x1b[0m")
            assert resized_metrics.requested_ns == metrics.requested_ns

        stage = "fullscreen"
        client.reset_presenter()
        fullscreen = calculate_layout(120, 40, True).video
        full, _ = client.frame_at(target, fullscreen, "quality", capabilities)
        assert full.endswith(b"\x1b[0m")

        stage = "render modes"
        for quality in QUALITY_MODES:
            for color in COLOR_MODES:
                for glyphs in ("unicode", "ascii"):
                    client.reset_presenter()
                    payload, _ = client.frame_at(
                        target,
                        Layout(40, 12, 2, 2, 20, 6, False),
                        quality,
                        Capabilities(color, glyphs),
                    )
                    assert payload.endswith(b"\x1b[0m")

        stage = "rapid seeks"
        latest = b""
        for generation, fraction in enumerate((0.1, 0.3, 0.6, 0.9), 1):
            seek_target = duration * fraction
            client.seek(seek_target)
            candidate, _ = client.frame_at(seek_target, normal.video, "fast", capabilities)
            if generation == 4:
                latest = candidate
        assert latest

        stage = "near EOF"
        near_eof = max(0.0, duration - 1.0)
        client.seek(near_eof)
        near_payload, _ = client.frame_at(near_eof, normal.video, "fast", capabilities)
        assert near_payload.endswith(b"\x1b[0m")
        past_eof_clean = False
        try:
            client.seek(duration + 1.0)
            client.frame_at(duration + 1.0, normal.video, "fast", capabilities)
        except (FrameUnavailable, RuntimeError):
            past_eof_clean = True
        assert past_eof_clean

        stage = "progress and audio fallback"
        line, bar_start, bar_width = progress_bar(duration / 2, duration, 80)
        clicked = progress_seek(bar_start + bar_width - 1, bar_start, bar_width, duration)
        assert line and abs(clicked - duration) < 0.001
        stage = "input parsing"
        assert parse_input("\x1b[<0;42;19M") == ("mouse", 42, 19)

        stage = "audio fallback"
        no_audio = AudioProvider()
        no_audio.start(0.0)
        assert no_audio.reported_position(12.5) == 12.5
        stage = "decoder reuse"
        assert client.process.pid == process_id
        stage = "frame outputs"
        assert exact and full and (representative or preview)

        print(
            f"PASS player pipeline: PID {process_id}, representative -> preview -> play -> "
            "seek -> resize -> fullscreen, one decoder"
        )
        print(
            f"PASS timing: requested {metrics.requested_ns / 1e9:.6f}s, "
            f"actual {metrics.presentation_ns / 1e9:.6f}s, "
            f"drift {(metrics.presentation_ns - metrics.requested_ns) / 1e6:+.2f}ms"
        )
        print(
            f"PASS metrics: acquire {metrics.acquisition_us / 1000:.2f}ms, "
            f"render {metrics.render_us / 1000:.2f}ms, ANSI {metrics.ansi_us / 1000:.2f}ms, "
            f"retained {metrics.retained_bytes}B, buffered {metrics.buffered_frames}, "
            f"dropped {metrics.dropped_frames}"
        )
        print("PASS controls: pause/resume/restart, rapid seek, progress click, EOF, quality/color/glyph")
        print("PASS audio-unavailable fallback: external audio disabled, PlaybackClock retained")
        return 0
    except Exception as error:
        detail = str(error)
        if "no video backend is available" in detail:
            print("Video backend unavailable on this platform.")
            print("TRend media/timing APIs are available.")
            return 0
        print(f"FAIL player self-test during {stage}: {error}")
        return 1
    finally:
        client.close()


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "videos",
        nargs="*",
        type=Path,
        help="ordered video playlist; defaults to the bundled MP4 fixtures",
    )
    parser.add_argument("--color", choices=COLOR_MODES, default="truecolor")
    parser.add_argument("--glyphs", choices=("unicode", "ascii"), default="unicode")
    parser.add_argument("--no-audio", action="store_true", help="disable external ffplay audio")
    parser.add_argument("--self-test", action="store_true", help="run deterministic integration checks without terminal/audio")
    return parser.parse_args()


if __name__ == "__main__":
    arguments = parse_args()
    raise SystemExit(run_self_test(arguments) if arguments.self_test else run_player(arguments))

#!/usr/bin/env python3
"""Small application-owned TUI for testing embedded TRend media previews."""

from __future__ import annotations

import argparse
import io
import os
import queue
import select
import shutil
import subprocess
import sys
import threading
import time
from dataclasses import dataclass
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
MATERIALS = ROOT / "tests" / "fixtures" / "media"


@dataclass(frozen=True)
class VideoEntry:
    title: str
    metadata: str
    video: Path


VIDEO_ENTRIES = (
    VideoEntry(
        "Color Systems",
        "0:01  H.264 High",
        MATERIALS / "ColorTest.mp4",
    ),
    VideoEntry(
        "Color Systems Detail",
        "synthetic fixture  H.264 High",
        MATERIALS / "ColorTest.mp4",
    ),
    VideoEntry(
        "Color Systems Preview",
        "synthetic fixture  H.264 High",
        MATERIALS / "ColorTest.mp4",
    ),
    VideoEntry(
        "Color Systems Replay",
        "looping preview  H.264 High",
        MATERIALS / "ColorTest.mp4",
    ),
)

CARD_SCALES = (50, 75, 100)

MEDIA = (
    ("PNG", MATERIALS / "ColorTest.png"),
    ("JPG", MATERIALS / "JpegPhotoScene.jpg"),
    ("GIF", MATERIALS / "ColorTest.gif"),
    ("MP4", MATERIALS / "ColorTest.mp4"),
    ("MP4", MATERIALS / "ColorTest.mp4"),
)


@dataclass(frozen=True)
class Capabilities:
    color: str
    glyphs: str


@dataclass(frozen=True)
class Layout:
    columns: int
    rows: int
    preview_row: int
    preview_column: int
    preview_width: int
    preview_height: int
    compact: bool


@dataclass(frozen=True)
class PreviewRequest:
    path: Path | None
    layout: Layout
    quality: str
    capabilities: Capabilities
    token: int
    presentation: str = "clip"
    start_ms: int = 0
    duration_ms: int = 3_000
    looping: bool = True


@dataclass(frozen=True)
class FrameMetrics:
    decode_us: int
    render_us: int
    ansi_us: int
    ansi_bytes: int
    retained_bytes: int
    decoded_frames: int
    dropped_frames: int
    buffered_frames: int
    process_bytes: int | None

    def summary(self) -> str:
        memory = (
            f" rss {self.process_bytes / (1024 * 1024):.1f} MiB"
            if self.process_bytes is not None
            else ""
        )
        return (
            f"decode {self.decode_us / 1000:.2f} ms  render {self.render_us / 1000:.2f} ms  "
            f"ANSI {self.ansi_us / 1000:.2f} ms/{self.ansi_bytes} B  "
            f"dropped {self.dropped_frames}{memory}"
        )


@dataclass(frozen=True)
class MediaMetadata:
    kind: str
    width: int
    height: int
    duration_ns: int | None
    nominal_fps: tuple[int, int] | None
    audio_present: bool
    seek_accuracy: str


@dataclass(frozen=True)
class SeekMetrics:
    requested_ns: int
    first_frame_ns: int | None
    accuracy: str
    seek_us: int
    discarded_frames: int


@dataclass(frozen=True)
class TimedFrameMetrics:
    requested_ns: int
    presentation_ns: int
    duration_ns: int | None
    acquisition_us: int
    render_us: int
    ansi_us: int
    ansi_bytes: int
    retained_bytes: int
    decoded_frames: int
    dropped_frames: int
    buffered_frames: int
    seek_discarded_frames: int


class FrameUnavailable(RuntimeError):
    pass


@dataclass(frozen=True)
class CardRect:
    row: int
    column: int
    width: int
    thumbnail_height: int


def grid_cards(layout: Layout, scale: int = 100) -> tuple[CardRect, ...]:
    if layout.compact or layout.columns < 72:
        return ()
    scale = min(CARD_SCALES, key=lambda candidate: abs(candidate - scale))
    maximum_width = max(8, (layout.columns - 8) // 2)
    maximum_height = max(2, (layout.rows - 13) // 2)
    card_width = max(8, maximum_width * scale // 100)
    thumbnail_height = max(2, maximum_height * scale // 100)
    grid_width = card_width * 2 + 4
    first_column = max(2, (layout.columns - grid_width) // 2 + 2)
    row_gap = thumbnail_height + 4
    return tuple(
        CardRect(
            4 + (index // 2) * row_gap,
            first_column + (index % 2) * (card_width + 2),
            card_width,
            thumbnail_height,
        )
        for index in range(len(VIDEO_ENTRIES))
    )


def move_selection(selected: int, key: str) -> int:
    row, column = divmod(selected, 2)
    if key == "left":
        column = (column - 1) % 2
    elif key == "right":
        column = (column + 1) % 2
    elif key == "up":
        row = (row - 1) % 2
    elif key == "down":
        row = (row + 1) % 2
    return row * 2 + column


def resize_card_scale(scale: int, grow: bool) -> int:
    index = CARD_SCALES.index(scale)
    next_index = min(len(CARD_SCALES) - 1, index + 1) if grow else max(0, index - 1)
    return CARD_SCALES[next_index]


def fullscreen_layout(layout: Layout) -> Layout:
    return Layout(
        layout.columns,
        layout.rows,
        2,
        1,
        layout.columns,
        max(1, layout.rows - 4),
        layout.compact,
    )


def detect_capabilities(
    color: str, glyphs: str, environ: dict[str, str] | None = None
) -> Capabilities:
    environ = os.environ if environ is None else environ
    term = environ.get("TERM", "")
    color_term = environ.get("COLORTERM", "").lower()
    if color == "auto":
        if "NO_COLOR" in environ or term.lower() == "dumb":
            color = "mono"
        elif color_term in ("truecolor", "24bit"):
            color = "truecolor"
        elif "256color" in term.lower():
            color = "256"
        elif term:
            color = "16"
        else:
            color = "mono"
    if glyphs == "auto":
        encoding = (getattr(sys.stdout, "encoding", None) or "").lower()
        glyphs = "unicode" if "utf" in encoding and term.lower() != "dumb" else "ascii"
    return Capabilities(color, glyphs)


def calculate_layout(columns: int, rows: int) -> Layout:
    columns = max(1, columns)
    rows = max(1, rows)
    compact = columns < 72 or rows < 22
    if compact:
        return Layout(columns, rows, 1, 1, 0, 0, True)
    if columns >= 72:
        preview_column = min(58, max(38, columns // 2))
        return Layout(
            columns,
            rows,
            4,
            preview_column,
            max(1, columns - preview_column - 2),
            max(1, rows - 10),
            False,
        )
    return Layout(columns, rows, 10, 3, max(1, columns - 4), max(1, rows - 13), False)


def bridge_path() -> Path:
    suffix = ".exe" if os.name == "nt" else ""
    return ROOT / "target" / "debug" / f"trend-python-tui-bridge{suffix}"


def ensure_bridge() -> Path:
    bridge = bridge_path()
    subprocess.run(
        ["cargo", "build", "-p", "trend-python-tui-bridge"],
        cwd=ROOT,
        check=True,
    )
    return bridge


class BridgeClient:
    def __init__(self, executable: Path, media: Path) -> None:
        self.process = subprocess.Popen(
            [str(executable), str(media)],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            bufsize=0,
        )

    def frame(self, request: PreviewRequest, elapsed_ms: int) -> tuple[bytes, FrameMetrics]:
        assert self.process.stdin is not None
        assert self.process.stdout is not None
        layout = request.layout
        command = (
            f"FRAME {layout.preview_width} {layout.preview_height} "
            f"{layout.preview_row} {layout.preview_column} {request.quality} "
            f"{request.capabilities.color} {request.capabilities.glyphs} {elapsed_ms} "
            f"{request.presentation} {request.start_ms} {request.duration_ms} "
            f"{'loop' if request.looping else 'once'}\n"
        )
        self.process.stdin.write(command.encode("ascii"))
        self.process.stdin.flush()
        header = self.process.stdout.readline().decode("utf-8", "replace").rstrip()
        if header.startswith("OK "):
            (
                _,
                length,
                decode_us,
                render_us,
                ansi_us,
                retained_bytes,
                decoded_frames,
                dropped_frames,
                buffered_frames,
            ) = header.split()
            payload = self.process.stdout.read(int(length))
            if len(payload) != int(length):
                raise RuntimeError("bridge ended during an ANSI frame")
            return payload, FrameMetrics(
                int(decode_us),
                int(render_us),
                int(ansi_us),
                len(payload),
                int(retained_bytes),
                int(decoded_frames),
                int(dropped_frames),
                int(buffered_frames),
                process_memory_bytes(self.process.pid),
            )
        if header == "WAIT":
            return b"", FrameMetrics(0, 0, 0, 0, 0, 0, 0, 0, process_memory_bytes(self.process.pid))
        if header.startswith("ERR "):
            raise RuntimeError(header[4:])
        if not header:
            assert self.process.stderr is not None
            detail = self.process.stderr.read().decode("utf-8", "replace").strip()
            raise RuntimeError(detail or "bridge exited")
        raise RuntimeError(f"invalid bridge response: {header}")

    def metadata(self) -> MediaMetadata:
        header = self._request_header("META")
        fields = header.split()
        if len(fields) != 9 or fields[0] != "META_OK":
            raise RuntimeError(f"invalid metadata response: {header}")
        _, kind, width, height, duration_ns, fps_num, fps_den, audio, accuracy = fields
        fps = None if fps_num == "0" or fps_den == "0" else (int(fps_num), int(fps_den))
        return MediaMetadata(
            kind,
            int(width),
            int(height),
            optional_nanoseconds(duration_ns),
            fps,
            audio == "1",
            accuracy,
        )

    def seek(self, timestamp: float) -> SeekMetrics:
        requested_ns = seconds_to_nanoseconds(timestamp)
        header = self._request_header(f"SEEK {requested_ns}")
        fields = header.split()
        if len(fields) != 6 or fields[0] != "SEEK_OK":
            raise RuntimeError(f"invalid seek response: {header}")
        _, requested, first_frame, accuracy, seek_us, discarded = fields
        return SeekMetrics(
            int(requested),
            optional_nanoseconds(first_frame),
            accuracy,
            int(seek_us),
            int(discarded),
        )

    def frame_at(
        self,
        timestamp: float,
        layout: Layout,
        quality: str,
        capabilities: Capabilities,
    ) -> tuple[bytes, TimedFrameMetrics]:
        requested_ns = seconds_to_nanoseconds(timestamp)
        header = self._request_header(
            f"FRAME_AT {layout.preview_width} {layout.preview_height} "
            f"{layout.preview_row} {layout.preview_column} {quality} "
            f"{capabilities.color} {capabilities.glyphs} {requested_ns}"
        )
        if header == "WAIT":
            raise FrameUnavailable("no frame is available at the requested timestamp")
        fields = header.split()
        if len(fields) != 13 or fields[0] != "FRAME_OK":
            raise RuntimeError(f"invalid frame-at response: {header}")
        (
            _,
            length,
            requested,
            presentation,
            duration,
            acquisition_us,
            render_us,
            ansi_us,
            retained_bytes,
            decoded_frames,
            dropped_frames,
            buffered_frames,
            seek_discarded_frames,
        ) = fields
        assert self.process.stdout is not None
        payload = self.process.stdout.read(int(length))
        if len(payload) != int(length):
            raise RuntimeError("bridge ended during an exact-timestamp ANSI frame")
        return payload, TimedFrameMetrics(
            int(requested),
            int(presentation),
            optional_nanoseconds(duration),
            int(acquisition_us),
            int(render_us),
            int(ansi_us),
            len(payload),
            int(retained_bytes),
            int(decoded_frames),
            int(dropped_frames),
            int(buffered_frames),
            int(seek_discarded_frames),
        )

    def clock_position(self, wall_ns: int | None = None) -> tuple[int, bool]:
        return self._clock("POSITION", wall_ns=wall_ns)

    def clock_pause(self, wall_ns: int | None = None) -> tuple[int, bool]:
        return self._clock("PAUSE", wall_ns=wall_ns)

    def clock_play(self, wall_ns: int | None = None) -> tuple[int, bool]:
        return self._clock("PLAY", wall_ns=wall_ns)

    def clock_seek(self, media_seconds: float, wall_ns: int | None = None) -> tuple[int, bool]:
        return self._clock(
            "SEEK", seconds_to_nanoseconds(media_seconds), wall_ns=wall_ns
        )

    def clock_restart(self, wall_ns: int | None = None) -> tuple[int, bool]:
        return self._clock("RESTART", wall_ns=wall_ns)

    def reset_presenter(self) -> None:
        header = self._request_header("RESET")
        if header != "RESET_OK":
            raise RuntimeError(f"invalid reset response: {header}")

    def _clock(
        self, operation: str, media_ns: int | None = None, wall_ns: int | None = None
    ) -> tuple[int, bool]:
        wall_ns = time.monotonic_ns() if wall_ns is None else wall_ns
        middle = "" if media_ns is None else f" {media_ns}"
        header = self._request_header(f"CLOCK {operation}{middle} {wall_ns}")
        fields = header.split()
        if len(fields) != 3 or fields[0] != "CLOCK_OK":
            raise RuntimeError(f"invalid clock response: {header}")
        return int(fields[1]), fields[2] == "1"

    def _request_header(self, command: str) -> str:
        assert self.process.stdin is not None
        assert self.process.stdout is not None
        self.process.stdin.write((command + "\n").encode("ascii"))
        self.process.stdin.flush()
        header = self.process.stdout.readline().decode("utf-8", "replace").rstrip()
        if header.startswith("ERR "):
            raise RuntimeError(header[4:])
        if not header:
            assert self.process.stderr is not None
            detail = self.process.stderr.read().decode("utf-8", "replace").strip()
            raise RuntimeError(detail or "bridge exited")
        return header

    def close(self) -> None:
        if self.process.poll() is None and self.process.stdin is not None:
            try:
                self.process.stdin.write(b"QUIT\n")
                self.process.stdin.flush()
            except (BrokenPipeError, OSError):
                pass
        try:
            self.process.wait(timeout=1)
        except subprocess.TimeoutExpired:
            self.process.terminate()
            self.process.wait(timeout=1)


def seconds_to_nanoseconds(value: float) -> int:
    if value < 0:
        raise ValueError("timestamp must not be negative")
    return round(value * 1_000_000_000)


def optional_nanoseconds(value: str) -> int | None:
    parsed = int(value)
    return None if parsed < 0 else parsed


def process_memory_bytes(process_id: int) -> int | None:
    if os.name == "nt":
        import ctypes

        class ProcessMemoryCounters(ctypes.Structure):
            _fields_ = [
                ("cb", ctypes.c_ulong),
                ("page_fault_count", ctypes.c_ulong),
                ("peak_working_set_size", ctypes.c_size_t),
                ("working_set_size", ctypes.c_size_t),
                ("quota_peak_paged_pool_usage", ctypes.c_size_t),
                ("quota_paged_pool_usage", ctypes.c_size_t),
                ("quota_peak_non_paged_pool_usage", ctypes.c_size_t),
                ("quota_non_paged_pool_usage", ctypes.c_size_t),
                ("pagefile_usage", ctypes.c_size_t),
                ("peak_pagefile_usage", ctypes.c_size_t),
            ]

        process = ctypes.windll.kernel32.OpenProcess(0x0410, False, process_id)
        if not process:
            return None
        counters = ProcessMemoryCounters()
        counters.cb = ctypes.sizeof(counters)
        try:
            if ctypes.windll.psapi.GetProcessMemoryInfo(
                process, ctypes.byref(counters), counters.cb
            ):
                return int(counters.working_set_size)
        finally:
            ctypes.windll.kernel32.CloseHandle(process)
        return None
    statm = Path(f"/proc/{process_id}/statm")
    if statm.exists():
        try:
            resident_pages = int(statm.read_text(encoding="ascii").split()[1])
            return resident_pages * int(os.sysconf("SC_PAGE_SIZE"))
        except (OSError, ValueError, IndexError):
            return None
    return None


class PreviewWorker:
    def __init__(self, executable: Path) -> None:
        self.executable = executable
        self.requests: queue.Queue[PreviewRequest | None] = queue.Queue(maxsize=1)
        self.results: queue.Queue[tuple[int, bytes, FrameMetrics | str]] = queue.Queue(maxsize=2)
        self.thread = threading.Thread(target=self._run, name="trend-tui-preview", daemon=True)
        self.thread.start()

    def update(self, request: PreviewRequest) -> None:
        while True:
            try:
                self.requests.get_nowait()
            except queue.Empty:
                break
        self.requests.put_nowait(request)

    def take_results(self) -> list[tuple[int, bytes, FrameMetrics | str]]:
        results = []
        while True:
            try:
                results.append(self.results.get_nowait())
            except queue.Empty:
                return results

    def close(self) -> None:
        try:
            self.requests.put_nowait(None)
        except queue.Full:
            try:
                self.requests.get_nowait()
            except queue.Empty:
                pass
            self.requests.put_nowait(None)
        self.thread.join(timeout=2)

    def _publish(self, result: tuple[int, bytes, FrameMetrics | str]) -> None:
        try:
            self.results.put_nowait(result)
        except queue.Full:
            try:
                self.results.get_nowait()
            except queue.Empty:
                pass
            self.results.put_nowait(result)

    def _run(self) -> None:
        request: PreviewRequest | None = None
        client: BridgeClient | None = None
        opened_at = time.monotonic()
        while True:
            try:
                replacement = self.requests.get(timeout=0.08 if request else None)
                if replacement is None:
                    break
                if request is None or replacement.path != request.path:
                    if client:
                        client.close()
                    client = None
                    opened_at = time.monotonic()
                request = replacement
            except queue.Empty:
                pass
            if request is None or request.path is None or request.layout.compact:
                if client:
                    client.close()
                    client = None
                continue
            try:
                if client is None:
                    client = BridgeClient(self.executable, request.path)
                elapsed_ms = int((time.monotonic() - opened_at) * 1000)
                payload, status = client.frame(request, elapsed_ms)
                self._publish((request.token, payload, status))
            except Exception as error:  # The error belongs in the application status bar.
                self._publish((request.token, b"", f"preview error: {error}"))
                if client:
                    client.close()
                    client = None
                time.sleep(0.15)
        if client:
            client.close()


class ThumbnailWorker:
    def __init__(self, executable: Path) -> None:
        self.executable = executable
        self.requests: queue.Queue[tuple[int, Layout, Capabilities, int] | None] = queue.Queue(maxsize=1)
        self.results: queue.Queue[tuple[int, bytes, str]] = queue.Queue(maxsize=1)
        self.thread = threading.Thread(target=self._run, name="trend-tui-thumbnails", daemon=True)
        self.thread.start()

    def update(
        self, token: int, layout: Layout, capabilities: Capabilities, card_scale: int
    ) -> None:
        while True:
            try:
                self.requests.get_nowait()
            except queue.Empty:
                break
        self.requests.put_nowait((token, layout, capabilities, card_scale))

    def take_results(self) -> list[tuple[int, bytes, str]]:
        results = []
        while True:
            try:
                results.append(self.results.get_nowait())
            except queue.Empty:
                return results

    def close(self) -> None:
        try:
            self.requests.put_nowait(None)
        except queue.Full:
            self.requests.get_nowait()
            self.requests.put_nowait(None)
        self.thread.join(timeout=3)

    def _run(self) -> None:
        clients: dict[Path, BridgeClient] = {}
        try:
            while True:
                request = self.requests.get()
                if request is None:
                    return
                token, layout, capabilities, card_scale = request
                payloads = []
                for entry, card in zip(VIDEO_ENTRIES, grid_cards(layout, card_scale)):
                    client = clients.get(entry.video)
                    if client is None:
                        client = BridgeClient(self.executable, entry.video)
                        clients[entry.video] = client
                    thumbnail_layout = Layout(
                        layout.columns,
                        layout.rows,
                        card.row,
                        card.column,
                        card.width,
                        card.thumbnail_height,
                        False,
                    )
                    payload, _ = client.frame(
                        PreviewRequest(
                            entry.video,
                            thumbnail_layout,
                            "balanced",
                            capabilities,
                            token,
                            presentation="representative",
                            duration_ms=0,
                            looping=False,
                        ),
                        0,
                    )
                    payloads.append(payload)
                result = (token, b"".join(payloads), "first-frame cache reused")
                try:
                    self.results.put_nowait(result)
                except queue.Full:
                    self.results.get_nowait()
                    self.results.put_nowait(result)
        except Exception as error:
            try:
                self.results.put_nowait((token, b"", f"thumbnail error: {error}"))
            except queue.Full:
                self.results.get_nowait()
                self.results.put_nowait((token, b"", f"thumbnail error: {error}"))
        finally:
            for client in clients.values():
                client.close()


class TerminalSession:
    def __init__(self) -> None:
        self.posix_settings = None
        self.windows_handle = None
        self.windows_mode = None

    def __enter__(self) -> "TerminalSession":
        if not sys.stdin.isatty() or not sys.stdout.isatty():
            raise RuntimeError("interactive mode requires a terminal; use --probe for CI")
        if os.name == "nt":
            self._enable_windows_vt()
        else:
            import termios
            import tty

            self.posix_settings = termios.tcgetattr(sys.stdin.fileno())
            tty.setcbreak(sys.stdin.fileno())
        write_text("\x1b[?1049h\x1b[?25l\x1b[2J")
        return self

    def __exit__(self, *_: object) -> None:
        write_text("\x1b[0m\x1b[?25h\x1b[?1049l")
        if os.name == "nt" and self.windows_handle is not None:
            import ctypes

            ctypes.windll.kernel32.SetConsoleMode(self.windows_handle, self.windows_mode)
        elif self.posix_settings is not None:
            import termios

            termios.tcsetattr(sys.stdin.fileno(), termios.TCSADRAIN, self.posix_settings)

    def read_key(self, timeout: float) -> str | None:
        if os.name == "nt":
            import msvcrt

            deadline = time.monotonic() + timeout
            while time.monotonic() < deadline:
                if msvcrt.kbhit():
                    key = msvcrt.getwch()
                    if key in ("\x00", "\xe0"):
                        return {"H": "up", "P": "down", "K": "left", "M": "right"}.get(
                            msvcrt.getwch()
                        )
                    return {"\r": "enter", " ": "stop"}.get(key, key.lower())
                time.sleep(0.005)
            return None
        ready, _, _ = select.select([sys.stdin], [], [], timeout)
        if not ready:
            return None
        data = os.read(sys.stdin.fileno(), 8).decode("utf-8", "ignore")
        if data.startswith("\x1b[A"):
            return "up"
        if data.startswith("\x1b[B"):
            return "down"
        if data.startswith("\x1b[C"):
            return "right"
        if data.startswith("\x1b[D"):
            return "left"
        return {"\r": "enter", "\n": "enter", " ": "stop"}.get(data, data.lower())

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


def write_text(value: str) -> None:
    sys.stdout.write("\x1b[0;39;49m" + value)
    sys.stdout.flush()


def write_bytes(value: bytes) -> int:
    started = time.perf_counter_ns()
    sys.stdout.buffer.write(value)
    sys.stdout.buffer.flush()
    return (time.perf_counter_ns() - started) // 1_000


def clipped(value: str, width: int) -> str:
    return value[: max(0, width)].ljust(max(0, width))


def draw_shell(
    layout: Layout,
    selected: int,
    status: str,
    glyphs: str,
    card_scale: int,
    clear: bool = False,
) -> None:
    write_text("\x1b[2J\x1b[H" if clear else "\x1b[H")
    if layout.compact:
        lines = [
            "TERMINAL VIDEO",
            f"Terminal {layout.columns}x{layout.rows} is too small for preview.",
            "Resize to at least 72x22. Q quits.",
        ]
        for row, line in enumerate(lines[: layout.rows], 1):
            write_text(f"\x1b[{row};1H{clipped(line, layout.columns)}")
        return
    unicode = glyphs == "unicode"
    horizontal, vertical = ("─", "│") if unicode else ("-", "|")
    top_left, top_right, bottom_left, bottom_right = (
        ("┌", "┐", "└", "┘") if unicode else ("+", "+", "+", "+")
    )
    write_text(f"\x1b[1;1H{clipped(' TERMINAL VIDEO', layout.columns)}")
    write_text(f"\x1b[2;1H{horizontal * layout.columns}")
    write_text("\x1b[3;2HVideo grid")
    for index, (entry, card) in enumerate(
        zip(VIDEO_ENTRIES, grid_cards(layout, card_scale))
    ):
        card_left = card.column - 1
        card_top = card.row - 1
        write_text(
            f"\x1b[{card_top};{card_left}H{top_left}{horizontal * card.width}{top_right}"
        )
        for offset in range(card.thumbnail_height):
            write_text(f"\x1b[{card.row + offset};{card_left}H{vertical}")
            write_text(
                f"\x1b[{card.row + offset};{card.column + card.width}H{vertical}"
            )
        write_text(
            f"\x1b[{card.row + card.thumbnail_height};{card_left}H"
            f"{bottom_left}{horizontal * card.width}{bottom_right}"
        )
        marker = ">" if index == selected else " "
        write_text(
            f"\x1b[{card.row + card.thumbnail_height + 1};{card_left}H"
            f"{clipped(marker + ' ' + entry.title, card.width + 2)}"
        )
        write_text(
            f"\x1b[{card.row + card.thumbnail_height + 2};{card_left}H"
            f"{clipped(entry.metadata, card.width + 2)}"
        )
    write_text(f"\x1b[{layout.rows - 2};1H{horizontal * layout.columns}")
    write_text(
        f"\x1b[{layout.rows - 1};1H{clipped(' Arrows navigate  Enter fullscreen  +/- resize cards  Space stop  1/2/3 quality  C color  U glyphs  Q quit', layout.columns)}"
    )
    draw_status(layout, status)


def draw_status(layout: Layout, status: str) -> None:
    if not layout.compact and layout.rows >= 2:
        write_text(f"\x1b[{layout.rows};1H{clipped(' ' + status, layout.columns)}")


def card_layout(layout: Layout, selected: int, scale: int = 100) -> Layout:
    card = grid_cards(layout, scale)[selected]
    return Layout(
        layout.columns,
        layout.rows,
        card.row,
        card.column,
        card.width,
        card.thumbnail_height,
        False,
    )


def thumbnail_surface_key(
    layout: Layout, capabilities: Capabilities, card_scale: int
) -> tuple[Layout, Capabilities, int]:
    return layout, capabilities, card_scale


def draw_fullscreen(layout: Layout, entry: VideoEntry, status: str) -> None:
    write_text("\x1b[2J\x1b[H")
    write_text(f"\x1b[1;1H{clipped(' FULL VIDEO  ' + entry.title, layout.columns)}")
    write_text(
        f"\x1b[{layout.rows - 1};1H"
        f"{clipped(' Esc return to thumbnails  Space stop  Q quit', layout.columns)}"
    )
    draw_status(layout, status)


def run_interactive(args: argparse.Namespace) -> int:
    executable = ensure_bridge()
    capabilities = detect_capabilities(args.color, args.glyphs)
    selected = 0
    quality = "balanced"
    token = 1
    stopped = False
    fullscreen = False
    card_scale = 75
    status = "selected preview starts automatically"
    worker = PreviewWorker(executable)
    thumbnails = ThumbnailWorker(executable)
    try:
        with TerminalSession() as terminal:
            last_layout = None
            last_state = None
            thumbnail_generation = 0
            thumbnail_payload = b""
            thumbnail_payload_key = None
            frame_count = 0
            fps_started = time.monotonic()
            while True:
                size = shutil.get_terminal_size((80, 24))
                layout = calculate_layout(size.columns, size.lines)
                layout_changed = layout != last_layout
                if layout_changed and last_layout is not None:
                    token += 1
                    status = "resize: preview viewport updated without reopening decoder"
                cards_changed = bool(
                    last_state is not None and last_state[6] != card_scale
                )
                thumbnail_key = thumbnail_surface_key(layout, capabilities, card_scale)
                if thumbnail_payload_key != thumbnail_key:
                    thumbnail_payload = b""
                state = (
                    layout,
                    selected,
                    quality,
                    capabilities,
                    stopped,
                    fullscreen,
                    card_scale,
                    token,
                )
                if state != last_state:
                    if fullscreen:
                        draw_fullscreen(layout, VIDEO_ENTRIES[selected], status)
                    else:
                        draw_shell(
                            layout,
                            selected,
                            status,
                            capabilities.glyphs,
                            card_scale,
                            clear=layout_changed or cards_changed,
                        )
                        if thumbnail_payload:
                            write_bytes(thumbnail_payload)
                    output_layout = (
                        fullscreen_layout(layout)
                        if fullscreen
                        else card_layout(layout, selected, card_scale)
                    )
                    worker.update(
                        PreviewRequest(
                            None if stopped else VIDEO_ENTRIES[selected].video,
                            output_layout,
                            quality,
                            capabilities,
                            token,
                            duration_ms=(2**63 - 1) if fullscreen else 3_000,
                            looping=not fullscreen,
                        )
                    )
                    if not fullscreen and (
                        layout_changed
                        or cards_changed
                        or (last_state and last_state[3] != capabilities)
                    ):
                        thumbnail_generation += 1
                        thumbnails.update(
                            thumbnail_generation, layout, capabilities, card_scale
                        )
                    last_layout = layout
                    last_state = state
                for result_token, payload, thumbnail_status in thumbnails.take_results():
                    if result_token == thumbnail_generation and payload:
                        thumbnail_payload = payload
                        thumbnail_payload_key = thumbnail_key
                        write_bytes(payload)
                    elif result_token == thumbnail_generation and thumbnail_status.startswith(
                        "thumbnail error"
                    ):
                        status = thumbnail_status
                        draw_status(layout, status)
                for result_token, payload, new_status in worker.take_results():
                    if result_token == token:
                        if payload:
                            presentation_us = write_bytes(payload)
                            frame_count += 1
                        else:
                            presentation_us = 0
                        if isinstance(new_status, FrameMetrics):
                            elapsed = max(0.001, time.monotonic() - fps_started)
                            status = (
                                f"{frame_count / elapsed:.1f} FPS  {new_status.summary()}  "
                                f"present {presentation_us / 1000:.2f} ms"
                            )
                        else:
                            status = new_status
                        if status:
                            draw_status(layout, status)
                key = terminal.read_key(0.03)
                if key == "q":
                    return 0
                if key == "\x1b":
                    if fullscreen:
                        fullscreen = False
                        stopped = True
                        token += 1
                        status = "thumbnail grid restored"
                    else:
                        return 0
                elif key in ("up", "down", "left", "right") and not fullscreen:
                    selected = move_selection(selected, key)
                    stopped = False
                    token += 1
                    frame_count = 0
                    fps_started = time.monotonic()
                    status = f"opening {VIDEO_ENTRIES[selected].title}"
                elif key == "enter":
                    fullscreen = True
                    stopped = False
                    token += 1
                    frame_count = 0
                    fps_started = time.monotonic()
                    status = f"full playback: {VIDEO_ENTRIES[selected].title}"
                elif key == "stop":
                    stopped = True
                    token += 1
                    status = "preview stopped; decoder released"
                elif key in ("+", "=", "-", "_") and not fullscreen:
                    resized_scale = resize_card_scale(card_scale, key in ("+", "="))
                    if resized_scale != card_scale:
                        thumbnail_payload = b""
                        card_scale = resized_scale
                        stopped = False
                        token += 1
                        status = f"video cards: {card_scale}%"
                elif key in ("1", "2", "3"):
                    quality = {"1": "fast", "2": "balanced", "3": "quality"}[key]
                    token += 1
                elif key == "c":
                    modes = ("truecolor", "256", "16", "mono")
                    capabilities = Capabilities(
                        modes[(modes.index(capabilities.color) + 1) % len(modes)],
                        capabilities.glyphs,
                    )
                    token += 1
                elif key == "u":
                    capabilities = Capabilities(
                        capabilities.color,
                        "ascii" if capabilities.glyphs == "unicode" else "unicode",
                    )
                    token += 1
    finally:
        thumbnails.close()
        worker.close()


def probe_media(
    executable: Path,
    path: Path,
    layout: Layout,
    capabilities: Capabilities,
    representative: bool = False,
) -> str:
    client = BridgeClient(executable, path)
    static = representative or path.suffix.lower() in (".png", ".jpg", ".jpeg")
    request = PreviewRequest(
        path,
        layout,
        "balanced",
        capabilities,
        1,
        presentation="representative" if static else "clip",
        duration_ms=0 if static else 3_000,
        looping=not static,
    )
    try:
        for elapsed_ms in (0, 50, 100, 250, 500):
            payload, status = client.frame(request, elapsed_ms)
            if payload:
                return f"PASS {path.name}: {status.summary()}"
        return f"FAIL {path.name}: decoder produced no frame"
    except Exception as error:
        return f"UNAVAILABLE {path.name}: {error}"
    finally:
        client.close()


def run_probe(args: argparse.Namespace) -> int:
    executable = ensure_bridge()
    columns, rows = (int(value) for value in args.size.lower().split("x", 1))
    layout = calculate_layout(columns, rows)
    capabilities = detect_capabilities(args.color, args.glyphs)
    if layout.compact:
        print(f"PASS layout {columns}x{rows}: compact fallback, no preview requested")
        return 0
    failures = 0
    for _, path in MEDIA[:4]:
        result = probe_media(executable, path, layout, capabilities)
        print(result)
        failures += not result.startswith("PASS")
    for entry, card in zip(VIDEO_ENTRIES, grid_cards(layout)):
        thumbnail_layout = Layout(
            layout.columns,
            layout.rows,
            card.row,
            card.column,
            card.width,
            card.thumbnail_height,
            False,
        )
        result = probe_media(
            executable,
            entry.video,
            thumbnail_layout,
            capabilities,
            representative=True,
        )
        print(f"THUMBNAIL {result}")
        failures += not result.startswith("PASS")
    print(
        f"layout={columns}x{rows} preview={layout.preview_width}x{layout.preview_height} "
        f"color={capabilities.color} glyphs={capabilities.glyphs}"
    )
    return 1 if failures else 0


def run_stress(args: argparse.Namespace) -> int:
    executable = ensure_bridge()
    capabilities = detect_capabilities(args.color, args.glyphs)
    layout = calculate_layout(80, 24)
    worker = PreviewWorker(executable)
    final_token = 0
    try:
        convergence_started = time.monotonic()
        for final_token, selected in enumerate((0, 1, 2, 3, 0), 1):
            worker.update(
                PreviewRequest(
                    VIDEO_ENTRIES[selected].video,
                    layout,
                    "balanced",
                    capabilities,
                    final_token,
                )
            )
            if selected == 0 and final_token > 1:
                convergence_started = time.monotonic()
            time.sleep(0.005)
        deadline = time.monotonic() + 5
        while time.monotonic() < deadline:
            for token, payload, status in worker.take_results():
                if token == final_token and payload:
                    convergence_ms = (time.monotonic() - convergence_started) * 1000
                    worker.update(
                        PreviewRequest(None, layout, "balanced", capabilities, final_token + 1)
                    )
                    print(
                        f"PASS rapid switching: latest token {token}, "
                        f"converged in {convergence_ms:.2f} ms, {len(payload)} ANSI bytes, "
                        f"{status.summary()}"
                    )
                    print("PASS stop request: preview bridge scheduled for release")
                    return 0
            time.sleep(0.01)
        print("FAIL rapid switching: latest preview did not publish within 5 seconds")
        return 1
    finally:
        worker.close()


def percentile(values: list[float], fraction: float) -> float:
    ordered = sorted(values)
    return ordered[min(len(ordered) - 1, int((len(ordered) - 1) * fraction))]


def await_frame(
    worker: PreviewWorker, token: int, timeout: float
) -> tuple[FrameMetrics, float] | None:
    started = time.monotonic()
    deadline = started + timeout
    while time.monotonic() < deadline:
        for result_token, payload, result in worker.take_results():
            if result_token == token and payload and isinstance(result, FrameMetrics):
                return result, time.monotonic() - started
        time.sleep(0.002)
    return None


def run_benchmark(args: argparse.Namespace) -> int:
    executable = ensure_bridge()
    capabilities = detect_capabilities(args.color, args.glyphs)
    worker = PreviewWorker(executable)
    initial_layout = calculate_layout(80, 24)
    path = VIDEO_ENTRIES[2].video
    token = 1
    worker.update(PreviewRequest(path, initial_layout, "balanced", capabilities, token))
    loop_gaps = []
    metrics = []
    started = previous = time.monotonic()
    try:
        while time.monotonic() - started < 2.0:
            now = time.monotonic()
            loop_gaps.append(now - previous)
            previous = now
            for result_token, payload, result in worker.take_results():
                if result_token == token and payload and isinstance(result, FrameMetrics):
                    metrics.append(result)
            time.sleep(0.005)
        if not metrics:
            print("FAIL benchmark: preview produced no frames")
            return 1
        before_resize_decoded = metrics[-1].decoded_frames
        token += 1
        resized_layout = calculate_layout(120, 40)
        resize_started = time.monotonic()
        worker.update(PreviewRequest(path, resized_layout, "balanced", capabilities, token))
        resized = await_frame(worker, token, 5.0)
        if resized is None:
            print("FAIL benchmark: resized preview produced no frame")
            return 1
        resized_metrics, _ = resized
        resize_ms = (time.monotonic() - resize_started) * 1000
        decode_values = [float(item.decode_us) for item in metrics]
        render_values = [float(item.render_us) for item in metrics]
        ansi_time_values = [float(item.ansi_us) for item in metrics]
        ansi_values = [item.ansi_bytes for item in metrics]
        process_values = [item.process_bytes for item in metrics if item.process_bytes is not None]
        application_bytes = process_memory_bytes(os.getpid())
        print(
            f"PASS preview: {len(metrics) / 2.0:.1f} FPS, "
            f"decode p50/p95 {percentile(decode_values, 0.5) / 1000:.2f}/"
            f"{percentile(decode_values, 0.95) / 1000:.2f} ms, "
            f"render p50/p95 {percentile(render_values, 0.5) / 1000:.2f}/"
            f"{percentile(render_values, 0.95) / 1000:.2f} ms, "
            f"ANSI p50/p95 {percentile(ansi_time_values, 0.5) / 1000:.2f}/"
            f"{percentile(ansi_time_values, 0.95) / 1000:.2f} ms"
        )
        print(
            f"PASS output: ANSI p50 {percentile([float(value) for value in ansi_values], 0.5):.0f} B, "
            f"adapter retained {metrics[-1].retained_bytes} B, "
            f"bridge RSS {max(process_values) / (1024 * 1024):.1f} MiB"
            if process_values
            else f"PASS output: ANSI p50 {percentile([float(value) for value in ansi_values], 0.5):.0f} B"
        )
        print(
            f"PASS responsiveness: UI-loop p95 {percentile(loop_gaps, 0.95) * 1000:.2f} ms; "
            f"decoder dropped {metrics[-1].dropped_frames} frames"
        )
        if application_bytes is not None:
            print(f"PASS application memory: Python RSS {application_bytes / (1024 * 1024):.1f} MiB")
        print(
            f"PASS resize: {resize_ms:.2f} ms, decoder remained live "
            f"({before_resize_decoded} -> {resized_metrics.decoded_frames} decoded frames)"
        )
        return 0
    finally:
        worker.close()


def render_once(
    executable: Path, path: Path, layout: Layout, capabilities: Capabilities
) -> bytes:
    client = BridgeClient(executable, path)
    try:
        payload, _ = client.frame(
            PreviewRequest(path, layout, "balanced", capabilities, 1), 250
        )
        return payload
    finally:
        client.close()


def run_compatibility() -> int:
    executable = ensure_bridge()
    layout = calculate_layout(80, 24)
    path = VIDEO_ENTRIES[0].video
    for color in ("truecolor", "256", "16", "mono"):
        for glyphs in ("unicode", "ascii"):
            capabilities = Capabilities(color, glyphs)
            first = render_once(executable, path, layout, capabilities)
            second = render_once(executable, path, layout, capabilities)
            if not first or first != second or not first.endswith(b"\x1b[0m"):
                print(f"FAIL deterministic output: {color}/{glyphs}")
                return 1
            print(f"PASS deterministic output: {color}/{glyphs}, {len(first)} bytes")
    try:
        render_once(executable, MATERIALS / "ColorTest.webp", layout, Capabilities("mono", "ascii"))
    except Exception as error:
        print(f"PASS unsupported WebP: {error}")
        return 0
    print("FAIL unsupported WebP: bridge accepted an unsupported format")
    return 1


def run_workflow() -> int:
    executable = ensure_bridge()
    path = VIDEO_ENTRIES[0].video
    capabilities = Capabilities("truecolor", "unicode")
    grid = calculate_layout(80, 24)
    card = grid_cards(grid)[0]
    thumbnail_layout = Layout(80, 24, card.row, card.column, card.width, card.thumbnail_height, False)
    client = BridgeClient(executable, path)
    process_id = client.process.pid
    try:
        thumbnail, thumbnail_metrics = client.frame(
            PreviewRequest(
                path,
                thumbnail_layout,
                "balanced",
                capabilities,
                1,
                presentation="representative",
                duration_ms=0,
                looping=False,
            ),
            0,
        )
        client.frame(
            PreviewRequest(path, thumbnail_layout, "balanced", capabilities, 2), 100
        )
        preview, preview_metrics = client.frame(
            PreviewRequest(path, thumbnail_layout, "balanced", capabilities, 2), 160
        )
        large = calculate_layout(120, 40)
        large_card = grid_cards(large)[0]
        resized_layout = Layout(
            120,
            40,
            large_card.row,
            large_card.column,
            large_card.width,
            large_card.thumbnail_height,
            False,
        )
        resize_started = time.monotonic()
        resized, resized_metrics = client.frame(
            PreviewRequest(path, resized_layout, "balanced", capabilities, 3), 200
        )
        resize_ms = (time.monotonic() - resize_started) * 1000
        full, full_metrics = client.frame(
            PreviewRequest(
                path,
                fullscreen_layout(large),
                "balanced",
                capabilities,
                4,
                duration_ms=2**63 - 1,
                looping=False,
            ),
            300,
        )
        restored, _ = client.frame(
            PreviewRequest(
                path,
                resized_layout,
                "balanced",
                capabilities,
                5,
                presentation="representative",
                duration_ms=0,
                looping=False,
            ),
            400,
        )
        payloads = (thumbnail, preview, resized, full, restored)
        if not all(payload and payload.endswith(b"\x1b[0m") for payload in payloads):
            print("FAIL workflow: missing positioned output or terminal-state restoration")
            return 1
        if client.process.pid != process_id:
            print("FAIL workflow: decoder process was replaced")
            return 1
        if not (
            thumbnail_metrics.decoded_frames
            <= preview_metrics.decoded_frames
            <= resized_metrics.decoded_frames
            <= full_metrics.decoded_frames
        ):
            print("FAIL workflow: decoder counters did not remain monotonic")
            return 1
        print(
            f"PASS workflow: one PID {process_id}, thumbnail -> preview -> resize -> "
            f"fullscreen -> thumbnail, resize {resize_ms:.2f} ms, "
            f"decoded {thumbnail_metrics.decoded_frames} -> {full_metrics.decoded_frames}"
        )
        return 0
    finally:
        client.close()


def run_seek_demo(args: argparse.Namespace) -> int:
    executable = ensure_bridge()
    path = VIDEO_ENTRIES[2].video
    capabilities = detect_capabilities(args.color, args.glyphs)
    layout = Layout(120, 40, 5, 20, 60, 20, False)
    client = BridgeClient(executable, path)
    try:
        metadata = client.metadata()
        if metadata.duration_ns is None:
            print("FAIL seek demo: video duration is unavailable")
            return 1
        duration_seconds = metadata.duration_ns / 1_000_000_000
        requested = min(83.0, max(0.0, duration_seconds - 0.001))
        marker = min(39, round(requested / duration_seconds * 39))
        timeline = "-" * marker + "o" + "-" * (39 - marker)
        print(f"timeline [{timeline}] {requested:06.3f}s / {duration_seconds:06.3f}s")

        seek_metrics = client.seek(requested)
        payload, frame_metrics = client.frame_at(
            requested, layout, "balanced", capabilities
        )
        if not payload or not payload.endswith(b"\x1b[0m"):
            print("FAIL seek demo: exact-timestamp frame did not restore terminal state")
            return 1

        rapid_started = time.monotonic_ns()
        latest_token = 0
        latest_payload = b""
        for latest_token, fraction in enumerate((0.1, 0.3, 0.6, 0.9), 1):
            target = duration_seconds * fraction
            client.seek(target)
            candidate, _ = client.frame_at(target, layout, "balanced", capabilities)
            if latest_token == 4:
                latest_payload = candidate
        cancellation_us = (time.monotonic_ns() - rapid_started) // 1_000
        if not latest_payload:
            print("FAIL seek demo: latest rapid-seek generation produced no frame")
            return 1

        write_started = time.monotonic_ns()
        sink = io.BytesIO()
        sink.write(payload)
        write_us = (time.monotonic_ns() - write_started) // 1_000
        actual_seconds = frame_metrics.presentation_ns / 1_000_000_000
        print(
            f"PASS exact seek: requested {requested:.6f}s, decoder frame "
            f"{actual_seconds:.6f}s, accuracy {seek_metrics.accuracy}"
        )
        print(
            f"PASS render_at: row {layout.preview_row}, column {layout.preview_column}, "
            f"viewport {layout.preview_width}x{layout.preview_height}, {len(payload)} ANSI bytes"
        )
        print(
            f"PASS timings: seek {seek_metrics.seek_us / 1000:.2f} ms, "
            f"acquire {frame_metrics.acquisition_us / 1000:.2f} ms, "
            f"render {frame_metrics.render_us / 1000:.2f} ms, "
            f"ANSI {frame_metrics.ansi_us / 1000:.2f} ms, "
            f"mock write {write_us / 1000:.2f} ms"
        )
        print(
            f"PASS resources: retained {frame_metrics.retained_bytes} B, "
            f"buffered {frame_metrics.buffered_frames}, decoded {frame_metrics.decoded_frames}, "
            f"dropped {frame_metrics.dropped_frames}, seek-discarded "
            f"{frame_metrics.seek_discarded_frames}"
        )
        print(
            f"PASS rapid seek: latest token {latest_token} only, 4 requests completed in "
            f"{cancellation_us / 1000:.2f} ms"
        )
        return 0
    finally:
        client.close()


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--probe", action="store_true", help="render fixtures without an interactive terminal")
    parser.add_argument("--stress", action="store_true", help="rapidly replace preview requests")
    parser.add_argument("--benchmark", action="store_true", help="measure live preview and resize behavior")
    parser.add_argument("--compatibility", action="store_true", help="check deterministic terminal modes")
    parser.add_argument("--workflow", action="store_true", help="test thumbnail, preview, and fullscreen reuse")
    parser.add_argument("--seek-demo", action="store_true", help="seek by master-clock timestamp and render into an application rectangle")
    parser.add_argument("--size", default="80x24", help="probe terminal size, for example 40x12")
    parser.add_argument("--color", choices=("auto", "truecolor", "256", "16", "mono"), default="auto")
    parser.add_argument("--glyphs", choices=("auto", "unicode", "ascii"), default="auto")
    return parser.parse_args()


if __name__ == "__main__":
    arguments = parse_args()
    if arguments.probe:
        raise SystemExit(run_probe(arguments))
    if arguments.stress:
        raise SystemExit(run_stress(arguments))
    if arguments.benchmark:
        raise SystemExit(run_benchmark(arguments))
    if arguments.compatibility:
        raise SystemExit(run_compatibility())
    if arguments.workflow:
        raise SystemExit(run_workflow())
    if arguments.seek_demo:
        raise SystemExit(run_seek_demo(arguments))
    raise SystemExit(run_interactive(arguments))
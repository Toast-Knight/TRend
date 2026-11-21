# `trend-term`: design

Status: **partly implemented** in `adapters/term` (2026-11): session enter and restore (also on
panic), size and resize, keys with held-key state, mouse, the measured cell shape where the
terminal's size report includes pixels, and timed frame writes, on the crossterm backend chosen
in section 4. The test viewer runs on it (its own Win32 console code and its direct crossterm
use are gone), and the `showcase` example uses the mouse. Not yet: probing by query (DECRQM 2026,
`CSI 16 t`) and a frame-drop policy. Nothing in the core changed.

Section 2 describes the repository as it was before the crate existed.

## 1. The boundary

TRend's core turns a framebuffer into cells, and cells into bytes. It
never touches a file descriptor, a console handle, or a signal. The host
decides where the bytes go and when.

Everything that requires owning the terminal — putting it into a mode,
asking it questions, and reading its input — goes into `trend-term`.
That crate is optional: nothing in the core depends on it, and a host
with its own terminal layer (a TUI framework, an editor, a game
engine's console) keeps using the core alone.

| Concern | Where | Why |
| --- | --- | --- |
| Capability policy from environment values (`TerminalCapabilities::from_env_values`) | core | Pure function; testable without a terminal |
| `TerminalCapabilities::detect`, `TerminalEnvironment::detect` | core | `std` only (environment variables, `IsTerminal`); already stable API |
| ANSI output: cursor positioning, SGR, synchronized updates (mode 2026 brackets), diffing | core | This is the renderer |
| `OutputOptions`, `PresentStats`, `Surface::set_origin`, `cell_at_terminal` | core | Pure mappings over the renderer's own state |
| Raw mode, alternate screen, cursor visibility, and restoring them on exit or panic | `trend-term` | Owns the terminal; platform APIs |
| Enabling VT processing on Windows consoles | `trend-term` | Win32 console API |
| Terminal size and resize notification | `trend-term` | `ioctl`/`SIGWINCH` on Unix, console events on Windows |
| Keyboard, mouse (SGR 1006, pixel 1016), focus, and paste input | `trend-term` | Needs raw mode and an input parser |
| Probing: DECRQM for mode 2026, cell pixel size (`CSI 16 t`), window pixel size (`CSI 14 t`) | `trend-term` | Write a query, read a reply with a timeout |
| Writing frames with back-pressure measurement | `trend-term` (helper) | Owns the output handle; the core only produces bytes |

## 2. Evidence

- `tools/test-viewer/src/main.rs` hand-rolls a terminal session twice.
  On Windows it declares `GetStdHandle`, `GetConsoleMode`,
  `SetConsoleMode` (to set `ENABLE_VIRTUAL_TERMINAL_PROCESSING`),
  `GetConsoleScreenBufferInfo`, `_kbhit`, and `_getch` itself, writes
  `CSI ?1049h` and `CSI ?25l` by hand, and restores the console mode in a
  `Drop` impl. On Unix it uses crossterm for raw mode, the alternate
  screen, size, and input. About 140 lines of the viewer's 974 are this
  session layer.
- `examples/hud_3d.rs`, `procedural_source.rs`, and `surface_demo.rs` take
  `--size COLSxROWS` and otherwise hard-code a size. The
  `external-renderer` host falls back to the `COLUMNS`/`LINES` variables,
  which most shells do not export to child processes, and then to a fixed
  100×30. None of them follows a resize.
- `Surface::cell_at_terminal` (M4) maps terminal coordinates to cells,
  but no code in the repository produces mouse coordinates to feed it.
- `CellAspect` defaults to 1:2 because nothing measures the real cell
  pixel size; the 3D examples' `pixel_aspect` is therefore a guess on
  most terminals.
- Output rate: full-motion truecolor video at 200×60 produces about
  13 MB/s at 30 fps, and 31 MB/s at 320×90 (see
  [benchmarks.md](benchmarks.md), M6). Whether a terminal absorbs that is
  a property of the terminal, not of TRend, and nothing measures it today.
  The `external-renderer` host times `write_all` + `flush` per frame as a
  first step.

Every host needs the same few hundred lines, and each copy gets the
restore-on-panic path slightly differently wrong. That is the case for a
shared crate. None of it is the case for putting this in the core.

## 3. Proposed API sketch

```rust,ignore
use trend_term::{Session, SessionOptions, Event};

let mut session = Session::enter(SessionOptions {
    alternate_screen: true,
    hide_cursor: true,
    mouse: false,
    probe: true, // DECRQM 2026 and CSI 16 t, each with a short timeout
})?;
let info = session.info(); // columns, rows, cell pixel size if known,
                           // TerminalCapabilities, synchronized-update support
let mut surface = Surface::new(encoder, info.viewport(), info.capabilities)?;
surface.set_cell_aspect(info.cell_aspect());
surface.set_output_options(info.recommended_output_options());

loop {
    while let Some(event) = session.poll(Duration::ZERO)? {
        match event {
            Event::Resize { columns, rows } => surface.resize(Viewport::new(columns, rows)?)?,
            Event::Mouse { row, column, .. } => { let cell = surface.cell_at_terminal(row, column); }
            Event::Key(key) if key.is_quit() => return Ok(()),
            _ => {}
        }
    }
    // ... render, encode ...
    let timing = session.write_frame(surface.present_bytes())?; // write + flush, timed
}
// Dropping the session (or unwinding through it) restores the terminal.
```

The two crates meet only through core types (`Viewport`,
`TerminalCapabilities`, `CellAspect`, `OutputOptions`, byte slices), so
`trend-term` depends on `trend`, never the reverse.

## 4. Backend choice

Two options:

1. **crossterm backend.** Raw mode, alternate screen, size, resize, and
   an input parser on Unix and Windows, already used by the test viewer.
   Costs a dependency tree in `trend-term` only. Its event reader may
   consume or reject the replies to DECRQM and `CSI 16 t`; probing must be
   prototyped before relying on it (read replies before starting the
   event loop, or parse them from crossterm's raw input).
2. **Hand-rolled.** `termios` and `ioctl` via `libc` on Unix, the console
   API on Windows, plus an input parser. Fewer dependencies, more code to
   own, and the Windows console has several legacy modes to get right.

Recommendation: start with crossterm behind a default feature, keep the
public API free of crossterm types, and replace the backend only if a
host needs a smaller dependency tree. The core stays dependency-free
either way.

## 5. Staged plan

1. Extract the test viewer's session layer into `trend-term` with the API
   above (enter/restore, size, resize, keys), and port the test viewer
   and one example onto it. Acceptance: the viewer behaves the same on
   Windows Terminal, conhost, and a Unix terminal, and restores the
   terminal after a panic.
2. Add probing (DECRQM 2026, `CSI 16 t`) with timeouts and a
   capability-override environment variable. Acceptance: the 3D
   examples use the measured pixel aspect where the terminal reports it.
3. Add mouse (SGR 1006; 1016 where supported) and connect it to
   `cell_at_terminal` in one example.
4. Add `write_frame` timing and a documented frame-drop policy (skip
   presentation when the previous write has not finished within the
   frame budget), then measure real terminals at 200×60 and 320×90.

`trend-term` is not a TUI framework: no widgets, layout, focus
management, or event routing beyond turning terminal input into events.

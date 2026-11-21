# trend-term

An optional terminal session for TRend programs: the part that owns the terminal. TRend's core
turns pixels into cells and cells into bytes and never touches the terminal itself; this crate
does the rest, so a program doesn't have to write it again.

- **Enter and restore.** Raw mode, the alternate screen, a hidden cursor, mouse reporting and a
  window title, all put back (and colours reset) when the `Session` is dropped, and also if the
  program panics, so the panic's message is readable.
- **Size.** The terminal's size in cells, a `Resize` event when it changes (also on terminals
  that never announce a resize: the size is checked a few times a second), and the real shape of
  a cell where the terminal reports its pixel size (so 3D keeps its proportions).
- **Input.** Keys and mouse as plain events, plus `Session::is_down(key)` for held keys: exact
  where the terminal reports key releases (the Windows console; kitty, WezTerm, foot, Ghostty and
  others with the kitty keyboard protocol), estimated from auto-repeat elsewhere.
- **Output.** `Session::write_frame(bytes)` writes and flushes a frame and reports how long the
  terminal took to accept it.

It is not a TUI framework: no widgets, layout or focus. The backend is
[`crossterm`](https://crates.io/crates/crossterm); none of its types appear in this crate's API,
so the backend can change without breaking programs. Nothing in TRend's core depends on this crate.

## Start here

```text
cargo run --release -p trend-term --example starter
```

[`examples/starter.rs`](examples/starter.rs) is a complete program in about 50 lines: an animated
picture that follows the terminal's size, a square moved with the arrow keys, and a clean exit.
Change its `draw` closure to draw anything.

```rust
let mut session = Session::enter(SessionOptions::new())?;
let info = session.info();
let mut surface = Surface::new(HalfBlock, info.viewport, info.capabilities)?;
surface.set_output_options(info.recommended_output_options());
loop {
    while let Some(event) = session.poll(Duration::ZERO)? {
        match event {
            Event::Resize(viewport) => surface.resize(viewport)?,
            Event::Key { key: Key::Esc, .. } => return Ok(()),
            _ => {}
        }
    }
    surface.draw(|pixels, spec| { /* fill the pixels */ })?;
    session.write_frame(surface.present_bytes())?;
}
```

```text
cargo run --release -p trend-term --example showcase
```

[`examples/showcase.rs`](examples/showcase.rs) puts the optional crates together: a golden ring
drawn by `trend-3d` under a sky it reflects, a title drawn by `trend-text`, arrow keys or a mouse
drag to turn it, the wheel to zoom, `E` for the next environment and `Tab` for the next cell
encoder.

TRend's own media viewer, [`tools/test-viewer`](../../tools/test-viewer), runs on this crate too:
images, animations and video with a status line, on Windows and Unix from one piece of code.

## Not yet

- Asking the terminal questions (whether it supports synchronized updates, its cell size where
  the size report has no pixels). Synchronized updates are requested anyway; terminals that don't
  know them ignore the request.
- A frame-drop policy. `write_frame` gives the timing to build one from.
- Bracketed paste events.

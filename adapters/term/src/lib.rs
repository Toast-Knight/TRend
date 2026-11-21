//! A terminal session for TRend programs: everything that needs to own the terminal.
//!
//! TRend's core turns pixels into cells and cells into bytes; it never touches the terminal
//! itself. This optional crate does the rest, the same few hundred lines every program otherwise
//! writes for itself:
//!
//! - **Enter and restore:** raw mode, the alternate screen, a hidden cursor, mouse reporting and a
//!   window title, all put back when the [`Session`] is dropped, and also if the program panics.
//! - **Size:** the terminal's size in cells, [`Event::Resize`] when it changes (also on terminals
//!   that never announce a resize), and the shape of a cell where the terminal reports its pixel
//!   size.
//! - **Input:** keys and mouse as plain events, and [`Session::is_down`] for held keys (exact where
//!   the terminal reports key releases, estimated from auto-repeat elsewhere), which games need.
//! - **Output:** [`Session::write_frame`] writes and flushes a frame's bytes and says how long the
//!   terminal took, so a program can skip frames when the terminal is behind.
//!
//! ```no_run
//! use std::time::Duration;
//! use trend::{HalfBlock, Surface};
//! use trend_term::{Event, Key, Session, SessionOptions};
//!
//! let mut session = Session::enter(SessionOptions::default())?;
//! let info = session.info();
//! let mut surface = Surface::new(HalfBlock, info.viewport, info.capabilities)?;
//! surface.set_output_options(info.recommended_output_options());
//! 'frames: loop {
//!     while let Some(event) = session.poll(Duration::ZERO)? {
//!         match event {
//!             Event::Resize(viewport) => surface.resize(viewport)?,
//!             Event::Key { key: Key::Esc, .. } => break 'frames,
//!             _ => {}
//!         }
//!     }
//!     surface.draw(|mut pixels, _spec| {
//!         for y in 0..pixels.height() {
//!             pixels.row_mut(y).fill(128);
//!         }
//!     })?;
//!     session.write_frame(surface.present_bytes())?;
//!     std::thread::sleep(Duration::from_millis(16));
//! }
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! It is not a TUI framework: no widgets, layout or focus, only the terminal itself. The backend
//! is `crossterm`; none of its types appear in this API.

#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::io::{self, Write};
use std::sync::Once;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crossterm::event::{self as backend, KeyEventKind, KeyModifiers};
use crossterm::{cursor, execute, style, terminal};
use trend::{CellAspect, OutputOptions, TerminalCapabilities, Viewport};

/// What [`Session::enter`] sets up. Everything set up is undone when the session ends.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionOptions {
    /// Draw on the alternate screen, so the user's scrollback comes back afterwards.
    pub alternate_screen: bool,
    pub hide_cursor: bool,
    /// Report mouse buttons, movement while a button is held, and the wheel.
    pub mouse: bool,
    /// Ask for key releases and repeats where the terminal can report them (the kitty keyboard
    /// protocol; the Windows console always reports them). Makes [`Session::is_down`] exact.
    ///
    /// Finding out means asking the terminal and waiting for its answer. Terminals answer at
    /// once; one that answers nothing at all delays [`Session::enter`] by up to two seconds, so
    /// turn this off in programs that only read key presses.
    pub key_releases: bool,
    /// The window title, which streaming and capture tools list.
    pub title: Option<String>,
}

impl Default for SessionOptions {
    /// The alternate screen with a hidden cursor and key releases; no mouse, no title.
    fn default() -> Self {
        Self {
            alternate_screen: true,
            hide_cursor: true,
            mouse: false,
            key_releases: true,
            title: None,
        }
    }
}

/// A key, without modifiers. Letters arrive lower-case with [`Modifiers::shift`] set.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Key {
    Char(char),
    Enter,
    Esc,
    Tab,
    Backspace,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
    Insert,
    Delete,
    /// F1 to F24.
    Function(u8),
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Modifiers {
    pub shift: bool,
    pub control: bool,
    pub alt: bool,
}

/// Whether a key event is the first press, an auto-repeat, or the release.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum KeyState {
    Press,
    Repeat,
    Release,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MouseAction {
    Down(MouseButton),
    Up(MouseButton),
    /// Moved with the button held.
    Drag(MouseButton),
    Moved,
    ScrollUp,
    ScrollDown,
}

/// Something the terminal reported.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    Key {
        key: Key,
        modifiers: Modifiers,
        state: KeyState,
    },
    /// `column` and `row` are terminal cells from the top left, starting at 0: what
    /// `Surface::cell_at_terminal` takes (after adding 1 for its 1-based coordinates).
    Mouse {
        action: MouseAction,
        column: u16,
        row: u16,
        modifiers: Modifiers,
    },
    /// The terminal's new size in cells.
    Resize(Viewport),
    FocusGained,
    FocusLost,
}

/// What is known about the terminal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TerminalInfo {
    /// Size in cells.
    pub viewport: Viewport,
    pub capabilities: TerminalCapabilities,
    /// The size of one cell in pixels, where the terminal reports it.
    pub cell_pixels: Option<(u16, u16)>,
    /// Whether key releases are reported, so held keys are exact.
    pub key_releases: bool,
}

impl TerminalInfo {
    /// The shape of a cell: measured where the terminal reports pixel sizes, otherwise TRend's
    /// default of twice as tall as wide.
    pub fn cell_aspect(&self) -> CellAspect {
        match self.cell_pixels {
            Some((width, height)) if width > 0 && height > 0 => CellAspect { width, height },
            _ => CellAspect::default(),
        }
    }

    /// Output options that suit an interactive session: one colour sequence per cell and whole
    /// frames shown at once (terminals that don't know synchronized updates ignore the request).
    pub fn recommended_output_options(&self) -> OutputOptions {
        OutputOptions::new()
            .with_combined_colors(true)
            .with_synchronized_updates(true)
    }
}

/// How a frame's write went.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameTiming {
    pub bytes: usize,
    /// Time to write and flush: how long the terminal took to accept the frame.
    pub write: Duration,
}

/// What to undo, shared with the panic hook.
static RAW: AtomicBool = AtomicBool::new(false);
static ALTERNATE: AtomicBool = AtomicBool::new(false);
static CURSOR_HIDDEN: AtomicBool = AtomicBool::new(false);
static MOUSE: AtomicBool = AtomicBool::new(false);
static KEYBOARD: AtomicBool = AtomicBool::new(false);
static HOOK: Once = Once::new();

/// Puts back whatever a session changed. Safe to call more than once.
fn restore() {
    let mut out = io::stdout();
    if KEYBOARD.swap(false, Ordering::SeqCst) {
        let _ = execute!(out, backend::PopKeyboardEnhancementFlags);
    }
    if MOUSE.swap(false, Ordering::SeqCst) {
        let _ = execute!(out, backend::DisableMouseCapture);
    }
    if CURSOR_HIDDEN.swap(false, Ordering::SeqCst) {
        let _ = execute!(out, cursor::Show);
    }
    let entered = RAW.load(Ordering::SeqCst);
    if entered {
        // Whatever colours the last frame left set must not leak into the shell's prompt.
        let _ = execute!(out, style::SetAttribute(style::Attribute::Reset));
    }
    if ALTERNATE.swap(false, Ordering::SeqCst) {
        let _ = execute!(out, terminal::LeaveAlternateScreen);
    }
    if RAW.swap(false, Ordering::SeqCst) {
        let _ = terminal::disable_raw_mode();
    }
    let _ = out.flush();
}

/// How long a key counts as held after its last event when the terminal reports no releases: the
/// first auto-repeat comes after about half a second, later ones much faster.
fn held_window(presses: u32) -> Duration {
    Duration::from_millis(if presses > 1 { 140 } else { 560 })
}

/// Keys held down, from press, repeat and release events.
#[derive(Debug, Default)]
struct HeldKeys {
    keys: HashMap<Key, (Instant, u32)>,
    shift: Option<Instant>,
    releases: bool,
}

impl HeldKeys {
    fn record(&mut self, key: Key, modifiers: Modifiers, state: KeyState, now: Instant) {
        match state {
            KeyState::Release => {
                self.releases = true;
                self.keys.remove(&key);
            }
            KeyState::Press | KeyState::Repeat => {
                let entry = self.keys.entry(key).or_insert((now, 0));
                entry.0 = now;
                entry.1 += 1;
            }
        }
        if modifiers.shift && state != KeyState::Release {
            self.shift = Some(now);
        } else if !modifiers.shift {
            self.shift = None;
        }
    }

    fn is_down(&self, key: Key, now: Instant) -> bool {
        self.keys.get(&key).is_some_and(|&(last, presses)| {
            self.releases || now.duration_since(last) < held_window(presses)
        })
    }

    fn shift(&self, now: Instant) -> bool {
        self.shift
            .is_some_and(|last| now.duration_since(last) < held_window(2))
    }
}

/// How often [`Session::poll`] compares the terminal's size with the last one it reported.
const SIZE_CHECK: Duration = Duration::from_millis(250);

/// The terminal, taken over for a program's lifetime. See the crate documentation.
#[derive(Debug)]
pub struct Session {
    info: TerminalInfo,
    held: HeldKeys,
    size_checked: Instant,
}

/// The terminal's size in cells, asked of the terminal itself.
fn viewport_now() -> io::Result<Viewport> {
    let (columns, rows) = terminal::size()?;
    Ok(Viewport {
        width: columns.max(1),
        height: rows.max(1),
    })
}

fn modifiers(from: KeyModifiers) -> Modifiers {
    Modifiers {
        shift: from.contains(KeyModifiers::SHIFT),
        control: from.contains(KeyModifiers::CONTROL),
        alt: from.contains(KeyModifiers::ALT),
    }
}

fn key(code: backend::KeyCode) -> Option<Key> {
    use backend::KeyCode;
    Some(match code {
        KeyCode::Char(character) => Key::Char(character.to_ascii_lowercase()),
        KeyCode::Enter => Key::Enter,
        KeyCode::Esc => Key::Esc,
        KeyCode::Tab | KeyCode::BackTab => Key::Tab,
        KeyCode::Backspace => Key::Backspace,
        KeyCode::Left => Key::Left,
        KeyCode::Right => Key::Right,
        KeyCode::Up => Key::Up,
        KeyCode::Down => Key::Down,
        KeyCode::Home => Key::Home,
        KeyCode::End => Key::End,
        KeyCode::PageUp => Key::PageUp,
        KeyCode::PageDown => Key::PageDown,
        KeyCode::Insert => Key::Insert,
        KeyCode::Delete => Key::Delete,
        KeyCode::F(number) => Key::Function(number),
        _ => return None,
    })
}

fn button(from: backend::MouseButton) -> MouseButton {
    match from {
        backend::MouseButton::Left => MouseButton::Left,
        backend::MouseButton::Right => MouseButton::Right,
        backend::MouseButton::Middle => MouseButton::Middle,
    }
}

/// Turns a backend event into ours; `None` for events this crate does not report.
fn translate(event: backend::Event) -> Option<Event> {
    Some(match event {
        backend::Event::Key(event) => {
            let mut mods = modifiers(event.modifiers);
            if event.code == backend::KeyCode::BackTab
                || matches!(event.code, backend::KeyCode::Char(c) if c.is_ascii_uppercase())
            {
                mods.shift = true;
            }
            Event::Key {
                key: key(event.code)?,
                modifiers: mods,
                state: match event.kind {
                    KeyEventKind::Press => KeyState::Press,
                    KeyEventKind::Repeat => KeyState::Repeat,
                    KeyEventKind::Release => KeyState::Release,
                },
            }
        }
        backend::Event::Mouse(event) => {
            use backend::MouseEventKind;
            let action = match event.kind {
                MouseEventKind::Down(from) => MouseAction::Down(button(from)),
                MouseEventKind::Up(from) => MouseAction::Up(button(from)),
                MouseEventKind::Drag(from) => MouseAction::Drag(button(from)),
                MouseEventKind::Moved => MouseAction::Moved,
                MouseEventKind::ScrollUp => MouseAction::ScrollUp,
                MouseEventKind::ScrollDown => MouseAction::ScrollDown,
                _ => return None,
            };
            Event::Mouse {
                action,
                column: event.column,
                row: event.row,
                modifiers: modifiers(event.modifiers),
            }
        }
        backend::Event::Resize(columns, rows) => Event::Resize(Viewport {
            width: columns.max(1),
            height: rows.max(1),
        }),
        backend::Event::FocusGained => Event::FocusGained,
        backend::Event::FocusLost => Event::FocusLost,
        _ => return None,
    })
}

/// The cell size in pixels from the terminal's size report, if it gives one.
fn cell_pixels() -> Option<(u16, u16)> {
    let size = terminal::window_size().ok()?;
    if size.columns == 0 || size.rows == 0 || size.width == 0 || size.height == 0 {
        return None;
    }
    let cell = (size.width / size.columns, size.height / size.rows);
    (cell.0 > 0 && cell.1 > 0).then_some(cell)
}

impl Session {
    /// Takes over the terminal as `options` ask. Fails if standard output is not a terminal.
    pub fn enter(options: SessionOptions) -> io::Result<Self> {
        // A panic's message is only readable on the normal screen with the terminal restored.
        HOOK.call_once(|| {
            let previous = std::panic::take_hook();
            std::panic::set_hook(Box::new(move |info| {
                restore();
                previous(info);
            }));
        });
        let viewport = viewport_now()?;
        // TRend's output is escape sequences; a console that cannot be switched to them (before
        // Windows 10) would show them as text.
        #[cfg(windows)]
        if !crossterm::ansi_support::supports_ansi() {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "this console does not support ANSI escape sequences",
            ));
        }
        terminal::enable_raw_mode()?;
        RAW.store(true, Ordering::SeqCst);
        let mut out = io::stdout();
        let entered = (|| -> io::Result<bool> {
            if let Some(title) = &options.title {
                execute!(out, terminal::SetTitle(title))?;
            }
            if options.alternate_screen {
                execute!(out, terminal::EnterAlternateScreen)?;
                ALTERNATE.store(true, Ordering::SeqCst);
            }
            if options.hide_cursor {
                execute!(out, cursor::Hide)?;
                CURSOR_HIDDEN.store(true, Ordering::SeqCst);
            }
            if options.mouse {
                execute!(out, backend::EnableMouseCapture)?;
                MOUSE.store(true, Ordering::SeqCst);
            }
            let mut releases = cfg!(windows);
            if options.key_releases && terminal::supports_keyboard_enhancement().unwrap_or(false) {
                execute!(
                    out,
                    backend::PushKeyboardEnhancementFlags(
                        backend::KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
                            | backend::KeyboardEnhancementFlags::REPORT_EVENT_TYPES
                    )
                )?;
                KEYBOARD.store(true, Ordering::SeqCst);
                releases = true;
            }
            Ok(releases)
        })();
        let key_releases = match entered {
            Ok(releases) => releases,
            Err(error) => {
                restore();
                return Err(error);
            }
        };
        Ok(Self {
            info: TerminalInfo {
                viewport,
                capabilities: TerminalCapabilities::detect(),
                cell_pixels: cell_pixels(),
                key_releases,
            },
            held: HeldKeys {
                releases: key_releases,
                ..HeldKeys::default()
            },
            size_checked: Instant::now(),
        })
    }

    /// What is known about the terminal; the size follows [`Event::Resize`].
    pub const fn info(&self) -> TerminalInfo {
        self.info
    }

    /// Compares the terminal's size with the last one reported; `Some` when it changed.
    fn size_changed(&mut self) -> Option<Event> {
        self.size_checked = Instant::now();
        let viewport = viewport_now().ok()?;
        if viewport == self.info.viewport {
            return None;
        }
        self.info.viewport = viewport;
        self.info.cell_pixels = cell_pixels();
        Some(Event::Resize(viewport))
    }

    /// The next event, waiting up to `timeout` (zero only checks). `None` when nothing arrived.
    ///
    /// [`Event::Resize`] is reported once per change of size, with the size the terminal gives
    /// when asked. The size is also checked a few times a second, so a resize is seen on terminals
    /// that do not announce it.
    pub fn poll(&mut self, timeout: Duration) -> io::Result<Option<Event>> {
        let deadline = Instant::now() + timeout;
        loop {
            if self.size_checked.elapsed() >= SIZE_CHECK
                && let Some(resize) = self.size_changed()
            {
                return Ok(Some(resize));
            }
            let wait = deadline.saturating_duration_since(Instant::now());
            if !backend::poll(wait.min(SIZE_CHECK))? {
                if wait <= SIZE_CHECK {
                    return Ok(None);
                }
                continue;
            }
            let Some(event) = translate(backend::read()?) else {
                continue;
            };
            match event {
                Event::Key {
                    key,
                    modifiers,
                    state,
                } => self.held.record(key, modifiers, state, Instant::now()),
                // The backend's own numbers are not used: on the Windows console they describe
                // the scroll-back buffer, not the window.
                Event::Resize(_) => match self.size_changed() {
                    Some(resize) => return Ok(Some(resize)),
                    None => continue,
                },
                // Keys released while the window was unfocused never report it.
                Event::FocusLost => self.held.keys.clear(),
                _ => {}
            }
            return Ok(Some(event));
        }
    }

    /// Whether `key` is held down now. Exact where the terminal reports key releases
    /// ([`TerminalInfo::key_releases`]); elsewhere a key counts as held while it auto-repeats.
    pub fn is_down(&self, key: Key) -> bool {
        self.held.is_down(key, Instant::now())
    }

    /// Whether Shift was held with the latest key events.
    pub fn shift_down(&self) -> bool {
        self.held.shift(Instant::now())
    }

    /// Writes a frame's bytes (from `Surface::present_bytes`) and flushes, timing how long the
    /// terminal takes to accept them.
    pub fn write_frame(&mut self, bytes: &[u8]) -> io::Result<FrameTiming> {
        let started = Instant::now();
        let mut out = io::stdout().lock();
        out.write_all(bytes)?;
        out.flush()?;
        Ok(FrameTiming {
            bytes: bytes.len(),
            write: started.elapsed(),
        })
    }

    /// Clears the screen, as after a resize when the old picture no longer lines up.
    pub fn clear(&mut self) -> io::Result<()> {
        execute!(io::stdout(), terminal::Clear(terminal::ClearType::All))
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        restore();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use backend::{KeyCode, KeyEvent, MouseEvent, MouseEventKind};

    #[test]
    fn backend_events_become_plain_events() {
        let pressed = translate(backend::Event::Key(KeyEvent::new(
            KeyCode::Char('W'),
            KeyModifiers::CONTROL,
        )));
        assert_eq!(
            pressed,
            Some(Event::Key {
                key: Key::Char('w'),
                modifiers: Modifiers {
                    shift: true,
                    control: true,
                    alt: false
                },
                state: KeyState::Press,
            })
        );
        let wheel = translate(backend::Event::Mouse(MouseEvent {
            kind: MouseEventKind::ScrollDown,
            column: 7,
            row: 3,
            modifiers: KeyModifiers::NONE,
        }));
        assert!(matches!(
            wheel,
            Some(Event::Mouse {
                action: MouseAction::ScrollDown,
                column: 7,
                row: 3,
                ..
            })
        ));
        assert_eq!(
            translate(backend::Event::Resize(0, 40)),
            Some(Event::Resize(Viewport {
                width: 1,
                height: 40
            }))
        );
        assert_eq!(translate(backend::Event::Paste("x".into())), None);
        assert_eq!(key(KeyCode::F(5)), Some(Key::Function(5)));
        assert_eq!(key(KeyCode::CapsLock), None);
    }

    #[test]
    fn held_keys_follow_releases_or_auto_repeat() {
        let start = Instant::now();
        let none = Modifiers::default();
        // A terminal without key releases: held while repeats keep coming.
        let mut held = HeldKeys::default();
        held.record(Key::Char('w'), none, KeyState::Press, start);
        assert!(held.is_down(Key::Char('w'), start + Duration::from_millis(500)));
        assert!(!held.is_down(Key::Char('w'), start + Duration::from_millis(600)));
        held.record(
            Key::Char('w'),
            none,
            KeyState::Repeat,
            start + Duration::from_millis(500),
        );
        assert!(held.is_down(Key::Char('w'), start + Duration::from_millis(600)));
        assert!(!held.is_down(Key::Char('w'), start + Duration::from_millis(700)));
        // With releases: held until released, however long.
        let mut held = HeldKeys {
            releases: true,
            ..HeldKeys::default()
        };
        let shift = Modifiers {
            shift: true,
            ..none
        };
        held.record(Key::Up, shift, KeyState::Press, start);
        assert!(held.is_down(Key::Up, start + Duration::from_secs(60)));
        assert!(held.shift(start + Duration::from_millis(100)));
        held.record(
            Key::Up,
            none,
            KeyState::Release,
            start + Duration::from_secs(61),
        );
        assert!(!held.is_down(Key::Up, start + Duration::from_secs(61)));
        assert!(!held.is_down(Key::Down, start));
    }

    #[test]
    fn info_gives_the_cell_shape_and_output_options() {
        let mut info = TerminalInfo {
            viewport: Viewport {
                width: 80,
                height: 24,
            },
            capabilities: TerminalCapabilities::detect(),
            cell_pixels: None,
            key_releases: false,
        };
        assert_eq!(info.cell_aspect(), CellAspect::default());
        info.cell_pixels = Some((9, 20));
        assert_eq!(
            info.cell_aspect(),
            CellAspect {
                width: 9,
                height: 20
            }
        );
        let _ = info.recommended_output_options();
        // Restoring with nothing entered does nothing and is safe to repeat.
        restore();
        restore();
    }
}

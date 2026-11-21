use std::io::{IsTerminal, stdin, stdout};
use std::time::{Duration, Instant};

use crate::cell::{Cell, CellFrame, CellView, Color};
use crate::color::{ASCII_RAMP, SHADE_RAMP, ansi16_index, ansi256_index, luminance, ramp_glyph};
use crate::framebuffer::PixelFrameView;
use crate::geometry::{CellRect, RegionError};
use crate::renderer::{RenderError, RenderOptions, RenderRequest, Renderer};

/// ANSI color encoding emitted by [`AnsiPresenter`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TerminalColorMode {
    TrueColor,
    Ansi256,
    Ansi16,
    Monochrome,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TerminalCapabilities {
    pub color: TerminalColorMode,
    pub unicode: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TerminalEnvironment {
    pub capabilities: TerminalCapabilities,
    pub stdin_is_terminal: bool,
    pub stdout_is_terminal: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PositionedRenderRequest {
    pub render: RenderRequest,
    pub origin_row: u16,
    pub origin_column: u16,
}

#[derive(Debug)]
pub struct PositionedRenderOutput<'a> {
    pub bytes: &'a [u8],
    pub render_time: Duration,
    pub ansi_time: Duration,
}

impl TerminalCapabilities {
    pub const CONSERVATIVE: Self = Self {
        color: TerminalColorMode::Monochrome,
        unicode: false,
    };

    pub fn from_env_values(term: Option<&str>, color_term: Option<&str>, no_color: bool) -> Self {
        let term = term.unwrap_or("");
        let color_term = color_term.unwrap_or("");
        let unicode = !term.is_empty() && !term.eq_ignore_ascii_case("dumb");
        let color = if no_color || term.eq_ignore_ascii_case("dumb") {
            TerminalColorMode::Monochrome
        } else if color_term.eq_ignore_ascii_case("truecolor")
            || color_term.eq_ignore_ascii_case("24bit")
            || contains_ignore_ascii_case(term, "truecolor")
            || contains_ignore_ascii_case(term, "24bit")
            || contains_ignore_ascii_case(term, "direct")
        {
            TerminalColorMode::TrueColor
        } else if contains_ignore_ascii_case(term, "256color") {
            TerminalColorMode::Ansi256
        } else if !term.is_empty() {
            TerminalColorMode::Ansi16
        } else {
            TerminalColorMode::Monochrome
        };
        Self { color, unicode }
    }

    pub fn detect() -> Self {
        let term = std::env::var("TERM").ok();
        let color_term = std::env::var("COLORTERM").ok();
        Self::from_env_values(
            term.as_deref(),
            color_term.as_deref(),
            std::env::var_os("NO_COLOR").is_some(),
        )
    }
}

impl TerminalEnvironment {
    pub fn detect() -> Self {
        Self {
            capabilities: TerminalCapabilities::detect(),
            stdin_is_terminal: stdin().is_terminal(),
            stdout_is_terminal: stdout().is_terminal(),
        }
    }
}

fn contains_ignore_ascii_case(haystack: &str, needle: &str) -> bool {
    haystack
        .as_bytes()
        .windows(needle.len())
        .any(|part| part.eq_ignore_ascii_case(needle.as_bytes()))
}

/// Opt-in presentation behavior. The default reproduces the presenter's
/// original output byte for byte.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[non_exhaustive]
pub struct OutputOptions {
    /// Wrap each non-empty frame in DEC synchronized-update mode 2026
    /// (`CSI ? 2026 h` … `CSI ? 2026 l`), so supporting terminals show the
    /// frame at once instead of drawing it progressively (no tearing).
    /// Terminals without the mode ignore it.
    pub synchronized_updates: bool,
    /// When a cell changes both colors, set them with one SGR sequence
    /// instead of two. Same result on screen, about 5% fewer bytes on
    /// full-change truecolor frames and 8% in 256 colors.
    pub combined_colors: bool,
    /// In 256- and 16-color modes, skip color sequences that select the
    /// palette entry the terminal already has. Many neighboring RGB colors
    /// map to one entry, so this removes most color bytes (1.7–5.4× smaller
    /// output across the M6 workloads) with the same result on screen, and
    /// presenting costs no more CPU than the default. Truecolor and
    /// monochrome output are unaffected. Off by default only because it
    /// changes the bytes written; it is planned to become the default at
    /// the next output-compatibility boundary.
    pub compact_indexed_colors: bool,
}

impl OutputOptions {
    pub const fn new() -> Self {
        Self {
            synchronized_updates: false,
            combined_colors: false,
            compact_indexed_colors: false,
        }
    }

    pub const fn with_synchronized_updates(mut self, enabled: bool) -> Self {
        self.synchronized_updates = enabled;
        self
    }

    pub const fn with_combined_colors(mut self, enabled: bool) -> Self {
        self.combined_colors = enabled;
        self
    }

    pub const fn with_compact_indexed_colors(mut self, enabled: bool) -> Self {
        self.compact_indexed_colors = enabled;
        self
    }
}

/// What the most recent presentation emitted.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[non_exhaustive]
pub struct PresentStats {
    /// Total bytes, including cursor moves, colors, glyphs, and wrappers.
    pub bytes: usize,
    /// Cells written.
    pub cells: usize,
    /// Runs of consecutive changed cells, each starting with a cursor move.
    pub runs: usize,
    /// SGR color sequences emitted.
    pub color_sequences: usize,
    /// SGR sequences emitted for cell attributes (bold, underline, …).
    pub attribute_sequences: usize,
}

const SYNC_BEGIN: &[u8] = b"\x1b[?2026h";
const SYNC_END: &[u8] = b"\x1b[?2026l";

/// Converts cell frames to ANSI bytes, using incremental updates after the first frame.
///
/// Reuse one presenter for a playback surface so unchanged cells can be skipped.
/// Call [`Self::invalidate`] (or create a fresh presenter) after external
/// terminal output invalidates its knowledge of the previously presented frame.
#[derive(Debug)]
pub struct AnsiPresenter {
    /// What the terminal shows, cell for cell. `Cell::UNKNOWN` marks cells
    /// whose on-screen content is not known and must be drawn.
    previous: CellFrame,
    previous_origin: Option<(usize, usize)>,
    bytes: Vec<u8>,
    capabilities: TerminalCapabilities,
    options: OutputOptions,
    stats: PresentStats,
    foreground: Option<Color>,
    background: Option<Color>,
    /// The last RGB colors looked up in the palette, with their keys, for
    /// the foreground and background: neighboring cells often repeat them.
    palette_memo: [Option<(Color, Color)>; 2],
    /// Attributes last written ([`Cell::BOLD`] and friends). The terminal is
    /// assumed to start with none, as the color logic always has.
    attributes: u8,
}

impl AnsiPresenter {
    pub fn new(mode: TerminalColorMode) -> Self {
        Self::with_capabilities(TerminalCapabilities {
            color: mode,
            unicode: true,
        })
    }

    pub fn with_capabilities(capabilities: TerminalCapabilities) -> Self {
        Self::with_options(capabilities, OutputOptions::default())
    }

    pub fn with_options(capabilities: TerminalCapabilities, options: OutputOptions) -> Self {
        Self {
            previous: CellFrame::default(),
            previous_origin: None,
            bytes: Vec::new(),
            capabilities,
            options,
            stats: PresentStats::default(),
            foreground: None,
            background: None,
            palette_memo: [None; 2],
            attributes: 0,
        }
    }

    pub const fn capabilities(&self) -> TerminalCapabilities {
        self.capabilities
    }

    pub const fn options(&self) -> OutputOptions {
        self.options
    }

    /// Changes output options from the next presentation on. What's on
    /// screen stays valid.
    pub fn set_options(&mut self, options: OutputOptions) {
        if options.compact_indexed_colors != self.options.compact_indexed_colors {
            // The pen's colors are tracked differently in the two modes.
            self.foreground = None;
            self.background = None;
        }
        self.options = options;
    }

    /// Statistics for the most recent presentation.
    pub const fn last_stats(&self) -> PresentStats {
        self.stats
    }

    /// Forgets what the terminal shows, so the next presentation redraws
    /// every cell it covers. Use it after other output overwrote the area.
    pub fn invalidate(&mut self) {
        self.previous.cells_mut().fill(Cell::UNKNOWN);
        self.foreground = None;
        self.background = None;
    }

    pub fn present<'a>(&'a mut self, current: &CellFrame) -> &'a [u8] {
        self.present_frame(current.view(), (1, 1), true, false);
        &self.bytes
    }

    /// Presents an incremental frame at a 1-based terminal origin without
    /// clearing cells outside the frame when its geometry changes. The output
    /// restores default terminal attributes before returning control to its host.
    pub fn present_at<'a>(
        &'a mut self,
        current: &CellFrame,
        origin_row: u16,
        origin_column: u16,
    ) -> &'a [u8] {
        self.present_view_at(current.view(), origin_row, origin_column)
    }

    /// [`Self::present_at`] for any cell view, such as a region of a larger
    /// frame given to a presenter of its own.
    pub fn present_view_at<'a>(
        &'a mut self,
        current: CellView<'_>,
        origin_row: u16,
        origin_column: u16,
    ) -> &'a [u8] {
        let origin = (
            usize::from(origin_row.max(1)),
            usize::from(origin_column.max(1)),
        );
        self.present_frame(current, origin, false, true);
        &self.bytes
    }

    /// Presents only `region` of `frame`, where `frame` is shown at a
    /// 1-based terminal origin. Cells outside the region are neither
    /// examined nor written, and the presenter keeps its knowledge of them,
    /// so different regions can be presented at different rates (a video
    /// pane every frame, a status bar once a second) with one presenter.
    ///
    /// If `frame`'s size or origin differs from the previous presentation,
    /// everything previously shown is forgotten first. Like
    /// [`Self::present_at`], nothing outside the frame is cleared and
    /// attributes are restored afterwards.
    pub fn present_region_at<'a>(
        &'a mut self,
        frame: CellView<'_>,
        region: CellRect,
        origin_row: u16,
        origin_column: u16,
    ) -> Result<&'a [u8], RegionError> {
        region.check_within(frame.width(), frame.height())?;
        let origin = (
            usize::from(origin_row.max(1)),
            usize::from(origin_column.max(1)),
        );
        self.begin();
        self.prepare(frame.width(), frame.height(), origin, false);
        self.emit(frame, region, origin);
        self.finish(true);
        Ok(&self.bytes)
    }

    pub fn render_at<'a>(
        &'a mut self,
        renderer: &Renderer,
        source: PixelFrameView<'_>,
        request: PositionedRenderRequest,
        options: RenderOptions,
        output: &mut CellFrame,
    ) -> Result<PositionedRenderOutput<'a>, RenderError> {
        let render_started = Instant::now();
        renderer.render_with_options_into(source, request.render, options, output)?;
        let render_time = render_started.elapsed();
        let ansi_started = Instant::now();
        let bytes = self.present_at(output, request.origin_row, request.origin_column);
        let ansi_time = ansi_started.elapsed();
        Ok(PositionedRenderOutput {
            bytes,
            render_time,
            ansi_time,
        })
    }

    pub fn output_capacity(&self) -> usize {
        self.bytes.capacity()
    }

    /// Presents a whole frame: the shared body of `present`, `present_at`,
    /// and `present_view_at`.
    fn present_frame(
        &mut self,
        current: CellView<'_>,
        origin: (usize, usize),
        clear_on_geometry_change: bool,
        positioned: bool,
    ) {
        self.begin();
        self.prepare(
            current.width(),
            current.height(),
            origin,
            clear_on_geometry_change,
        );
        let everything = CellRect::new(0, 0, current.width(), current.height());
        self.emit(current, everything, origin);
        self.finish(positioned);
    }

    fn begin(&mut self) {
        self.bytes.clear();
        self.stats = PresentStats::default();
        if self.options.synchronized_updates {
            self.bytes.extend_from_slice(SYNC_BEGIN);
        }
    }

    /// Makes the remembered frame match `width` × `height` at `origin`,
    /// forgetting it (and clearing the screen, if asked) when either changed.
    fn prepare(
        &mut self,
        width: u16,
        height: u16,
        origin: (usize, usize),
        clear_on_geometry_change: bool,
    ) {
        let geometry_changed = self.previous.width() != width || self.previous.height() != height;
        if geometry_changed {
            if clear_on_geometry_change {
                self.bytes.extend_from_slice(b"\x1b[2J");
            }
            self.previous.resize(width, height).ok();
        }
        if geometry_changed || self.previous_origin != Some(origin) {
            self.invalidate();
        }
        self.previous_origin = Some(origin);
    }

    /// Writes the changed cells of `region`, run by run, and remembers them.
    fn emit(&mut self, current: CellView<'_>, region: CellRect, origin: (usize, usize)) {
        let frame_width = usize::from(current.width());
        let first = usize::from(region.x);
        let width = usize::from(region.width);
        let capabilities = self.capabilities;
        let combined = self.options.combined_colors;
        let indexed = self.options.compact_indexed_colors
            && matches!(
                capabilities.color,
                TerminalColorMode::Ansi256 | TerminalColorMode::Ansi16
            );
        let rows = usize::from(region.y)..usize::from(region.y) + usize::from(region.height);
        let (mut cells, mut runs, mut color_sequences, mut attribute_sequences) = (0, 0, 0, 0);
        // Local copies keep the pen in registers: `self.bytes` is written
        // in between, and through `&mut self` fields the compiler must
        // assume every write may change them.
        let mut pen_attributes = self.attributes;
        let mut pen = ColorPen {
            foreground: self.foreground,
            background: self.background,
            palette_memo: self.palette_memo,
        };
        for row in rows {
            let current_row = &current.row(row as u16)[first..first + width];
            let start = row * frame_width + first;
            let previous_row = &self.previous.cells()[start..start + width];
            let mut column = 0;
            while column < width {
                // Skip to the next changed cell in this row. Forgotten cells
                // are `Cell::UNKNOWN`, which never equals a real cell.
                match current_row[column..]
                    .iter()
                    .zip(&previous_row[column..])
                    .position(|(now, before)| now != before)
                {
                    Some(offset) => column += offset,
                    None => break,
                }

                self.bytes.extend_from_slice(b"\x1b[");
                push_decimal(&mut self.bytes, origin.0 + row);
                self.bytes.push(b';');
                push_decimal(&mut self.bytes, origin.1 + first + column);
                self.bytes.push(b'H');
                runs += 1;
                let run_start = column;
                while column < width && current_row[column] != previous_row[column] {
                    let cell = current_row[column];
                    if cell.attributes != pen_attributes {
                        write_attributes(&mut self.bytes, pen_attributes, cell.attributes);
                        pen_attributes = cell.attributes;
                        attribute_sequences += 1;
                    }
                    color_sequences += write_cell(
                        &mut self.bytes,
                        capabilities,
                        combined,
                        indexed,
                        &mut pen,
                        cell,
                    );
                    column += 1;
                }
                cells += column - run_start;
            }
            self.previous.cells_mut()[start..start + width].copy_from_slice(current_row);
        }
        self.stats.cells += cells;
        self.stats.runs += runs;
        self.stats.color_sequences += color_sequences;
        self.stats.attribute_sequences += attribute_sequences;
        self.attributes = pen_attributes;
        (self.foreground, self.background) = (pen.foreground, pen.background);
        self.palette_memo = pen.palette_memo;
    }

    /// Ends a presentation. Positioned output restores default attributes
    /// after anything it wrote, so the host's own text is unaffected; a
    /// synchronized update is closed, or dropped entirely if empty.
    fn finish(&mut self, positioned: bool) {
        let opener = if self.options.synchronized_updates {
            SYNC_BEGIN.len()
        } else {
            0
        };
        let wrote = self.bytes.len() > opener;
        if positioned && wrote {
            self.bytes.extend_from_slice(b"\x1b[0m");
            self.foreground = None;
            self.background = None;
            self.attributes = 0;
        }
        if opener > 0 {
            if wrote {
                self.bytes.extend_from_slice(SYNC_END);
            } else {
                self.bytes.clear();
            }
        }
        self.stats.bytes = self.bytes.len();
    }
}

/// The colors the terminal was last told, while presenting. See
/// [`AnsiPresenter`]'s fields of the same names.
struct ColorPen {
    foreground: Option<Color>,
    background: Option<Color>,
    palette_memo: [Option<(Color, Color)>; 2],
}

fn write_cell(
    bytes: &mut Vec<u8>,
    capabilities: TerminalCapabilities,
    combined: bool,
    indexed: bool,
    pen: &mut ColorPen,
    cell: Cell,
) -> usize {
    let mut color_sequences = 0;
    if capabilities.color == TerminalColorMode::Monochrome {
        // Cells that carry color are shaded by brightness. Cells with no
        // color at all (such as `Density` output) already encode the
        // image in their glyph and are written as-is below; before this
        // rule they always rendered blank.
        if cell.foreground.kind != 0 || cell.background.kind != 0 {
            let glyph = monochrome_glyph(cell, capabilities.unicode);
            let mut encoded = [0; 4];
            bytes.extend_from_slice(glyph.encode_utf8(&mut encoded).as_bytes());
            return 0;
        }
    } else {
        // The pen remembers what the terminal was told. With `indexed`, that
        // is the palette entry, so colors mapping to the same entry match,
        // and sequences are written from the entry without searching again.
        let (foreground_key, background_key) = if indexed {
            (
                memo_key(
                    capabilities.color,
                    cell.foreground,
                    &mut pen.palette_memo[0],
                ),
                memo_key(
                    capabilities.color,
                    cell.background,
                    &mut pen.palette_memo[1],
                ),
            )
        } else {
            (cell.foreground, cell.background)
        };
        let foreground_changed = pen.foreground != Some(foreground_key);
        let background_changed = pen.background != Some(background_key);
        let mode = capabilities.color;
        if combined && foreground_changed && background_changed {
            if indexed {
                write_colors::<true>(bytes, mode, foreground_key, background_key);
            } else {
                write_colors::<false>(bytes, mode, foreground_key, background_key);
            }
            color_sequences += 1;
        } else {
            if foreground_changed {
                if indexed {
                    write_color::<true>(bytes, mode, foreground_key, true);
                } else {
                    write_color::<false>(bytes, mode, foreground_key, true);
                }
                color_sequences += 1;
            }
            if background_changed {
                if indexed {
                    write_color::<true>(bytes, mode, background_key, false);
                } else {
                    write_color::<false>(bytes, mode, background_key, false);
                }
                color_sequences += 1;
            }
        }
        pen.foreground = Some(foreground_key);
        pen.background = Some(background_key);
    }
    let mut encoded = [0; 4];
    let glyph = char::from_u32(cell.glyph).unwrap_or(' ');
    let glyph = if capabilities.unicode || glyph.is_ascii() {
        glyph
    } else if glyph == ' ' {
        ' '
    } else {
        '#'
    };
    bytes.extend_from_slice(glyph.encode_utf8(&mut encoded).as_bytes());
    color_sequences
}

/// The pen key for `color` when tracking palette entries: the default color
/// as itself, others as their palette index under a color kind (2) that no
/// real color has, so keys never match an RGB pen or another mode's index.
#[inline]
fn palette_key(mode: TerminalColorMode, color: Color) -> Color {
    if color.kind == 0 {
        return color;
    }
    let index = match mode {
        TerminalColorMode::Ansi256 => ansi256_index(color),
        _ => ansi16_index(color),
    };
    Color {
        red: index,
        green: 0,
        blue: 0,
        kind: 2,
    }
}

/// [`palette_key`], remembering the last 16-color lookup: an exact cache,
/// since the key depends only on the mode and the color, and presenters
/// don't change mode. The 16-color search compares against every entry and
/// neighboring cells often repeat colors; the 256-color index is a little
/// arithmetic, cheaper than consulting the cache (measured in M6).
#[inline]
fn memo_key(mode: TerminalColorMode, color: Color, memo: &mut Option<(Color, Color)>) -> Color {
    if mode != TerminalColorMode::Ansi16 {
        return palette_key(mode, color);
    }
    match *memo {
        Some((last, key)) if last == color => key,
        _ => {
            let key = palette_key(mode, color);
            *memo = Some((color, key));
            key
        }
    }
}

/// SGR parameters turning each attribute on and off, by bit.
const ATTRIBUTE_CODES: [(u8, &[u8], &[u8]); 7] = [
    (Cell::BOLD, b"1", b"22"),
    (Cell::DIM, b"2", b"22"),
    (Cell::ITALIC, b"3", b"23"),
    (Cell::UNDERLINE, b"4", b"24"),
    (Cell::BLINK, b"5", b"25"),
    (Cell::REVERSE, b"7", b"27"),
    (Cell::STRIKETHROUGH, b"9", b"29"),
];

/// One SGR sequence changing the terminal's attributes from `from` to `to`
/// without touching colors. Bold and dim share their "off" code, so turning
/// one off re-applies the other if it stays on. Rare next to colors, so
/// kept out of the hot loop.
#[cold]
#[inline(never)]
fn write_attributes(bytes: &mut Vec<u8>, from: u8, to: u8) {
    let intensity = Cell::BOLD | Cell::DIM;
    let mut off = from & !to;
    let mut on = to & !from;
    if off & intensity != 0 {
        off |= from & intensity;
        on |= to & intensity;
    }
    bytes.extend_from_slice(b"\x1b[");
    let mut first = true;
    let mut push = |bytes: &mut Vec<u8>, code: &[u8]| {
        if !first {
            bytes.push(b';');
        }
        first = false;
        bytes.extend_from_slice(code);
    };
    let mut intensity_off_written = false;
    for &(bit, _, off_code) in &ATTRIBUTE_CODES {
        if off & bit != 0 && !(bit & intensity != 0 && intensity_off_written) {
            push(bytes, off_code);
            intensity_off_written |= bit & intensity != 0;
        }
    }
    for &(bit, on_code, _) in &ATTRIBUTE_CODES {
        if on & bit != 0 {
            push(bytes, on_code);
        }
    }
    bytes.push(b'm');
}

fn monochrome_glyph(cell: Cell, unicode: bool) -> char {
    let foreground = luminance(cell.foreground);
    let background = luminance(cell.background);
    let glyph = char::from_u32(cell.glyph).unwrap_or(' ');
    let value = match glyph {
        ' ' => background,
        '▀' | '▄' => ((u16::from(foreground) + u16::from(background)) / 2) as u8,
        _ => foreground,
    };
    if unicode {
        ramp_glyph(&SHADE_RAMP, value)
    } else {
        ramp_glyph(&ASCII_RAMP, value)
    }
}

/// With `PRECOMPUTED`, `color` is a [`palette_key`]: its palette index is
/// already known and isn't searched for again.
fn write_color<const PRECOMPUTED: bool>(
    output: &mut Vec<u8>,
    mode: TerminalColorMode,
    color: Color,
    foreground: bool,
) {
    // Formatted by hand into a stack buffer and appended once: `write!` and
    // many small appends dominated presentation time. The bytes are identical
    // to the original formatted sequences (see the golden tests).
    if color.kind == 0 {
        output.extend_from_slice(if foreground { b"\x1b[39m" } else { b"\x1b[49m" });
        return;
    }
    if mode == TerminalColorMode::Monochrome {
        return;
    }
    let mut sequence = Sequence::<24>::new();
    sequence.push_slice(b"\x1b[");
    push_color_parameters::<24, PRECOMPUTED>(&mut sequence, mode, color, foreground);
    sequence.push(b'm');
    output.extend_from_slice(sequence.as_bytes());
}

/// Both colors in one SGR sequence: `CSI <foreground> ; <background> m`.
/// Terminals apply SGR parameters left to right, so this sets exactly what
/// the two separate sequences would.
fn write_colors<const PRECOMPUTED: bool>(
    output: &mut Vec<u8>,
    mode: TerminalColorMode,
    foreground: Color,
    background: Color,
) {
    if mode == TerminalColorMode::Monochrome {
        return;
    }
    // Two truecolor parameter lists: at most 2 + 16 + 1 + 16 + 1 bytes.
    let mut sequence = Sequence::<40>::new();
    sequence.push_slice(b"\x1b[");
    push_color_parameters::<40, PRECOMPUTED>(&mut sequence, mode, foreground, true);
    sequence.push(b';');
    push_color_parameters::<40, PRECOMPUTED>(&mut sequence, mode, background, false);
    sequence.push(b'm');
    output.extend_from_slice(sequence.as_bytes());
}

/// The SGR parameters selecting `color`, such as `38;2;255;0;0` or `49`.
#[inline]
fn push_color_parameters<const N: usize, const PRECOMPUTED: bool>(
    sequence: &mut Sequence<N>,
    mode: TerminalColorMode,
    color: Color,
    foreground: bool,
) {
    if color.kind == 0 {
        sequence.push_slice(if foreground { b"39" } else { b"49" });
        return;
    }
    match mode {
        TerminalColorMode::TrueColor => {
            sequence.push_slice(if foreground { b"38;2;" } else { b"48;2;" });
            sequence.push_u8(color.red);
            sequence.push(b';');
            sequence.push_u8(color.green);
            sequence.push(b';');
            sequence.push_u8(color.blue);
        }
        TerminalColorMode::Ansi256 => {
            sequence.push_slice(if foreground { b"38;5;" } else { b"48;5;" });
            sequence.push_u8(if PRECOMPUTED {
                color.red
            } else {
                ansi256_index(color)
            });
        }
        TerminalColorMode::Ansi16 => {
            let index = if PRECOMPUTED {
                color.red
            } else {
                ansi16_index(color)
            };
            let base = if foreground { 30 } else { 40 };
            let bright_base = if foreground { 90 } else { 100 };
            let code = if index < 8 {
                base + index
            } else {
                bright_base + (index - 8)
            };
            sequence.push_u8(code);
        }
        TerminalColorMode::Monochrome => {}
    }
}

/// A short escape sequence assembled on the stack.
struct Sequence<const N: usize = 24> {
    bytes: [u8; N],
    length: usize,
}

impl<const N: usize> Sequence<N> {
    #[inline]
    const fn new() -> Self {
        Self {
            bytes: [0; N],
            length: 0,
        }
    }

    #[inline]
    fn push(&mut self, byte: u8) {
        self.bytes[self.length] = byte;
        self.length += 1;
    }

    #[inline]
    fn push_slice(&mut self, bytes: &[u8]) {
        self.bytes[self.length..self.length + bytes.len()].copy_from_slice(bytes);
        self.length += bytes.len();
    }

    /// Decimal with no padding, exactly as `{}` formats a `u8`.
    #[inline]
    fn push_u8(&mut self, value: u8) {
        if value >= 100 {
            self.push(b'0' + value / 100);
        }
        if value >= 10 {
            self.push(b'0' + value / 10 % 10);
        }
        self.push(b'0' + value % 10);
    }

    #[inline]
    fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.length]
    }
}

/// Appends `value` in decimal ASCII with no padding, exactly as `{}` would.
#[inline]
fn push_decimal(output: &mut Vec<u8>, mut value: usize) {
    if value < 10 {
        output.push(b'0' + value as u8);
        return;
    }
    let mut digits = [0_u8; 20];
    let mut start = digits.len();
    while value > 0 {
        start -= 1;
        digits[start] = b'0' + (value % 10) as u8;
        value /= 10;
    }
    output.extend_from_slice(&digits[start..]);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stack_sequences_format_u8_like_display() {
        for value in 0..=u8::MAX {
            let mut sequence = Sequence::<24>::new();
            sequence.push_u8(value);
            assert_eq!(sequence.as_bytes(), value.to_string().as_bytes());
        }
    }

    #[test]
    fn decimal_formatting_matches_display() {
        let mut bytes = Vec::new();
        for value in (0..70_000).chain([usize::MAX, usize::MAX / 7, 1 << 40]) {
            bytes.clear();
            push_decimal(&mut bytes, value);
            assert_eq!(bytes, value.to_string().as_bytes());
        }
    }

    fn frame(color: Color) -> CellFrame {
        let mut frame = CellFrame::default();
        frame.resize(2, 1).unwrap();
        frame.cells_mut()[0] = Cell::half_block(color, Color::rgb(0, 0, 0));
        frame
    }

    #[test]
    fn unchanged_frame_emits_no_bytes() {
        let frame = frame(Color::rgb(255, 0, 0));
        let mut presenter = AnsiPresenter::new(TerminalColorMode::TrueColor);
        assert!(!presenter.present(&frame).is_empty());
        assert!(presenter.present(&frame).is_empty());
    }

    #[test]
    fn changed_cell_emits_cursor_color_and_glyph() {
        let first = frame(Color::rgb(255, 0, 0));
        let second = frame(Color::rgb(0, 255, 0));
        let mut presenter = AnsiPresenter::new(TerminalColorMode::TrueColor);
        presenter.present(&first);
        let bytes = presenter.present(&second);

        assert!(bytes.starts_with(b"\x1b[1;1H"));
        let color_prefix = b"\x1b[38;2;0;255";
        assert!(
            bytes
                .windows(color_prefix.len())
                .any(|part| part == color_prefix)
        );
        assert!(bytes.ends_with("▀".as_bytes()));
    }

    #[test]
    fn monochrome_uses_ascii_density_instead_of_half_block_lines() {
        let mut frame = CellFrame::default();
        frame.resize(3, 1).unwrap();
        frame.cells_mut()[0] = Cell::half_block(Color::rgb(0, 0, 0), Color::rgb(0, 0, 0));
        frame.cells_mut()[1] =
            Cell::half_block(Color::rgb(128, 128, 128), Color::rgb(128, 128, 128));
        frame.cells_mut()[2] =
            Cell::half_block(Color::rgb(255, 255, 255), Color::rgb(255, 255, 255));
        let mut presenter = AnsiPresenter::with_capabilities(TerminalCapabilities {
            color: TerminalColorMode::Monochrome,
            unicode: false,
        });

        let output = std::str::from_utf8(presenter.present(&frame)).unwrap();
        assert!(output.ends_with(" =@"));
        assert!(!output.contains('▀'));
    }

    #[test]
    fn monochrome_unicode_uses_shaded_blocks() {
        let frame = frame(Color::rgb(255, 255, 255));
        let mut presenter = AnsiPresenter::with_capabilities(TerminalCapabilities {
            color: TerminalColorMode::Monochrome,
            unicode: true,
        });

        let output = std::str::from_utf8(presenter.present(&frame)).unwrap();
        assert!(output.contains('░'));
        assert!(!output.contains('▀'));
    }

    #[test]
    fn embedded_frame_uses_origin_without_clearing_application_ui() {
        let frame = frame(Color::rgb(255, 0, 0));
        let mut presenter = AnsiPresenter::new(TerminalColorMode::TrueColor);
        let bytes = presenter.present_at(&frame, 6, 24);

        assert!(!bytes.windows(4).any(|part| part == b"\x1b[2J"));
        assert!(bytes.starts_with(b"\x1b[6;24H"));
        assert!(bytes.ends_with(b"\x1b[0m"));
    }

    #[test]
    fn render_at_combines_caller_viewport_with_positioned_presentation() {
        let pixels = [255, 0, 0, 255, 0, 0, 255, 255];
        let source = PixelFrameView::new(
            &pixels,
            1,
            2,
            4,
            crate::PixelFormat::Rgba8,
            core::time::Duration::from_millis(250),
        )
        .unwrap();
        let mut output = CellFrame::default();
        let mut presenter = AnsiPresenter::new(TerminalColorMode::TrueColor);

        let positioned = presenter
            .render_at(
                &Renderer::new(),
                source,
                PositionedRenderRequest {
                    render: RenderRequest {
                        viewport: crate::Viewport {
                            width: 3,
                            height: 2,
                        },
                        fit: crate::FitMode::Stretch,
                        generation: 7,
                    },
                    origin_row: 5,
                    origin_column: 10,
                },
                RenderOptions::default(),
                &mut output,
            )
            .unwrap();

        assert_eq!((output.width(), output.height()), (3, 2));
        assert_eq!(output.generation(), 7);
        assert!(positioned.bytes.starts_with(b"\x1b[5;10H"));
        assert!(positioned.bytes.ends_with(b"\x1b[0m"));
    }

    #[test]
    fn embedded_frame_reestablishes_colors_after_its_terminal_reset() {
        let first = frame(Color::rgb(255, 0, 0));
        let second = frame(Color::rgb(0, 255, 0));
        let mut presenter = AnsiPresenter::new(TerminalColorMode::TrueColor);
        presenter.present_at(&first, 6, 24);
        let bytes = presenter.present_at(&second, 6, 24);

        assert!(
            bytes
                .windows(b"\x1b[38;2;0;255;0m".len())
                .any(|part| part == b"\x1b[38;2;0;255;0m")
        );
        assert!(bytes.ends_with(b"\x1b[0m"));
    }

    #[test]
    fn unchanged_embedded_frame_emits_no_bytes() {
        let frame = frame(Color::rgb(255, 0, 0));
        let mut presenter = AnsiPresenter::new(TerminalColorMode::TrueColor);
        presenter.present_at(&frame, 6, 24);

        assert!(presenter.present_at(&frame, 6, 24).is_empty());
    }

    #[test]
    fn moving_unchanged_embedded_frame_redraws_at_new_origin() {
        let frame = frame(Color::rgb(255, 0, 0));
        let mut presenter = AnsiPresenter::new(TerminalColorMode::TrueColor);
        presenter.present_at(&frame, 6, 24);
        let bytes = presenter.present_at(&frame, 8, 12);

        assert!(bytes.starts_with(b"\x1b[8;12H"));
        assert!(!bytes.windows(4).any(|part| part == b"\x1b[2J"));
        assert!(bytes.ends_with(b"\x1b[0m"));
    }

    #[test]
    fn warmed_output_buffer_reuses_capacity() {
        let frame = frame(Color::rgb(255, 0, 0));
        let mut presenter = AnsiPresenter::new(TerminalColorMode::Ansi256);
        presenter.present(&frame);
        let capacity = presenter.output_capacity();
        presenter.present(&frame);

        assert_eq!(presenter.output_capacity(), capacity);
    }

    #[test]
    fn capability_detection_is_conservative_and_overridable() {
        assert_eq!(
            TerminalCapabilities::from_env_values(None, None, false),
            TerminalCapabilities::CONSERVATIVE
        );
        assert_eq!(
            TerminalCapabilities::from_env_values(Some("xterm-256color"), None, false),
            TerminalCapabilities {
                color: TerminalColorMode::Ansi256,
                unicode: true,
            }
        );
        assert_eq!(
            TerminalCapabilities::from_env_values(Some("screen"), Some("truecolor"), false).color,
            TerminalColorMode::TrueColor
        );
        assert_eq!(
            TerminalCapabilities::from_env_values(Some("xterm-256color"), Some("truecolor"), true)
                .color,
            TerminalColorMode::Monochrome
        );
    }

    #[test]
    fn non_unicode_terminal_receives_ascii_glyphs() {
        let frame = frame(Color::rgb(255, 255, 255));
        let mut presenter = AnsiPresenter::with_capabilities(TerminalCapabilities::CONSERVATIVE);
        let bytes = presenter.present(&frame);

        assert!(bytes.contains(&b'='));
        assert!(!bytes.windows("▀".len()).any(|part| part == "▀".as_bytes()));
    }

    #[test]
    fn explicit_black_is_not_emitted_as_terminal_default() {
        let mut frame = CellFrame::default();
        frame.resize(1, 1).unwrap();
        frame.cells_mut()[0] = Cell::half_block(Color::rgb(0, 0, 0), Color::rgb(0, 0, 0));
        let mut presenter = AnsiPresenter::new(TerminalColorMode::TrueColor);
        let bytes = presenter.present(&frame);

        assert!(bytes.windows(13).any(|part| part == b"\x1b[38;2;0;0;0m"));
        assert!(bytes.windows(13).any(|part| part == b"\x1b[48;2;0;0;0m"));
        assert!(!bytes.windows(5).any(|part| part == b"\x1b[39m"));
        assert!(!bytes.windows(5).any(|part| part == b"\x1b[49m"));
    }

    #[test]
    fn changed_run_restores_default_planes_after_black() {
        let mut black = CellFrame::default();
        black.resize(1, 1).unwrap();
        black.cells_mut()[0] = Cell::half_block(Color::rgb(0, 0, 0), Color::rgb(0, 0, 0));
        let mut transparent = CellFrame::default();
        transparent.resize(1, 1).unwrap();
        let mut presenter = AnsiPresenter::new(TerminalColorMode::TrueColor);
        presenter.present(&black);
        let bytes = presenter.present(&transparent);

        assert!(bytes.windows(5).any(|part| part == b"\x1b[39m"));
        assert!(bytes.windows(5).any(|part| part == b"\x1b[49m"));
        assert!(bytes.ends_with(b" "));
    }
}

use std::io::Write;
use std::time::{Duration, Instant};

use crate::cell::{Cell, CellFrame, Color};
use crate::color::{ansi16_index, ansi256_index};
use crate::framebuffer::PixelFrameView;
use crate::renderer::{RenderError, RenderOptions, RenderRequest, Renderer};

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
}

fn contains_ignore_ascii_case(haystack: &str, needle: &str) -> bool {
    haystack
        .as_bytes()
        .windows(needle.len())
        .any(|part| part.eq_ignore_ascii_case(needle.as_bytes()))
}

#[derive(Debug)]
pub struct AnsiPresenter {
    previous: CellFrame,
    previous_origin: Option<(usize, usize)>,
    bytes: Vec<u8>,
    capabilities: TerminalCapabilities,
    foreground: Option<Color>,
    background: Option<Color>,
}

impl AnsiPresenter {
    pub fn new(mode: TerminalColorMode) -> Self {
        Self::with_capabilities(TerminalCapabilities {
            color: mode,
            unicode: true,
        })
    }

    pub fn with_capabilities(capabilities: TerminalCapabilities) -> Self {
        Self {
            previous: CellFrame::default(),
            previous_origin: None,
            bytes: Vec::new(),
            capabilities,
            foreground: None,
            background: None,
        }
    }

    pub fn present<'a>(&'a mut self, current: &CellFrame) -> &'a [u8] {
        self.present_from(current, 1, 1, true)
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
        self.present_from(
            current,
            usize::from(origin_row.max(1)),
            usize::from(origin_column.max(1)),
            false,
        );
        if !self.bytes.is_empty() {
            self.bytes.extend_from_slice(b"\x1b[0m");
            self.foreground = None;
            self.background = None;
        }
        &self.bytes
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

    fn present_from<'a>(
        &'a mut self,
        current: &CellFrame,
        origin_row: usize,
        origin_column: usize,
        clear_on_geometry_change: bool,
    ) -> &'a [u8] {
        self.bytes.clear();
        let geometry_changed =
            self.previous.width() != current.width() || self.previous.height() != current.height();
        let origin_changed = self.previous_origin != Some((origin_row, origin_column));
        let redraw_all = geometry_changed || origin_changed;
        if geometry_changed {
            if clear_on_geometry_change {
                self.bytes.extend_from_slice(b"\x1b[2J");
            }
            self.previous.resize(current.width(), current.height()).ok();
            self.previous.clear();
        }
        if redraw_all {
            self.foreground = None;
            self.background = None;
        }

        let width = usize::from(current.width());
        for row in 0..usize::from(current.height()) {
            let mut column = 0;
            while column < width {
                let index = row * width + column;
                if !redraw_all && current.cells()[index] == self.previous.cells()[index] {
                    column += 1;
                    continue;
                }

                write!(
                    &mut self.bytes,
                    "\x1b[{};{}H",
                    origin_row + row,
                    origin_column + column
                )
                .ok();
                while column < width {
                    let index = row * width + column;
                    if !redraw_all && current.cells()[index] == self.previous.cells()[index] {
                        break;
                    }
                    self.write_cell(current.cells()[index]);
                    column += 1;
                }
            }
        }

        self.previous.cells_mut().copy_from_slice(current.cells());
        self.previous.set_generation(current.generation());
        self.previous_origin = Some((origin_row, origin_column));
        &self.bytes
    }

    pub fn output_capacity(&self) -> usize {
        self.bytes.capacity()
    }

    fn write_cell(&mut self, cell: Cell) {
        if self.capabilities.color == TerminalColorMode::Monochrome {
            let glyph = monochrome_glyph(cell, self.capabilities.unicode);
            let mut encoded = [0; 4];
            self.bytes
                .extend_from_slice(glyph.encode_utf8(&mut encoded).as_bytes());
            return;
        } else {
            if self.foreground != Some(cell.foreground) {
                write_color(
                    &mut self.bytes,
                    self.capabilities.color,
                    cell.foreground,
                    true,
                );
                self.foreground = Some(cell.foreground);
            }
            if self.background != Some(cell.background) {
                write_color(
                    &mut self.bytes,
                    self.capabilities.color,
                    cell.background,
                    false,
                );
                self.background = Some(cell.background);
            }
        }
        let mut encoded = [0; 4];
        let glyph = char::from_u32(cell.glyph).unwrap_or(' ');
        let glyph = if self.capabilities.unicode || glyph.is_ascii() {
            glyph
        } else if glyph == ' ' {
            ' '
        } else {
            '#'
        };
        self.bytes
            .extend_from_slice(glyph.encode_utf8(&mut encoded).as_bytes());
    }
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
        const LEVELS: [char; 5] = [' ', '░', '▒', '▓', '█'];
        LEVELS[usize::from(value) * (LEVELS.len() - 1) / 255]
    } else {
        const LEVELS: [char; 10] = [' ', '.', ':', '-', '=', '+', '*', '#', '%', '@'];
        LEVELS[usize::from(value) * (LEVELS.len() - 1) / 255]
    }
}

fn luminance(color: Color) -> u8 {
    if color.kind == 0 {
        return 0;
    }
    ((u16::from(color.red) * 54 + u16::from(color.green) * 183 + u16::from(color.blue) * 19) / 256)
        as u8
}

fn write_color(output: &mut Vec<u8>, mode: TerminalColorMode, color: Color, foreground: bool) {
    let plane = if foreground { 38 } else { 48 };
    if color.kind == 0 {
        let reset = if foreground { 39 } else { 49 };
        write!(output, "\x1b[{reset}m").ok();
        return;
    }

    match mode {
        TerminalColorMode::TrueColor => {
            write!(
                output,
                "\x1b[{plane};2;{};{};{}m",
                color.red, color.green, color.blue
            )
            .ok();
        }
        TerminalColorMode::Ansi256 => {
            write!(output, "\x1b[{plane};5;{}m", ansi256_index(color)).ok();
        }
        TerminalColorMode::Ansi16 => {
            let index = ansi16_index(color);
            let base = if foreground { 30 } else { 40 };
            let bright_base = if foreground { 90 } else { 100 };
            let code = if index < 8 {
                base + u16::from(index)
            } else {
                bright_base + u16::from(index - 8)
            };
            write!(output, "\x1b[{code}m").ok();
        }
        TerminalColorMode::Monochrome => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

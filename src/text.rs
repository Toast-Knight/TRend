//! Text as terminal cells: labels, HUDs, and status lines drawn over encoded
//! output.
//!
//! Text is written into a [`CellViewMut`] after encoding, so it stays sharp
//! at any encoder resolution and costs one cell per character. A sub-view
//! confines it to a panel; [`CellViewMut::fill`] clears a panel first.
//!
//! ```
//! # use trend::*;
//! # use trend::text::{put_str, TextStyle, TextBackground};
//! let mut cells = CellFrame::default();
//! cells.resize(20, 2).unwrap();
//! let style = TextStyle::new(Color::rgb(255, 255, 0))
//!     .with_background(TextBackground::Keep)
//!     .with_attributes(Cell::BOLD);
//! let written = put_str(&mut cells.view_mut(), 1, 0, "score 42", style);
//! assert_eq!(written, 8);
//! ```

use crate::cell::{Cell, CellViewMut, Color};

/// The background behind each character.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[non_exhaustive]
pub enum TextBackground {
    /// Keep the background color the cell already has, so text over an
    /// encoded image keeps the image's color behind it.
    #[default]
    Keep,
    /// The terminal's default background.
    Default,
    /// A specific color.
    Color(Color),
}

/// How [`put_str`] draws characters.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TextStyle {
    pub foreground: Color,
    pub background: TextBackground,
    /// A combination of [`Cell::BOLD`], [`Cell::UNDERLINE`], and the other
    /// attribute bits.
    pub attributes: u8,
}

impl TextStyle {
    /// Text in `foreground`, keeping the background, with no attributes.
    pub const fn new(foreground: Color) -> Self {
        Self {
            foreground,
            background: TextBackground::Keep,
            attributes: 0,
        }
    }

    pub const fn with_background(mut self, background: TextBackground) -> Self {
        self.background = background;
        self
    }

    pub const fn with_attributes(mut self, attributes: u8) -> Self {
        self.attributes = attributes;
        self
    }
}

/// Writes `text` into `cells` starting at `(column, row)`, one character per
/// cell, and returns how many cells were written. Text is clipped at the
/// view's right edge and is never wrapped.
///
/// Every character must occupy exactly one terminal column, or later
/// output on the row would shift. Control characters (including tabs and
/// newlines), and characters known to be wide (CJK, most emoji) or zero
/// width (combining marks, joiners, variation selectors), are therefore
/// written as `?`. Terminal width tables vary, so hosts that need wide text
/// should lay it out themselves.
pub fn put_str(
    cells: &mut CellViewMut<'_>,
    column: u16,
    row: u16,
    text: &str,
    style: TextStyle,
) -> u16 {
    if row >= cells.height() {
        return 0;
    }
    let line = cells.row_mut(row);
    let start = usize::from(column);
    if start >= line.len() {
        return 0;
    }
    let mut written = 0_u16;
    for (cell, character) in line[start..].iter_mut().zip(text.chars()) {
        let glyph = if single_column(character) {
            character
        } else {
            '?'
        };
        let background = match style.background {
            TextBackground::Keep => cell.background,
            TextBackground::Default => Color::DEFAULT,
            TextBackground::Color(color) => color,
        };
        let mut next = Cell::with_glyph(glyph, style.foreground, background);
        next.attributes = style.attributes & 0x7F;
        *cell = next;
        written += 1;
    }
    written
}

/// False for control characters and for characters that commonly occupy
/// zero or two terminal columns. Conservative: when in doubt, not single.
fn single_column(character: char) -> bool {
    let code = u32::from(character);
    if character.is_control() {
        return false;
    }
    if code < 0x300 {
        return true;
    }
    const ZERO_OR_WIDE: [(u32, u32); 25] = [
        (0x0300, 0x036F), // combining diacritical marks
        (0x0483, 0x0489), // Cyrillic combining marks
        (0x0591, 0x05C7), // Hebrew points
        (0x0610, 0x061A), // Arabic marks
        (0x064B, 0x065F), // Arabic harakat
        (0x1100, 0x115F), // Hangul Jamo leading consonants
        (0x1AB0, 0x1AFF), // combining marks extended
        (0x1DC0, 0x1DFF), // combining marks supplement
        (0x200B, 0x200F), // zero-width space, joiners, marks
        (0x2028, 0x202E), // separators and embedding controls
        (0x2060, 0x2064), // word joiner and invisible operators
        (0x20D0, 0x20FF), // combining marks for symbols
        (0x231A, 0x231B), // watch, hourglass (emoji presentation)
        (0x23E9, 0x23F3), // media emoji
        (0x2E80, 0x303E), // CJK radicals, punctuation
        (0x3041, 0x33FF), // kana, CJK compatibility
        (0x3400, 0x4DBF), // CJK extension A
        (0x4E00, 0xA4CF), // CJK ideographs, Yi
        (0xAC00, 0xD7A3), // Hangul syllables
        (0xF900, 0xFAFF), // CJK compatibility ideographs
        (0xFE00, 0xFE0F), // variation selectors
        (0xFE20, 0xFE6F), // combining half marks, CJK forms, small forms
        (0xFF00, 0xFF60), // fullwidth forms
        (0xFFE0, 0xFFE6), // fullwidth signs
        (0xFFF9, 0xFFFB), // interlinear annotation controls
    ];
    const SUPPLEMENTARY: [(u32, u32); 4] = [
        (0x1F000, 0x1FAFF), // emoji, pictographs, playing cards
        (0x20000, 0x3FFFD), // CJK extensions B and later
        (0xE0000, 0xE007F), // tags
        (0xE0100, 0xE01EF), // variation selectors supplement
    ];
    !ZERO_OR_WIDE
        .iter()
        .chain(&SUPPLEMENTARY)
        .any(|&(first, last)| (first..=last).contains(&code))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn common_narrow_text_passes_through() {
        for character in "Hello, wörld! 42 °C ±½ Ωμ →←↑↓ █▀▄░ ─│┌┐ ✓".chars()
        {
            assert!(single_column(character), "{character:?}");
        }
    }

    #[test]
    fn wide_zero_width_and_control_characters_are_rejected() {
        for character in [
            '\t',
            '\n',
            '\u{7}',
            '\u{85}',
            '漢',
            'か',
            '한',
            'Ａ',
            '\u{301}',
            '\u{200D}',
            '\u{FE0F}',
            '🙂',
            '🚀',
            '\u{20000}',
        ] {
            assert!(!single_column(character), "{character:?}");
        }
    }
}

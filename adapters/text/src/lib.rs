//! Scaled text for TRend: text drawn as pixels, so it grows with the picture instead of staying
//! one terminal cell per character.
//!
//! Terminal text has one size. When an application has many cells to spare (a big window, a small
//! font, a high resolution screen), headings and labels drawn as pixels through a TRend cell
//! encoder can be two, three or ten cells tall. Text is two colours per glyph, which is exactly
//! what TRend's cell encoders reproduce best.
//!
//! - [`Font::pixel`] is a built-in 5×9 pixel font (ASCII and a few symbols), drawn at whole-number
//!   scales: crisp at any size, no files needed.
//! - [`Font::from_ttf`] reads a TrueType font (`.ttf`, or one font of a `.ttc`) that the
//!   application supplies, such as a system font, and draws it anti-aliased at any size.
//!
//! Like the rest of TRend's optional crates it depends only on `trend`.
//!
//! ```
//! use trend::PixelBuffer;
//! use trend_text::{Font, TextStyle, draw_text};
//!
//! let mut pixels = PixelBuffer::new(80, 24).unwrap();
//! let style = TextStyle::new(20.0, [255, 220, 120, 255]);
//! let width = draw_text(&mut pixels.view_mut(), 2.0, 2.0, "Hi!", &Font::pixel(), &style);
//! assert!(width > 0.0);
//! ```

#![forbid(unsafe_code)]

mod coverage;
mod pixel_glyphs;
mod truetype;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use trend::PixelViewMut;

use coverage::{Accumulator, GlyphBitmap};
pub use truetype::FontError;
use truetype::{PathOp, TrueType};

/// Height of the built-in font's line, in its own pixels: 9 rows of glyph and 1 of spacing.
const PIXEL_LINE: f32 = 10.0;
/// Width of one built-in glyph cell, in its own pixels: 5 columns and 1 of spacing.
const PIXEL_ADVANCE: f32 = 6.0;
/// The built-in font's baseline, in its own pixels from the top.
const PIXEL_BASELINE: f32 = 7.0;

/// A typeface: the built-in pixel font or a TrueType font. Cheap to clone and shareable across
/// threads; outline glyphs are cached as they are drawn.
#[derive(Clone)]
pub struct Font {
    kind: Kind,
}

#[derive(Clone)]
enum Kind {
    Pixel,
    Outline(Arc<OutlineFont>),
}

struct OutlineFont {
    font: TrueType,
    cache: Mutex<HashMap<(u16, u32, u8), Arc<GlyphBitmap>>>,
}

impl std::fmt::Debug for Font {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.kind {
            Kind::Pixel => formatter.write_str("Font::pixel"),
            Kind::Outline(_) => formatter.write_str("Font::ttf"),
        }
    }
}

/// How text looks. `size` is the height of a line in pixels: the distance between the tops of two
/// lines of text set one under the other.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextStyle {
    pub size: f32,
    /// Straight RGBA; the alpha fades the whole text.
    pub color: [u8; 4],
    /// Extra pixels between characters (may be negative).
    pub letter_spacing: f32,
}

impl TextStyle {
    pub const fn new(size: f32, color: [u8; 4]) -> Self {
        Self {
            size,
            color,
            letter_spacing: 0.0,
        }
    }
}

/// The size of a piece of text, in pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TextSize {
    pub width: f32,
    pub height: f32,
}

impl Font {
    /// The built-in 5×9 pixel font. It has printable ASCII plus `° · … ‹ › × ↑ ↓ ← → █`; other
    /// characters are drawn as a box.
    pub const fn pixel() -> Self {
        Self { kind: Kind::Pixel }
    }

    /// Reads a TrueType font. For a collection (`.ttc`) this is its first font.
    pub fn from_ttf(bytes: Vec<u8>) -> Result<Self, FontError> {
        Self::from_collection(bytes, 0)
    }

    /// Reads font `index` of a TrueType collection (`.ttc`), or of a plain `.ttf` (index 0).
    pub fn from_collection(bytes: Vec<u8>, index: u32) -> Result<Self, FontError> {
        let font = TrueType::parse(bytes, index)?;
        Ok(Self {
            kind: Kind::Outline(Arc::new(OutlineFont {
                font,
                cache: Mutex::new(HashMap::new()),
            })),
        })
    }

    /// True for the built-in pixel font.
    pub const fn is_pixel(&self) -> bool {
        matches!(self.kind, Kind::Pixel)
    }

    /// The built-in font's whole-number scale for a line `size` (at least 1).
    fn pixel_scale(size: f32) -> f32 {
        (size / PIXEL_LINE).floor().max(1.0)
    }

    /// The size that fits `text` in `max_width` × `max_height` pixels, largest first. For the
    /// pixel font the result is a whole multiple of its line height, so it stays crisp.
    pub fn fit(&self, text: &str, max_width: f32, max_height: f32) -> f32 {
        let at_unit = self.measure(text, &TextStyle::new(PIXEL_LINE, [0; 4]));
        let by_width = if at_unit.width > 0.0 {
            max_width / at_unit.width * PIXEL_LINE
        } else {
            f32::MAX
        };
        let size = by_width.min(max_height).max(1.0);
        match self.kind {
            Kind::Pixel => Self::pixel_scale(size) * PIXEL_LINE,
            Kind::Outline(_) => size.floor(),
        }
    }

    /// How big `text` is when drawn with `style` (one line; `\n` is not special).
    pub fn measure(&self, text: &str, style: &TextStyle) -> TextSize {
        match &self.kind {
            Kind::Pixel => {
                let scale = Self::pixel_scale(style.size);
                let count = text.chars().count() as f32;
                let width = if count > 0.0 {
                    count * (PIXEL_ADVANCE * scale + style.letter_spacing)
                        - scale
                        - style.letter_spacing
                } else {
                    0.0
                };
                TextSize {
                    width: width.max(0.0),
                    height: (PIXEL_LINE * scale).max(style.size),
                }
            }
            Kind::Outline(outline) => {
                let scale = outline.scale(style.size);
                let mut width = 0.0;
                let mut previous = None;
                for character in text.chars() {
                    let glyph = outline.font.glyph(character);
                    if let Some(previous) = previous {
                        width +=
                            outline.font.kerning(previous, glyph) * scale + style.letter_spacing;
                    }
                    width += outline.font.advance(glyph) * scale;
                    previous = Some(glyph);
                }
                TextSize {
                    width,
                    height: style.size,
                }
            }
        }
    }
}

impl OutlineFont {
    /// Pixels per font unit for a line `size`.
    fn scale(&self, size: f32) -> f32 {
        let line = (self.font.ascender - self.font.descender).max(self.font.units_per_em * 0.5);
        size.max(0.0) / line
    }

    /// The glyph's coverage at `scale`, shifted right by `shift` quarter pixels; cached.
    fn bitmap(&self, glyph: u16, scale: f32, shift: u8) -> Arc<GlyphBitmap> {
        let key = (glyph, (scale * 1_000_000.0) as u32, shift);
        if let Ok(cache) = self.cache.lock()
            && let Some(bitmap) = cache.get(&key)
        {
            return Arc::clone(bitmap);
        }
        let bitmap = Arc::new(self.rasterize(glyph, scale, f32::from(shift) / 4.0));
        if let Ok(mut cache) = self.cache.lock() {
            if cache.len() > 8192 {
                cache.clear();
            }
            cache.insert(key, Arc::clone(&bitmap));
        }
        bitmap
    }

    fn rasterize(&self, glyph: u16, scale: f32, shift: f32) -> GlyphBitmap {
        let path = self.font.outline(glyph);
        let empty = GlyphBitmap {
            width: 0,
            height: 0,
            left: 0,
            top: 0,
            coverage: Vec::new(),
        };
        // Bounds in pixels (y down from the baseline).
        let mut bounds = [f32::MAX, f32::MAX, f32::MIN, f32::MIN];
        let mut grow = |x: f32, y: f32| {
            let (px, py) = (x * scale + shift, -y * scale);
            bounds[0] = bounds[0].min(px);
            bounds[1] = bounds[1].min(py);
            bounds[2] = bounds[2].max(px);
            bounds[3] = bounds[3].max(py);
        };
        for op in &path {
            match *op {
                PathOp::Move(x, y) | PathOp::Line(x, y) => grow(x, y),
                PathOp::Quad(cx, cy, x, y) => {
                    grow(cx, cy);
                    grow(x, y);
                }
                PathOp::Close => {}
            }
        }
        if bounds[0] > bounds[2] || !bounds.iter().all(|value| value.is_finite()) {
            return empty;
        }
        let left = bounds[0].floor() as i32;
        let top = bounds[1].floor() as i32;
        let width = (bounds[2].ceil() as i32 - left + 1).clamp(1, 4096) as usize;
        let height = (bounds[3].ceil() as i32 - top + 1).clamp(1, 4096) as usize;
        let to_pixels = |x: f32, y: f32| (x * scale + shift - left as f32, -y * scale - top as f32);
        let mut accumulator = Accumulator::new(width, height);
        let (mut start, mut pen) = ((0.0, 0.0), (0.0, 0.0));
        for op in &path {
            match *op {
                PathOp::Move(x, y) => {
                    start = to_pixels(x, y);
                    pen = start;
                }
                PathOp::Line(x, y) => {
                    let to = to_pixels(x, y);
                    accumulator.line(pen, to);
                    pen = to;
                }
                PathOp::Quad(cx, cy, x, y) => {
                    let to = to_pixels(x, y);
                    accumulator.quad(pen, to_pixels(cx, cy), to);
                    pen = to;
                }
                PathOp::Close => {
                    accumulator.line(pen, start);
                    pen = start;
                }
            }
        }
        GlyphBitmap {
            width: width as u32,
            height: height as u32,
            left,
            top,
            coverage: accumulator.finish(),
        }
    }
}

/// Blends `color` at `coverage` (0-255) into the pixel at `(x, y)`, if it is inside the view.
fn blend(view: &mut PixelViewMut<'_>, x: i32, y: i32, color: [u8; 4], coverage: u8) {
    if x < 0 || y < 0 || x as u32 >= view.width() || y as u32 >= view.height() || coverage == 0 {
        return;
    }
    let alpha = u32::from(coverage) * u32::from(color[3]) / 255;
    if alpha == 0 {
        return;
    }
    let row = view.row_mut(y as u32);
    let pixel = &mut row[x as usize * 4..x as usize * 4 + 4];
    if alpha == 255 {
        pixel.copy_from_slice(&color);
        return;
    }
    let destination_alpha = u32::from(pixel[3]);
    let out_alpha = alpha + destination_alpha * (255 - alpha) / 255;
    if out_alpha == 0 {
        return;
    }
    for channel in 0..3 {
        let source = u32::from(color[channel]) * alpha;
        let destination = u32::from(pixel[channel]) * destination_alpha * (255 - alpha) / 255;
        pixel[channel] = ((source + destination) / out_alpha).min(255) as u8;
    }
    pixel[3] = out_alpha.min(255) as u8;
}

/// Draws one line of `text` with its top-left corner at `(x, y)` pixels (fractions allowed; text
/// outside the view is clipped). Returns the width drawn, as [`Font::measure`] gives it.
pub fn draw_text(
    view: &mut PixelViewMut<'_>,
    x: f32,
    y: f32,
    text: &str,
    font: &Font,
    style: &TextStyle,
) -> f32 {
    match &font.kind {
        Kind::Pixel => {
            let scale = Font::pixel_scale(style.size);
            let line = PIXEL_LINE * scale;
            // Centred in a line taller than the scaled font, on whole pixels.
            let top = (y + ((style.size - line) / 2.0).max(0.0)).round() as i32;
            let mut pen = x;
            let step = scale as i32;
            for character in text.chars() {
                let rows = glyph_rows(character);
                let left = pen.round() as i32;
                for (row, bits) in rows.iter().enumerate() {
                    for column in 0..5 {
                        if bits & (0x10 >> column) == 0 {
                            continue;
                        }
                        for dy in 0..step {
                            for dx in 0..step {
                                blend(
                                    view,
                                    left + column * step + dx,
                                    top + row as i32 * step + dy,
                                    style.color,
                                    255,
                                );
                            }
                        }
                    }
                }
                pen += PIXEL_ADVANCE * scale + style.letter_spacing;
            }
            font.measure(text, style).width
        }
        Kind::Outline(outline) => {
            let scale = outline.scale(style.size);
            // Baseline on a whole pixel keeps horizontal strokes sharp.
            let baseline = (y + outline.font.ascender * scale).round() as i32;
            let mut pen = x;
            let mut previous = None;
            for character in text.chars() {
                let glyph = outline.font.glyph(character);
                if let Some(previous) = previous {
                    pen += outline.font.kerning(previous, glyph) * scale + style.letter_spacing;
                }
                let whole = pen.floor();
                let shift = ((pen - whole) * 4.0).round().min(3.0) as u8;
                let bitmap = outline.bitmap(glyph, scale, shift);
                for row in 0..bitmap.height as i32 {
                    for column in 0..bitmap.width as i32 {
                        let coverage =
                            bitmap.coverage[(row * bitmap.width as i32 + column) as usize];
                        blend(
                            view,
                            whole as i32 + bitmap.left + column,
                            baseline + bitmap.top + row,
                            style.color,
                            coverage,
                        );
                    }
                }
                pen += outline.font.advance(glyph) * scale;
                previous = Some(glyph);
            }
            pen - x
        }
    }
}

/// The built-in glyph rows for `character`, a box if the font lacks it.
fn glyph_rows(character: char) -> &'static [u8; 9] {
    if character == ' ' {
        return &[0; 9];
    }
    pixel_glyphs::GLYPHS
        .binary_search_by_key(&character, |&(glyph, _)| glyph)
        .map_or(&pixel_glyphs::MISSING, |index| {
            &pixel_glyphs::GLYPHS[index].1
        })
}

/// The pixel font's baseline for a line `size`, in pixels from the top of the line: where an
/// underline or a caret would go.
pub fn pixel_baseline(size: f32) -> f32 {
    let scale = Font::pixel_scale(size);
    ((size - PIXEL_LINE * scale) / 2.0).max(0.0) + PIXEL_BASELINE * scale
}

#[cfg(test)]
mod tests {
    use trend::PixelBuffer;

    use super::*;

    fn ink(buffer: &PixelBuffer) -> Vec<(u32, u32)> {
        let view = buffer.view();
        let mut out = Vec::new();
        for y in 0..view.height() {
            for x in 0..view.width() {
                let at = y as usize * view.stride() + x as usize * 4;
                if view.pixels()[at + 3] > 0 {
                    out.push((x, y));
                }
            }
        }
        out
    }

    #[test]
    fn pixel_font_scales_by_whole_numbers() {
        let font = Font::pixel();
        let style = TextStyle::new(10.0, [255; 4]);
        assert_eq!(font.measure("AB", &style).width, 11.0);
        let mut buffer = PixelBuffer::new(40, 30).unwrap();
        draw_text(&mut buffer.view_mut(), 0.0, 0.0, "I", &font, &style);
        let small = ink(&buffer);
        // 'I': a bar of 3 on top and bottom and a stem: 3 + 5 + 3 pixels.
        assert_eq!(small.len(), 11);
        assert!(small.contains(&(2, 3)));
        let mut buffer = PixelBuffer::new(40, 30).unwrap();
        let big = TextStyle::new(29.0, [255; 4]);
        draw_text(&mut buffer.view_mut(), 0.0, 0.0, "I", &font, &big);
        // Scale 2 (29 / 10, rounded down), centred: 4 pixels per font pixel.
        assert_eq!(ink(&buffer).len(), 44);
        assert_eq!(font.fit("Hello", 200.0, 35.0), 30.0);
        assert_eq!(font.fit("Hello", 60.0, 100.0), 20.0);
        assert_eq!(font.fit("Hello", 2.0, 2.0), 10.0, "never below one");
        assert_eq!(pixel_baseline(20.0), 14.0);
        assert_eq!(glyph_rows('é'), &pixel_glyphs::MISSING);
    }

    #[test]
    fn blending_mixes_into_what_is_there() {
        let mut buffer = PixelBuffer::new(1, 1).unwrap();
        let mut view = buffer.view_mut();
        blend(&mut view, 0, 0, [255, 0, 0, 255], 128);
        let pixel = view.row_mut(0)[..4].to_vec();
        assert_eq!(
            pixel,
            vec![255, 0, 0, 128],
            "over transparent: the colour, part alpha"
        );
        view.row_mut(0).copy_from_slice(&[0, 0, 255, 255]);
        blend(&mut view, 0, 0, [255, 0, 0, 255], 128);
        let pixel = view.row_mut(0)[..4].to_vec();
        assert!(
            pixel[0] > 120 && pixel[2] > 120 && pixel[3] == 255,
            "{pixel:?}"
        );
        blend(&mut view, 5, 5, [255; 4], 255);
    }

    #[test]
    fn truetype_glyphs_are_drawn_and_kerned() {
        let font = Font::from_ttf(truetype::tests::tiny_font()).unwrap();
        assert!(!font.is_pixel());
        // Ascender 800, descender -200: a 20 px line is 50 units per pixel.
        let style = TextStyle::new(20.0, [255; 4]);
        let size = font.measure("AB", &style);
        assert_eq!(size.width, (600.0 - 100.0 + 600.0) / 50.0);
        assert_eq!(size.height, 20.0);
        let mut buffer = PixelBuffer::new(40, 24).unwrap();
        let drawn = draw_text(&mut buffer.view_mut(), 0.0, 0.0, "A", &font, &style);
        assert_eq!(drawn, 12.0);
        // The square is x 100..500 (2..10 px), y 0..700 above a baseline 16 px down (2..16 px).
        let view = buffer.view();
        let alpha = |x: usize, y: usize| view.pixels()[y * view.stride() + x * 4 + 3];
        assert_eq!(alpha(5, 10), 255);
        assert_eq!(alpha(1, 10), 0);
        assert_eq!(alpha(11, 10), 0);
        assert_eq!(alpha(5, 17), 0);
        // Same glyph again comes from the cache and looks the same.
        let mut again = PixelBuffer::new(40, 24).unwrap();
        draw_text(&mut again.view_mut(), 0.0, 0.0, "A", &font, &style);
        assert_eq!(again.pixels(), buffer.pixels());
        assert!(font.fit("ABC", 300.0, 40.0) <= 40.0);
    }
}

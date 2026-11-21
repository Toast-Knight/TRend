# trend-text

Optional, dependency-free scaled text for TRend: text drawn as pixels, so it can be as big as the
picture allows instead of one terminal cell per character. Draw it into any `trend::PixelViewMut`
(a `Surface`'s pixels, a `PixelBuffer`) and TRend's cell encoders turn it into cells like any other
picture. Text is two colours per glyph, which is what the cell encoders reproduce best; the sextant
encoder (2 × 3 pixels per cell) is a good fit.

- **`Font::pixel()`**: a built-in 5 × 9 pixel font (printable ASCII and `° · … ‹ › × ↑ ↓ ← → █`),
  drawn at whole-number scales so it stays crisp. No files needed.
- **`Font::from_ttf(bytes)`**: a TrueType font the application supplies (a `.ttf`, or one font of a
  `.ttc` collection, such as a system font), drawn anti-aliased at any size with pair kerning. A
  small TrueType reader (character maps 4 and 12, simple and composite glyphs, `kern`) and a
  signed-area coverage rasterizer are built in; glyphs are cached as they are drawn. Fonts with
  PostScript (CFF) outlines, as in most `.otf` files, are refused with an error.
- **Layout helpers:** `Font::measure`, `Font::fit` (the largest size that fits a box) and
  `pixel_baseline`.

```rust
use trend_text::{Font, TextStyle, draw_text};

let font = Font::from_ttf(std::fs::read("C:/Windows/Fonts/segoeui.ttf")?)?;
surface.draw(|mut pixels, _spec| {
    let style = TextStyle::new(font.fit("Score 42", 200.0, 30.0), [255, 220, 120, 255]);
    draw_text(&mut pixels, 4.0, 2.0, "Score 42", &font, &style);
})?;
```

`size` is the height of a line in pixels. Text is blended over what is already there, so it can be
drawn over a rendered picture.

## Example

```text
cargo run --release -p trend-text --example text_view -- [--font FONT.ttf]
    [--encoder half|quadrant|sextant|braille] [--size COLSxROWS] [--ppm OUT.ppm]
```

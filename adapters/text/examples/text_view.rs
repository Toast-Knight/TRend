//! Shows scaled text in the terminal: the same lines at growing sizes, drawn as pixels and turned
//! into cells by a TRend encoder.
//!
//! ```text
//! cargo run --release -p trend-text --example text_view --
//!     [--font FONT.ttf] [--encoder half|quadrant|sextant|braille] [--size COLSxROWS] [--ppm OUT.ppm]
//! ```
//!
//! Without `--font` it uses the built-in pixel font. `--ppm` also saves the pixels to a picture.

use std::env;
use std::error::Error;
use std::fs;
use std::io::{self, Write};

use trend::{
    Braille, CellEncoder, HalfBlock, OutputOptions, Quadrant, Sextant, Surface,
    TerminalCapabilities, TerminalColorMode, Viewport,
};
use trend_text::{Font, TextStyle, draw_text};

fn main() -> Result<(), Box<dyn Error>> {
    let mut viewport = Viewport {
        width: 100,
        height: 30,
    };
    let mut encoder: Box<dyn CellEncoder> = Box::new(HalfBlock);
    let (mut font, mut ppm) = (Font::pixel(), None);
    let mut arguments = env::args().skip(1);
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--font" => {
                let path = arguments.next().ok_or("--font expects a .ttf file")?;
                font = Font::from_ttf(fs::read(path)?)?;
            }
            "--encoder" => {
                encoder = match arguments.next().as_deref() {
                    Some("half") => Box::new(HalfBlock),
                    Some("quadrant") => Box::new(Quadrant),
                    Some("sextant") => Box::new(Sextant),
                    Some("braille") => Box::new(Braille),
                    _ => return Err("--encoder expects half, quadrant, sextant or braille".into()),
                }
            }
            "--size" => {
                let value = arguments.next().ok_or("--size expects COLSxROWS")?;
                let (columns, rows) = value.split_once('x').ok_or("--size expects COLSxROWS")?;
                viewport = Viewport {
                    width: columns.parse()?,
                    height: rows.parse()?,
                };
            }
            "--ppm" => ppm = Some(arguments.next().ok_or("--ppm expects a file")?),
            other => return Err(format!("unknown argument: {other}").into()),
        }
    }
    let capabilities = TerminalCapabilities {
        color: TerminalColorMode::TrueColor,
        unicode: true,
    };
    let mut surface = Surface::new(encoder, viewport, capabilities)?;
    surface.set_output_options(OutputOptions::new().with_combined_colors(true));
    let mut saved = None;
    surface.draw(|mut pixels, _spec| {
        let (width, height) = (pixels.width(), pixels.height());
        // A dark gradient background.
        for y in 0..height {
            let shade = 20 + (y * 30 / height.max(1)) as u8;
            for pixel in pixels.row_mut(y).chunks_exact_mut(4) {
                pixel.copy_from_slice(&[shade, shade + 4, shade + 10, 255]);
            }
        }
        let lines: [(&str, [u8; 4]); 5] = [
            ("TRend scaled text", [255, 214, 120, 255]),
            ("Headings that grow with the window", [235, 240, 245, 255]),
            ("0123456789 · ‹ value › · 45°", [140, 200, 255, 255]),
            (
                "The quick brown fox jumps over the lazy dog",
                [200, 230, 190, 255],
            ),
            ("small print stays readable too", [170, 175, 185, 255]),
        ];
        let mut y = 2.0;
        let mut size = (height as f32 / 5.0).max(10.0);
        for (text, color) in lines {
            let style = TextStyle::new(font.fit(text, width as f32 - 4.0, size), color);
            draw_text(&mut pixels, 2.0, y, text, &font, &style);
            y += style.size + 2.0;
            size = (size * 0.75).max(10.0);
        }
        if ppm.is_some() {
            let view = pixels.as_view();
            let mut out = format!("P6\n{width} {height}\n255\n").into_bytes();
            for y in 0..height as usize {
                for pixel in
                    view.pixels()[y * view.stride()..][..width as usize * 4].chunks_exact(4)
                {
                    out.extend_from_slice(&pixel[..3]);
                }
            }
            saved = Some(out);
        }
    })?;
    if let (Some(path), Some(bytes)) = (ppm, saved) {
        fs::write(path, bytes)?;
    }
    let mut stdout = io::stdout().lock();
    write!(stdout, "\x1b[2J")?;
    surface.present(&mut stdout)?;
    writeln!(stdout, "\x1b[{};1H", viewport.height + 1)?;
    Ok(())
}

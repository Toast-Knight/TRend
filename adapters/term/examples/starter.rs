//! The smallest complete TRend program: an animated picture that follows the terminal's size,
//! a square you move with the arrow keys, and a clean exit on Esc or Q.
//!
//! ```text
//! cargo run --release -p trend-term --example starter
//! ```
//!
//! Change the `draw` closure to draw anything: every pixel is yours.

use std::time::{Duration, Instant};

use trend::{HalfBlock, Surface};
use trend_term::{Event, Key, Session, SessionOptions};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut session = Session::enter(SessionOptions::default())?;
    let info = session.info();
    let mut surface = Surface::new(HalfBlock, info.viewport, info.capabilities)?;
    surface.set_output_options(info.recommended_output_options());
    let (started, mut square) = (Instant::now(), (0.5_f32, 0.5_f32));
    loop {
        while let Some(event) = session.poll(Duration::ZERO)? {
            match event {
                Event::Resize(viewport) => surface.resize(viewport)?,
                Event::Key {
                    key: Key::Esc | Key::Char('q'),
                    ..
                } => return Ok(()),
                _ => {}
            }
        }
        // Held keys move the square smoothly, whatever the frame rate.
        let step = |positive: Key, negative: Key| {
            0.01 * (f32::from(session.is_down(positive)) - f32::from(session.is_down(negative)))
        };
        square.0 = (square.0 + step(Key::Right, Key::Left)).clamp(0.0, 1.0);
        square.1 = (square.1 + step(Key::Down, Key::Up)).clamp(0.0, 1.0);
        let time = started.elapsed().as_secs_f32();
        surface.draw(|mut pixels, _spec| {
            let (width, height) = (pixels.width() as f32, pixels.height() as f32);
            for y in 0..pixels.height() {
                for (x, pixel) in pixels.row_mut(y).chunks_exact_mut(4).enumerate() {
                    let (u, v) = (x as f32 / width, y as f32 / height);
                    let wave = (u * 9.0 + time).sin() + (v * 7.0 - time * 0.7).cos();
                    let near = (u - square.0).abs() < 0.04 && (v - square.1).abs() < 0.07;
                    let color = |phase: f32| (127.0 + 120.0 * (wave + phase).sin()) as u8;
                    let rgb = if near {
                        [255; 3]
                    } else {
                        [color(0.0), color(2.1), color(4.2)]
                    };
                    pixel.copy_from_slice(&[rgb[0], rgb[1], rgb[2], 255]);
                }
            }
        })?;
        session.write_frame(surface.present_bytes())?;
        std::thread::sleep(Duration::from_millis(16));
    }
}

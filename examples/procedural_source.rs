//! TRend as the output target of an arbitrary renderer.
//!
//! Two independent "applications" draw every frame, using only public APIs:
//!
//! - **Native mode** (top): the app asks for a `RasterSpec`, renders straight
//!   into a `PixelBuffer` at the encoder's resolution, and uses the spec's
//!   pixel aspect so its circle stays round under any encoder.
//! - **Scaled mode** (bottom): the app owns a fixed 320x180 framebuffer with
//!   padded rows (like a GPU readback). TRend borrows it as a
//!   `PixelFrameView` and resamples it with `ImageSource`.
//!
//! Each mode is a `Surface`, which keeps the spec, pixel buffer, cells, and
//! presenter consistent. `Surface` only bundles public steps; it produces
//! the same bytes as calling them by hand (this example did so until M4).
//!
//! ```text
//! cargo run --example procedural_source -- [--encoder half|quadrant|sextant|braille|density]
//!     [--size COLSxROWS] [--frames N] [--once]
//! ```

use std::env;
use std::error::Error;
use std::io::{self, Write};
use std::thread;
use std::time::{Duration, Instant};

use trend::{
    Braille, CellEncoder, Density, DensityRamp, FitMode, HalfBlock, ImageSource, PixelFormat,
    PixelFrameView, PixelViewMut, Quadrant, RasterSpec, SamplingFilter, Sextant, Surface,
    TerminalCapabilities, TerminalColorMode, Viewport,
};

struct Options {
    encoder: Box<dyn CellEncoder>,
    encoder_name: &'static str,
    viewport: Viewport,
    frames: u32,
    once: bool,
}

fn parse_options() -> Result<Options, Box<dyn Error>> {
    let mut options = Options {
        encoder: Box::new(Quadrant),
        encoder_name: "quadrant",
        viewport: Viewport {
            width: 48,
            height: 12,
        },
        frames: 120,
        once: false,
    };
    let mut arguments = env::args().skip(1);
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--encoder" => match arguments.next().as_deref() {
                Some("half") => {
                    options.encoder = Box::new(HalfBlock);
                    options.encoder_name = "half";
                }
                Some("quadrant") => {
                    options.encoder = Box::new(Quadrant);
                    options.encoder_name = "quadrant";
                }
                Some("sextant") => {
                    options.encoder = Box::new(Sextant);
                    options.encoder_name = "sextant";
                }
                Some("braille") => {
                    options.encoder = Box::new(Braille);
                    options.encoder_name = "braille";
                }
                Some("density") => {
                    options.encoder = Box::new(Density::new(DensityRamp::Shades, true));
                    options.encoder_name = "density";
                }
                _ => {
                    return Err(
                        "--encoder expects half, quadrant, sextant, braille, or density".into(),
                    );
                }
            },
            "--size" => {
                let value = arguments.next().ok_or("--size expects COLSxROWS")?;
                let (width, height) = value.split_once('x').ok_or("--size expects COLSxROWS")?;
                options.viewport = Viewport {
                    width: width.parse()?,
                    height: height.parse()?,
                };
            }
            "--frames" => options.frames = arguments.next().ok_or("--frames expects N")?.parse()?,
            "--once" => options.once = true,
            "-h" | "--help" => {
                println!(
                    "usage: procedural_source [--encoder half|quadrant|sextant|braille|density] [--size COLSxROWS] [--frames N] [--once]"
                );
                std::process::exit(0);
            }
            other => return Err(format!("unknown argument: {other}").into()),
        }
    }
    Ok(options)
}

/// Native-mode app: draws directly at the encoder's pixel resolution.
///
/// It knows nothing about cells or glyphs; it only reads the size and the
/// physical pixel aspect from the spec, exactly as a 2D or 3D renderer would
/// feed a projection matrix.
fn draw_native(spec: &RasterSpec, time: f32, mut target: PixelViewMut<'_>) {
    let width = spec.width();
    let height = spec.height();
    let aspect = spec.pixel_aspect();
    // Physical size of one pixel, normalized so height is 1.
    let pixel_width = aspect.width as f32 / aspect.height as f32;
    let physical_width = width as f32 * pixel_width;
    let physical_height = height as f32;
    let radius = physical_height * 0.3;
    let orbit = (physical_width - 2.0 * radius).max(0.0) * 0.5;
    let center = (
        physical_width * 0.5 + orbit * (time * 0.9).cos(),
        physical_height * 0.5,
    );

    for row in 0..height {
        for (column, pixel) in target.row_mut(row).chunks_exact_mut(4).enumerate() {
            let (px, py) = ((column as f32 + 0.5) * pixel_width, row as f32 + 0.5);
            let plasma = ((px * 0.35 + time).sin()
                + (py * 0.5 - time * 1.3).sin()
                + ((px + py) * 0.2 + time * 0.7).sin())
                / 3.0;
            let shade = ((plasma + 1.0) * 0.5 * 120.0) as u8;
            let dx = px - center.0;
            let dy = py - center.1;
            let color = if dx * dx + dy * dy <= radius * radius {
                [250, 200, 40, 255]
            } else {
                [shade / 3, shade / 2 + 20, shade + 60, 255]
            };
            pixel.copy_from_slice(&color);
        }
    }
}

/// Scaled-mode app: an existing fixed-size framebuffer with padded rows,
/// standing in for a game, emulator, browser, or GPU readback.
struct AppFramebuffer {
    pixels: Vec<u8>,
    width: u32,
    height: u32,
    stride: usize,
}

impl AppFramebuffer {
    fn new(width: u32, height: u32, row_padding: usize) -> Self {
        let stride = width as usize * 4 + row_padding;
        Self {
            pixels: vec![0; stride * height as usize],
            width,
            height,
            stride,
        }
    }

    /// A checkerboard floor, a sky gradient, and a bouncing square.
    fn draw(&mut self, time: f32) {
        let horizon = self.height / 2;
        let size = self.height as f32 * 0.25;
        let travel = self.width as f32 - size;
        let square_x = (travel * (0.5 + 0.5 * (time * 1.1).sin())) as u32;
        let bounce = (time * 3.0).sin().abs();
        let square_y = (horizon as f32 - size - bounce * size * 0.8) as u32;
        for y in 0..self.height {
            let row = &mut self.pixels[y as usize * self.stride..][..self.width as usize * 4];
            for (x, pixel) in row.chunks_exact_mut(4).enumerate() {
                let x = x as u32;
                let inside_square = x >= square_x
                    && x < square_x + size as u32
                    && y >= square_y
                    && y < square_y + size as u32;
                let color = if inside_square {
                    [230, 60, 60, 255]
                } else if y < horizon {
                    [40, 60 + (y * 120 / horizon) as u8, 160, 255]
                } else {
                    let depth = (y - horizon + 1) as f32;
                    let u =
                        ((x as f32 - self.width as f32 / 2.0) / depth * 8.0 + time * 20.0) as i32;
                    let v = (400.0 / depth + time * 10.0) as i32;
                    if (u.div_euclid(16) + v.div_euclid(4)) % 2 == 0 {
                        [200, 200, 190, 255]
                    } else {
                        [60, 70, 60, 255]
                    }
                };
                pixel.copy_from_slice(&color);
            }
        }
    }

    fn view(&self) -> PixelFrameView<'_> {
        PixelFrameView::new(
            &self.pixels,
            self.width,
            self.height,
            self.stride,
            PixelFormat::Rgba8,
            Duration::ZERO,
        )
        .expect("app framebuffer is valid")
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    let options = parse_options()?;
    let encoder = options.encoder.as_ref();
    let capabilities = TerminalCapabilities {
        color: TerminalColorMode::TrueColor,
        unicode: true,
    };
    // One surface per mode; each owns its spec, pixels, cells, and presenter.
    let mut native = Surface::new(encoder, options.viewport, capabilities)?;
    let mut scaled = Surface::new(encoder, options.viewport, capabilities)?;
    let spec = *native.spec();
    let mut app = AppFramebuffer::new(320, 180, 64);

    let native_row = 2;
    let scaled_row = native_row + options.viewport.height + 2;
    let status_row = scaled_row + options.viewport.height + 1;
    native.set_origin(native_row, 1);
    scaled.set_origin(scaled_row, 1);
    let mut stdout = io::stdout().lock();
    write!(stdout, "\x1b[2J\x1b[?25l")?;
    write!(
        stdout,
        "\x1b[1;1Hnative: app renders {}x{} px (pixel aspect {}:{})",
        spec.width(),
        spec.height(),
        spec.pixel_aspect().width,
        spec.pixel_aspect().height
    )?;
    write!(
        stdout,
        "\x1b[{};1Hscaled: app framebuffer {}x{} (stride {}) -> ImageSource -> {}x{} px",
        scaled_row - 1,
        app.width,
        app.height,
        app.stride,
        spec.width(),
        spec.height()
    )?;

    let frames = if options.once {
        1
    } else {
        options.frames.max(1)
    };
    let started = Instant::now();
    let frame_time = Duration::from_millis(33);
    for frame in 0..frames {
        let time = if options.once {
            1.0
        } else {
            started.elapsed().as_secs_f32()
        };

        // Native: render at encoder resolution; the surface encodes.
        native.draw(|pixels, spec| draw_native(spec, time, pixels))?;

        // Scaled: the app draws its own buffer; TRend borrows and resamples it.
        app.draw(time);
        scaled.encode_image(
            &ImageSource::new(app.view())
                .with_fit(FitMode::Contain)
                .with_sampling(SamplingFilter::Area),
        )?;

        let native_len = native.present(&mut stdout)?.bytes;
        let scaled_len = scaled.present(&mut stdout)?.bytes;

        write!(
            stdout,
            "\x1b[{status_row};1H\x1b[2Kencoder {} ({}x{} px/cell)  frame {}/{}  ANSI bytes: native {} scaled {}",
            options.encoder_name,
            encoder.grid().columns,
            encoder.grid().rows,
            frame + 1,
            frames,
            native_len,
            scaled_len
        )?;
        stdout.flush()?;

        if !options.once {
            let next = frame_time * (frame + 1);
            if let Some(wait) = next.checked_sub(started.elapsed()) {
                thread::sleep(wait);
            }
        }
    }
    writeln!(stdout, "\x1b[?25h")?;
    Ok(())
}

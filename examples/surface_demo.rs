//! `Surface`: the convenience layer for a real integration.
//!
//! Two surfaces share one screen:
//!
//! - **scene** (top): the app draws into the surface's pixel buffer through
//!   `Surface::draw`. After the first frame only a sprite moves, so the app
//!   redraws, re-encodes, and presents just the sprite's damage.
//! - **image** (bottom): an app-owned framebuffer placed with `ImageSource`
//!   and `Surface::encode_image`. It doesn't change, so after the first frame
//!   presenting it writes nothing. A terminal position is mapped back to the
//!   image's pixel, as a mouse handler would.
//!
//! The encoder is chosen from the terminal's capabilities, and output uses
//! synchronized updates and combined color sequences.
//!
//! ```text
//! cargo run --example surface_demo -- [--size COLSxROWS] [--frames N] [--once]
//! ```

use std::env;
use std::error::Error;
use std::io::{self, Write};
use std::thread;
use std::time::Duration;

use trend::{
    CellEncoder, Density, DensityRamp, FitMode, HalfBlock, ImageSource, OutputOptions, PixelBuffer,
    PixelRect, PixelViewMut, Quadrant, SamplingFilter, Surface, TerminalEnvironment, Viewport,
};

/// The first encoder, in order of preference, that this terminal shows
/// faithfully. TRend reports requirements; the policy is the host's.
fn choose_encoder(environment: &TerminalEnvironment) -> Box<dyn CellEncoder> {
    let preference: [Box<dyn CellEncoder>; 4] = [
        Box::new(Quadrant),
        Box::new(HalfBlock),
        Box::new(Density::new(DensityRamp::Shades, false)),
        Box::new(Density::ASCII),
    ];
    preference
        .into_iter()
        .find(|encoder| {
            encoder
                .requirements()
                .supported_by(environment.capabilities)
        })
        .unwrap_or_else(|| Box::new(Density::ASCII))
}

fn background(x: u32, y: u32, width: u32, height: u32) -> [u8; 4] {
    let red = (x * 90 / width.max(1)) as u8;
    let blue = (60 + y * 120 / height.max(1)) as u8;
    let grid = if x.is_multiple_of(16) || y.is_multiple_of(16) {
        30
    } else {
        0
    };
    [red + grid, 20 + grid, blue, 255]
}

/// The sprite's bounding box at `frame`, in scene pixels.
fn sprite(frame: u32, width: u32, height: u32) -> PixelRect {
    let size = (height / 3).clamp(2, 12);
    let travel = width.saturating_sub(size).max(1);
    let x = frame * 2 % travel;
    PixelRect::new(x, (height - size) / 2, size, size)
}

fn union(a: PixelRect, b: PixelRect) -> PixelRect {
    let left = a.x.min(b.x);
    let top = a.y.min(b.y);
    let right = (a.x + a.width).max(b.x + b.width);
    let bottom = (a.y + a.height).max(b.y + b.height);
    PixelRect::new(left, top, right - left, bottom - top)
}

/// Draws the scene inside `area` only (the whole scene on the first frame).
fn draw_scene(mut pixels: PixelViewMut<'_>, area: PixelRect, frame: u32) {
    let (width, height) = (pixels.width(), pixels.height());
    let sprite = sprite(frame, width, height);
    for y in area.y..area.y + area.height {
        let row = pixels.row_mut(y);
        for x in area.x..area.x + area.width {
            let inside = (sprite.x..sprite.x + sprite.width).contains(&x)
                && (sprite.y..sprite.y + sprite.height).contains(&y);
            let color = if inside {
                [250, 200, 40, 255]
            } else {
                background(x, y, width, height)
            };
            row[x as usize * 4..x as usize * 4 + 4].copy_from_slice(&color);
        }
    }
}

/// An app-owned "photo": a 256×160 framebuffer the app keeps.
fn photo() -> PixelBuffer {
    let mut photo = PixelBuffer::new(256, 160).expect("fixed size");
    let mut view = photo.view_mut();
    for y in 0..160 {
        for (x, pixel) in view.row_mut(y).chunks_exact_mut(4).enumerate() {
            let x = x as u32;
            let (dx, dy) = (x as i32 - 128, y as i32 - 80);
            let ring = ((dx * dx + dy * dy) as u32 / 64).is_multiple_of(2);
            let shade = ((x + y) / 2) as u8;
            pixel.copy_from_slice(&if ring {
                [shade, 120, 200 - shade / 2, 255]
            } else {
                [40, shade / 2, 60, 255]
            });
        }
    }
    photo
}

fn main() -> Result<(), Box<dyn Error>> {
    let mut viewport = Viewport {
        width: 60,
        height: 10,
    };
    let (mut frames, mut once) = (90, false);
    let mut arguments = env::args().skip(1);
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--size" => {
                let value = arguments.next().ok_or("--size needs COLSxROWS")?;
                let (columns, rows) = value.split_once('x').ok_or("--size needs COLSxROWS")?;
                viewport = Viewport {
                    width: columns.parse()?,
                    height: rows.parse()?,
                };
            }
            "--frames" => frames = arguments.next().ok_or("--frames needs N")?.parse()?,
            "--once" => once = true,
            other => return Err(format!("unknown argument {other}").into()),
        }
    }

    let environment = TerminalEnvironment::detect();
    let options = OutputOptions::new()
        .with_synchronized_updates(true)
        .with_combined_colors(true);

    let mut scene = Surface::new(
        choose_encoder(&environment),
        viewport,
        environment.capabilities,
    )?;
    scene.set_output_options(options);
    scene.set_origin(2, 1);

    let mut image = Surface::new(
        choose_encoder(&environment),
        viewport,
        environment.capabilities,
    )?;
    image.set_output_options(options);
    image.set_origin(viewport.height + 4, 1);
    let photo = photo();
    image.encode_image(
        &ImageSource::new(photo.view())
            .with_fit(FitMode::Contain)
            .with_sampling(SamplingFilter::Area),
    )?;

    let mut stdout = io::stdout().lock();
    let status_row = viewport.height * 2 + 5;
    write!(
        stdout,
        "\x1b[2J\x1b[?25l\x1b[1;1Hscene: draw + damage regions"
    )?;
    write!(stdout, "\x1b[{};1Himage: encode_image", viewport.height + 3)?;

    // Input mapping, as a mouse handler would do it: the terminal cell at the
    // image's center, back to a pixel of the app's photo.
    let (origin_row, origin_column) = image.origin();
    let click = (
        origin_row + viewport.height / 2,
        origin_column + viewport.width / 2,
    );
    let clicked = image
        .cell_at_terminal(click.0, click.1)
        .and_then(|(column, row)| image.source_mapping()?.cell_to_source(column, row));

    let frames = if once { 1 } else { frames.max(1) };
    let (width, height) = (scene.spec().width(), scene.spec().height());
    for frame in 0..frames {
        let written = if frame == 0 {
            scene.draw(|pixels, spec| {
                draw_scene(pixels, PixelRect::new(0, 0, spec.width(), spec.height()), 0)
            })?;
            scene.present(&mut stdout)?
        } else {
            let damage = union(
                sprite(frame - 1, width, height),
                sprite(frame, width, height),
            );
            draw_scene(scene.pixels_mut(), damage, frame);
            match scene.encode_region(damage)? {
                Some(cells) => scene.present_region(cells, &mut stdout)?,
                None => Default::default(),
            }
        };
        let image_bytes = image.present(&mut stdout)?.bytes;
        write!(
            stdout,
            "\x1b[{status_row};1H\x1b[2Kframe {frame:>3}: scene {:>6} bytes, {:>4} cells, {:>3} runs | image {:>6} bytes | click at {click:?} -> photo pixel {clicked:?}",
            written.bytes, written.cells, written.runs, image_bytes
        )?;
        stdout.flush()?;
        if !once {
            thread::sleep(Duration::from_millis(33));
        }
    }
    write!(stdout, "\x1b[{};1H\x1b[?25h", status_row + 1)?;
    stdout.flush()?;
    Ok(())
}

//! A HUD over software 3D: TRend as the output of a tiny renderer.
//!
//! Everything 3D lives in this example, not in TRend: a rotating cube with
//! flat shading, perspective, and a depth buffer, rasterized into the
//! surface's pixels at the encoder's resolution and pixel aspect. Then:
//!
//! 1. `compose::blend_over` puts a translucent radar sprite over the frame,
//!    and `Surface::encode_region` re-encodes just the cells it touched;
//! 2. `text::put_str` writes a styled status panel into the cells;
//! 3. `Surface::present` sends only what changed, with synchronized updates.
//!
//! ```text
//! cargo run --release --example hud_3d -- [--encoder half|quadrant|sextant|braille]
//!     [--size COLSxROWS] [--frames N] [--once]
//! ```

use std::env;
use std::error::Error;
use std::io::{self, Write};
use std::thread;
use std::time::{Duration, Instant};

use trend::compose::blend_over;
use trend::text::{TextBackground, TextStyle, put_str};
use trend::{
    Braille, Cell, CellEncoder, CellRect, Color, HalfBlock, OutputOptions, PixelBuffer,
    PixelViewMut, Quadrant, RasterSpec, Sextant, Surface, TerminalCapabilities, TerminalColorMode,
    Viewport,
};

type Vec3 = [f32; 3];

/// Cube corners and faces (as corner indices, counter-clockwise from
/// outside) with a base color each.
const CORNERS: [Vec3; 8] = [
    [-1.0, -1.0, -1.0],
    [1.0, -1.0, -1.0],
    [1.0, 1.0, -1.0],
    [-1.0, 1.0, -1.0],
    [-1.0, -1.0, 1.0],
    [1.0, -1.0, 1.0],
    [1.0, 1.0, 1.0],
    [-1.0, 1.0, 1.0],
];
const FACES: [([usize; 4], [f32; 3]); 6] = [
    ([4, 5, 6, 7], [230.0, 80.0, 70.0]),
    ([1, 0, 3, 2], [70.0, 200.0, 110.0]),
    ([5, 1, 2, 6], [80.0, 120.0, 240.0]),
    ([0, 4, 7, 3], [240.0, 200.0, 60.0]),
    ([7, 6, 2, 3], [200.0, 90.0, 220.0]),
    ([0, 1, 5, 4], [60.0, 210.0, 220.0]),
];

fn rotate(point: Vec3, time: f32) -> Vec3 {
    let (sin_y, cos_y) = (time * 0.9).sin_cos();
    let (sin_x, cos_x) = (time * 0.6).sin_cos();
    let [x, y, z] = point;
    let (x, z) = (x * cos_y + z * sin_y, -x * sin_y + z * cos_y);
    let (y, z) = (y * cos_x - z * sin_x, y * sin_x + z * cos_x);
    [x, y, z + 4.2]
}

fn cross(a: Vec3, b: Vec3) -> Vec3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn sub(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

/// Renders the scene into `pixels`. `depth` is the app's own depth buffer.
fn render(pixels: &mut PixelViewMut<'_>, spec: &RasterSpec, depth: &mut Vec<f32>, time: f32) {
    let (width, height) = (spec.width() as usize, spec.height() as usize);
    depth.clear();
    depth.resize(width * height, f32::INFINITY);

    // Sky and floor.
    for y in 0..height {
        let t = y as f32 / height as f32;
        let row = pixels.row_mut(y as u32);
        for (x, pixel) in row.chunks_exact_mut(4).enumerate() {
            let color = if t < 0.62 {
                [
                    (20.0 + 40.0 * t) as u8,
                    (30.0 + 60.0 * t) as u8,
                    (70.0 + 90.0 * t) as u8,
                ]
            } else {
                let checker = ((x as f32 / width as f32 * 16.0) as i32 + (t * 30.0) as i32) % 2;
                if checker == 0 {
                    [40, 44, 52]
                } else {
                    [28, 30, 36]
                }
            };
            pixel.copy_from_slice(&[color[0], color[1], color[2], 255]);
        }
    }

    // Projection honoring the physical pixel shape, so the cube stays a cube
    // under any encoder.
    let aspect = spec.pixel_aspect();
    let pixel_width = aspect.width as f32 / aspect.height as f32;
    let focal = height as f32 * 0.9;
    let project = |point: Vec3| {
        [
            width as f32 / 2.0 + point[0] / point[2] * focal / pixel_width,
            height as f32 / 2.0 - point[1] / point[2] * focal,
            point[2],
        ]
    };
    let light = [0.4_f32, 0.7, -0.6];
    let corners = CORNERS.map(|corner| rotate(corner, time));
    for (face, color) in FACES {
        let [a, b, c, d] = face.map(|index| corners[index]);
        let normal = cross(sub(b, a), sub(c, a));
        if normal[0] * a[0] + normal[1] * a[1] + normal[2] * a[2] >= 0.0 {
            continue; // Facing away.
        }
        let length = (normal[0].powi(2) + normal[1].powi(2) + normal[2].powi(2)).sqrt();
        let lit = ((normal[0] * light[0] + normal[1] * light[1] + normal[2] * light[2]) / length)
            .max(0.0)
            * 0.75
            + 0.25;
        let shade = color.map(|channel| (channel * lit) as u8);
        for triangle in [[a, b, c], [a, c, d]] {
            fill_triangle(pixels, depth, width, height, triangle.map(&project), shade);
        }
    }
}

/// Edge-function rasterization with a depth test.
fn fill_triangle(
    pixels: &mut PixelViewMut<'_>,
    depth: &mut [f32],
    width: usize,
    height: usize,
    [p0, p1, p2]: [Vec3; 3],
    color: [u8; 3],
) {
    let edge =
        |a: Vec3, b: Vec3, x: f32, y: f32| (b[0] - a[0]) * (y - a[1]) - (b[1] - a[1]) * (x - a[0]);
    let area = edge(p0, p1, p2[0], p2[1]);
    if area.abs() < f32::EPSILON {
        return;
    }
    let min_x = p0[0].min(p1[0]).min(p2[0]).floor().max(0.0) as usize;
    let max_x = (p0[0].max(p1[0]).max(p2[0]).ceil() as usize).min(width);
    let min_y = p0[1].min(p1[1]).min(p2[1]).floor().max(0.0) as usize;
    let max_y = (p0[1].max(p1[1]).max(p2[1]).ceil() as usize).min(height);
    for y in min_y..max_y {
        let row = pixels.row_mut(y as u32);
        for x in min_x..max_x {
            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
            let w0 = edge(p1, p2, px, py) / area;
            let w1 = edge(p2, p0, px, py) / area;
            let w2 = edge(p0, p1, px, py) / area;
            if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                continue;
            }
            let z = w0 * p0[2] + w1 * p1[2] + w2 * p2[2];
            if z < depth[y * width + x] {
                depth[y * width + x] = z;
                row[x * 4..x * 4 + 4].copy_from_slice(&[color[0], color[1], color[2], 255]);
            }
        }
    }
}

/// A translucent radar: a dark disc with rings and a sweeping line, `size`
/// pixels tall and as many wide as keep it round on `spec`'s pixels.
fn radar(spec: &RasterSpec, size: u32, time: f32) -> PixelBuffer {
    let aspect = spec.pixel_aspect();
    let width = (size * aspect.height / aspect.width).max(1);
    let mut sprite = PixelBuffer::new(width, size).expect("non-zero");
    let center = size as f32 / 2.0;
    let pixel_width = aspect.width as f32 / aspect.height as f32;
    let sweep = time * 2.0;
    let mut view = sprite.view_mut();
    for y in 0..size {
        for (x, pixel) in view.row_mut(y).chunks_exact_mut(4).enumerate() {
            let dx = (x as f32 + 0.5) * pixel_width - center;
            let dy = y as f32 + 0.5 - center;
            let radius = (dx * dx + dy * dy).sqrt() / center;
            let angle = dy.atan2(dx);
            let on_sweep = ((angle - sweep).rem_euclid(std::f32::consts::TAU)) < 0.35;
            let color = if radius > 1.0 {
                [0, 0, 0, 0]
            } else if (radius * 3.0).fract() < 0.12 || on_sweep {
                [90, 255, 140, 220]
            } else {
                [10, 30, 20, 150]
            };
            pixel.copy_from_slice(&color);
        }
    }
    sprite
}

fn main() -> Result<(), Box<dyn Error>> {
    let mut viewport = Viewport {
        width: 72,
        height: 22,
    };
    let (mut frames, mut once) = (240_u32, false);
    let mut encoder: (Box<dyn CellEncoder>, &str) = (Box::new(Sextant), "sextant");
    let mut arguments = env::args().skip(1);
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--encoder" => {
                encoder = match arguments.next().as_deref() {
                    Some("half") => (Box::new(HalfBlock), "half"),
                    Some("quadrant") => (Box::new(Quadrant), "quadrant"),
                    Some("sextant") => (Box::new(Sextant), "sextant"),
                    Some("braille") => (Box::new(Braille), "braille"),
                    _ => return Err("--encoder expects half, quadrant, sextant, or braille".into()),
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
            "--frames" => frames = arguments.next().ok_or("--frames expects N")?.parse()?,
            "--once" => once = true,
            other => return Err(format!("unknown argument: {other}").into()),
        }
    }

    let capabilities = TerminalCapabilities {
        color: TerminalColorMode::TrueColor,
        unicode: true,
    };
    let (encoder, encoder_name) = encoder;
    let mut surface = Surface::new(encoder, viewport, capabilities)?;
    surface.set_output_options(
        OutputOptions::new()
            .with_synchronized_updates(true)
            .with_combined_colors(true),
    );
    let mut depth = Vec::new();
    let radar_size = (surface.spec().height() / 3).max(8);
    let label = TextStyle::new(Color::rgb(255, 230, 120)).with_attributes(Cell::BOLD);
    let value = TextStyle::new(Color::rgb(220, 230, 240));

    let mut stdout = io::stdout().lock();
    write!(stdout, "\x1b[2J\x1b[?25l")?;
    let frames = if once { 1 } else { frames.max(1) };
    let started = Instant::now();
    let mut last_bytes = 0;
    for frame in 0..frames {
        let time = if once {
            1.0
        } else {
            started.elapsed().as_secs_f32()
        };

        // 1. The app renders its scene; the surface encodes it.
        surface.draw(|mut pixels, spec| render(&mut pixels, spec, &mut depth, time))?;

        // 2. A translucent sprite over the frame; re-encode just its cells.
        let sprite = radar(surface.spec(), radar_size, time);
        let x = (surface.spec().width() - sprite.width() - 2) as i32;
        if let Some(damage) = blend_over(sprite.view(), &mut surface.pixels_mut(), x, 2) {
            surface.encode_region(damage)?;
        }

        // 3. Text in cells, over the encoded image.
        let mut cells = surface.cells_mut();
        let mut panel = cells.sub(CellRect::new(
            1,
            1,
            26.min(viewport.width - 1),
            3.min(viewport.height - 1),
        ))?;
        panel.fill(Cell::with_glyph(
            ' ',
            Color::DEFAULT,
            Color::rgb(12, 14, 20),
        ));
        put_str(&mut panel, 1, 0, "encoder", label);
        put_str(&mut panel, 10, 0, encoder_name, value);
        put_str(&mut panel, 1, 1, "frame", label);
        put_str(&mut panel, 10, 1, &format!("{frame}"), value);
        put_str(&mut panel, 1, 2, "bytes", label);
        put_str(&mut panel, 10, 2, &format!("{last_bytes}"), value);
        put_str(
            &mut cells,
            1,
            viewport.height - 1,
            "software 3D + blend_over + put_str",
            TextStyle::new(Color::rgb(150, 160, 170)).with_background(TextBackground::Keep),
        );

        last_bytes = surface.present(&mut stdout)?.bytes;
        stdout.flush()?;
        if !once {
            let next = Duration::from_millis(33) * (frame + 1);
            if let Some(wait) = next.checked_sub(started.elapsed()) {
                thread::sleep(wait);
            }
        }
    }
    writeln!(stdout, "\x1b[{};1H\x1b[?25h", viewport.height + 1)?;
    Ok(())
}

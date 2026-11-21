//! Spins a 3D model in the terminal: `trend-3d` renders it into a TRend surface's pixels at the
//! encoder's resolution and pixel aspect, and the surface encodes and presents only what changed.
//!
//! ```text
//! cargo run --release -p trend-3d --example model_view -- MODEL.glb|gltf|obj|stl|ply
//!     [--encoder half|quadrant|sextant|braille] [--size COLSxROWS] [--frames N] [--once]
//!     [--mode shaded|edges|wire] [--flat] [--z-up]
//! ```
//!
//! Without a model it spins a built-in torus.

use std::env;
use std::error::Error;
use std::f32::consts::TAU;
use std::io::{self, Write};
use std::thread;
use std::time::{Duration, Instant};

use trend::text::{TextBackground, TextStyle, put_str};
use trend::{
    Braille, CellEncoder, Color, HalfBlock, OutputOptions, Quadrant, Sextant, Surface,
    TerminalCapabilities, TerminalColorMode, Viewport,
};
use trend_3d::{
    Mesh, ModelLimits, OrbitCamera, RenderMode, RenderStyle, Renderer, Shading, UpAxis,
};

/// A torus with a colour gradient, so the example runs without a model file.
fn torus() -> Mesh {
    let (rings, sides) = (48_u32, 24_u32);
    let mut mesh = Mesh::default();
    for ring in 0..rings {
        let u = ring as f32 / rings as f32 * TAU;
        for side in 0..sides {
            let v = side as f32 / sides as f32 * TAU;
            let radius = 1.0 + 0.38 * v.cos();
            mesh.positions
                .push([radius * u.cos(), 0.38 * v.sin(), radius * u.sin()]);
            mesh.vertex_colors.push([
                (128.0 + 127.0 * u.cos()) as u8,
                (128.0 + 127.0 * (u + 2.1).cos()) as u8,
                (128.0 + 127.0 * (u + 4.2).cos()) as u8,
                255,
            ]);
        }
    }
    for ring in 0..rings {
        for side in 0..sides {
            let index = |r: u32, s: u32| (r % rings) * sides + s % sides;
            let (a, b) = (index(ring, side), index(ring + 1, side));
            let (c, d) = (index(ring + 1, side + 1), index(ring, side + 1));
            mesh.triangles.push([a, c, b]);
            mesh.triangles.push([a, d, c]);
        }
    }
    mesh.ensure_normals();
    mesh
}

fn main() -> Result<(), Box<dyn Error>> {
    let mut viewport = Viewport {
        width: 80,
        height: 24,
    };
    let (mut frames, mut once, mut up) = (300_u32, false, None);
    let mut encoder: (Box<dyn CellEncoder>, &str) = (Box::new(HalfBlock), "half block");
    let mut style = RenderStyle::default();
    let mut path = None;
    let mut arguments = env::args().skip(1);
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--encoder" => {
                encoder = match arguments.next().as_deref() {
                    Some("half") => (Box::new(HalfBlock), "half block"),
                    Some("quadrant") => (Box::new(Quadrant), "quadrant"),
                    Some("sextant") => (Box::new(Sextant), "sextant"),
                    Some("braille") => (Box::new(Braille), "braille"),
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
            "--frames" => frames = arguments.next().ok_or("--frames expects N")?.parse()?,
            "--once" => once = true,
            "--flat" => style.shading = Shading::Flat,
            "--z-up" => up = Some(UpAxis::Z),
            "--mode" => {
                style.mode = match arguments.next().as_deref() {
                    Some("shaded") => RenderMode::Shaded,
                    Some("edges") => RenderMode::ShadedEdges,
                    Some("wire") => RenderMode::Wireframe,
                    _ => return Err("--mode expects shaded, edges or wire".into()),
                }
            }
            other if !other.starts_with("--") && path.is_none() => path = Some(other.to_owned()),
            other => return Err(format!("unknown argument: {other}").into()),
        }
    }

    let loaded = Instant::now();
    let (mesh, name) = match &path {
        Some(path) => (
            trend_3d::load(path.as_ref(), ModelLimits::default())?,
            path.rsplit(['/', '\\']).next().unwrap_or(path).to_owned(),
        ),
        None => (torus(), "built-in torus".to_owned()),
    };
    let load_time = loaded.elapsed();
    // STL comes from CAD and 3D printing, which stand models on Z.
    let up = up.unwrap_or(
        if path
            .as_deref()
            .is_some_and(|path| path.to_ascii_lowercase().ends_with(".stl"))
        {
            UpAxis::Z
        } else {
            UpAxis::Y
        },
    );
    let mut camera = OrbitCamera::framing_mesh(&mesh, up).ok_or("model has no geometry")?;

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
    let threads = thread::available_parallelism()
        .map_or(1, |count| count.get())
        .min(8);
    let mut renderer = Renderer::with_threads(threads);
    let label = TextStyle::new(Color::rgb(220, 230, 240)).with_background(TextBackground::Keep);
    let stats = mesh.stats();

    let mut stdout = io::stdout().lock();
    write!(stdout, "\x1b[2J\x1b[?25l")?;
    let frames = if once { 1 } else { frames.max(1) };
    let started = Instant::now();
    for frame in 0..frames {
        if !once && frame > 0 {
            camera.orbit(0.03, 0.0);
        }
        let render_started = Instant::now();
        surface.draw(|pixels, spec| {
            renderer.render(&mesh, &camera, &style, pixels, spec.pixel_aspect())
        })?;
        let render_time = render_started.elapsed();
        put_str(
            &mut surface.cells_mut(),
            1,
            0,
            &format!(
                "{name}  {} triangles  {encoder_name}  load {:.0} ms  frame {:.1} ms",
                stats.triangles,
                load_time.as_secs_f64() * 1000.0,
                render_time.as_secs_f64() * 1000.0
            ),
            label,
        );
        surface.present(&mut stdout)?;
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

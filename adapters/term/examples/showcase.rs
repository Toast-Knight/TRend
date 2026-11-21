//! TRend's optional crates together: a golden ring drawn by `trend-3d` under a sky it reflects,
//! a title drawn by `trend-text`, and the terminal handled by `trend-term`.
//!
//! ```text
//! cargo run --release -p trend-term --example showcase
//! ```
//!
//! Arrow keys or dragging with the mouse turn the ring, the wheel zooms, E changes the
//! environment, Tab the cell encoder, Esc or Q quits.

use std::f32::consts::TAU;
use std::sync::Arc;
use std::time::{Duration, Instant};

use trend::{CellEncoder, HalfBlock, Quadrant, Sextant, Surface};
use trend_3d::{
    Background, Environment, EnvironmentPreset, Material, Mesh, OrbitCamera, RenderStyle, Renderer,
    UpAxis,
};
use trend_term::{Event, Key, KeyState, MouseAction, Session, SessionOptions};
use trend_text::{Font, TextStyle, draw_text};

/// A ring (torus) with one shiny gold material.
fn ring() -> Mesh {
    let (rings, sides) = (64_u32, 28_u32);
    let mut mesh = Mesh::default();
    for ring in 0..rings {
        let u = ring as f32 / rings as f32 * TAU;
        for side in 0..sides {
            let v = side as f32 / sides as f32 * TAU;
            let radius = 1.0 + 0.4 * v.cos();
            mesh.positions
                .push([radius * u.cos(), 0.4 * v.sin(), radius * u.sin()]);
        }
    }
    for ring in 0..rings {
        for side in 0..sides {
            let index = |r: u32, s: u32| (r % rings) * sides + s % sides;
            let (a, b) = (index(ring, side), index(ring + 1, side));
            let (c, d) = (index(ring + 1, side + 1), index(ring, side + 1));
            mesh.triangles.extend([[a, c, b], [a, d, c]]);
        }
    }
    mesh.triangle_materials = vec![0; mesh.triangles.len()];
    mesh.materials = vec![Material {
        color: [255, 196, 86, 255],
        metallic: 1.0,
        roughness: 0.22,
        ..Material::new("gold")
    }];
    mesh.ensure_normals();
    mesh
}

fn encoder(index: usize) -> (Box<dyn CellEncoder>, &'static str) {
    match index % 3 {
        0 => (Box::new(HalfBlock), "half block"),
        1 => (Box::new(Quadrant), "quadrant"),
        _ => (Box::new(Sextant), "sextant"),
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut session = Session::enter(SessionOptions {
        title: Some("TRend showcase".into()),
        mouse: true,
        ..SessionOptions::default()
    })?;
    let info = session.info();
    let (mut encoder_index, mut preset_index) = (0, 1);
    let mut surface = Surface::new(encoder(encoder_index).0, info.viewport, info.capabilities)?;
    surface.set_cell_aspect(info.cell_aspect())?;
    surface.set_output_options(info.recommended_output_options());
    let mesh = ring();
    let mut camera = OrbitCamera::framing_mesh(&mesh, UpAxis::Y).ok_or("empty mesh")?;
    let mut renderer = Renderer::with_threads(4);
    let font = Font::pixel();
    let mut style = RenderStyle {
        background: Background::Environment { blur: 0.25 },
        ..RenderStyle::default()
    };
    let mut last = Instant::now();
    // Where the mouse was when last seen with a button held.
    let mut grabbed: Option<(u16, u16)> = None;
    loop {
        while let Some(event) = session.poll(Duration::ZERO)? {
            match event {
                Event::Resize(viewport) => surface.resize(viewport)?,
                Event::Key {
                    key,
                    state: KeyState::Press,
                    ..
                } => match key {
                    Key::Esc | Key::Char('q') => return Ok(()),
                    Key::Char('e') => preset_index += 1,
                    Key::Tab => {
                        encoder_index += 1;
                        surface.set_encoder(encoder(encoder_index).0)?;
                    }
                    _ => {}
                },
                Event::Mouse {
                    action,
                    column,
                    row,
                    ..
                } => match action {
                    MouseAction::Down(_) => grabbed = Some((column, row)),
                    MouseAction::Drag(_) => {
                        let (from_column, from_row) = grabbed.unwrap_or((column, row));
                        // A cell is about twice as tall as wide, so a row turns twice as far.
                        camera.orbit(
                            (f32::from(column) - f32::from(from_column)) * -0.05,
                            (f32::from(row) - f32::from(from_row)) * 0.1,
                        );
                        grabbed = Some((column, row));
                    }
                    MouseAction::Up(_) => grabbed = None,
                    MouseAction::ScrollUp => camera.zoom(0.9),
                    MouseAction::ScrollDown => camera.zoom(1.1),
                    MouseAction::Moved => {}
                },
                _ => {}
            }
        }
        let seconds = last.elapsed().as_secs_f32();
        last = Instant::now();
        let turn = |positive: Key, negative: Key| {
            f32::from(session.is_down(positive)) - f32::from(session.is_down(negative))
        };
        // The ring spins slowly on its own unless the mouse holds it; held arrow keys turn it
        // faster or tilt it.
        let spin = if grabbed.is_some() { 0.0 } else { 0.5 };
        camera.orbit(
            (spin + 2.0 * turn(Key::Right, Key::Left)) * seconds,
            1.5 * turn(Key::Up, Key::Down) * seconds,
        );
        let preset = EnvironmentPreset::ALL[preset_index % EnvironmentPreset::ALL.len()];
        if style
            .environment
            .as_ref()
            .is_none_or(|current| current.name() != preset.label())
        {
            style.environment = Some(Arc::new(Environment::preset(preset)));
        }
        let label = format!("{} · {}", preset.label(), encoder(encoder_index).1);
        surface.draw(|mut pixels, spec| {
            renderer.render(
                &mesh,
                &camera,
                &style,
                pixels.reborrow(),
                spec.pixel_aspect(),
            );
            // Scaled text: a title up to a third of the picture tall, and a smaller caption.
            let (width, height) = (pixels.width() as f32, pixels.height() as f32);
            let title = TextStyle::new(font.fit("TRend", width * 0.5, height * 0.3), [255; 4]);
            draw_text(&mut pixels, 3.0, 2.0, "TRend", &font, &title);
            let caption = TextStyle::new(10.0, [230, 235, 240, 255]);
            draw_text(&mut pixels, 3.0, height - 12.0, &label, &font, &caption);
        })?;
        session.write_frame(surface.present_bytes())?;
        std::thread::sleep(Duration::from_millis(16).saturating_sub(last.elapsed()));
    }
}

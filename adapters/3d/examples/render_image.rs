//! Renders a model (or a built-in material test) to a PPM image, for looking at lighting:
//!
//! ```text
//! cargo run --release -p trend-3d --example render_image -- MODEL|--spheres OUT.ppm
//!     [--size WxH] [--environment studio|sky|sunset|night|neon|overcast|none] [--sky]
//!     [--yaw RADIANS] [--exposure X]
//! ```
//!
//! `--spheres` draws five metal and five plastic spheres from mirror-smooth to fully rough, plus a
//! glowing one. `--sky` shows the environment behind the model.

use std::error::Error;
use std::f32::consts::{PI, TAU};
use std::io::Write;
use std::sync::Arc;

use trend::{PixelAspect, PixelBuffer};
use trend_3d::{
    Background, Environment, EnvironmentPreset, Material, Mesh, ModelLimits, OrbitCamera,
    RenderStyle, Renderer, UpAxis,
};

fn sphere(mesh: &mut Mesh, center: [f32; 3], radius: f32, material: u32) {
    let (rings, segments) = (24_u32, 48_u32);
    let base = mesh.positions.len() as u32;
    for ring in 0..=rings {
        let theta = ring as f32 / rings as f32 * PI;
        for segment in 0..=segments {
            let phi = segment as f32 / segments as f32 * TAU;
            let direction = [
                theta.sin() * phi.cos(),
                theta.cos(),
                theta.sin() * phi.sin(),
            ];
            mesh.positions.push(std::array::from_fn(|axis| {
                center[axis] + direction[axis] * radius
            }));
            mesh.uvs
                .push([segment as f32 / segments as f32, ring as f32 / rings as f32]);
        }
    }
    let row = segments + 1;
    for ring in 0..rings {
        for segment in 0..segments {
            let a = base + ring * row + segment;
            let (b, c, d) = (a + row, a + row + 1, a + 1);
            for triangle in [[a, d, c], [a, c, b]] {
                mesh.triangles.push(triangle);
                mesh.triangle_materials.push(material);
            }
        }
    }
}

fn spheres() -> Mesh {
    let mut mesh = Mesh::default();
    for (row, metallic) in [(0, 1.0_f32), (1, 0.0)] {
        for column in 0..5 {
            let roughness = 0.05 + column as f32 * 0.23;
            mesh.materials.push(Material {
                color: if metallic > 0.5 {
                    [255, 215, 150, 255]
                } else {
                    [200, 40, 40, 255]
                },
                metallic,
                roughness,
                ..Material::new(format!("m{metallic} r{roughness:.2}"))
            });
            let material = mesh.materials.len() as u32 - 1;
            sphere(
                &mut mesh,
                [column as f32 * 2.2 - 4.4, 1.1 - row as f32 * 2.2, 0.0],
                1.0,
                material,
            );
        }
    }
    mesh.materials.push(Material {
        color: [20, 20, 20, 255],
        emissive: [0.2, 1.2, 3.0],
        ..Material::new("glow")
    });
    let glow = mesh.materials.len() as u32 - 1;
    sphere(&mut mesh, [0.0, -3.3, 0.0], 0.8, glow);
    mesh.ensure_normals();
    mesh.ensure_tangents();
    mesh
}

fn main() -> Result<(), Box<dyn Error>> {
    let mut arguments = std::env::args().skip(1);
    let model = arguments
        .next()
        .ok_or("usage: render_image MODEL|--spheres OUT.ppm [...]")?;
    let out = arguments
        .next()
        .ok_or("usage: render_image MODEL|--spheres OUT.ppm [...]")?;
    let (mut width, mut height) = (480_u32, 320_u32);
    let mut style = RenderStyle {
        environment: Some(Arc::new(Environment::preset(EnvironmentPreset::Studio))),
        ..RenderStyle::default()
    };
    let mut yaw = None;
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--size" => {
                let value = arguments.next().ok_or("--size WxH")?;
                let (w, h) = value.split_once('x').ok_or("--size WxH")?;
                (width, height) = (w.parse()?, h.parse()?);
            }
            "--environment" => {
                let name = arguments.next().ok_or("--environment NAME")?;
                style.environment = EnvironmentPreset::ALL
                    .into_iter()
                    .find(|preset| preset.label().eq_ignore_ascii_case(&name))
                    .map(|preset| Arc::new(Environment::preset(preset)));
            }
            "--sky" => style.background = Background::Environment { blur: 0.0 },
            "--yaw" => yaw = Some(arguments.next().ok_or("--yaw RADIANS")?.parse()?),
            "--exposure" => style.exposure = arguments.next().ok_or("--exposure X")?.parse()?,
            other => return Err(format!("unknown argument {other}").into()),
        }
    }
    let (mesh, up) = if model == "--spheres" {
        (spheres(), UpAxis::Y)
    } else {
        let path = std::path::Path::new(&model);
        let up = if model.to_ascii_lowercase().ends_with(".stl") {
            UpAxis::Z
        } else {
            UpAxis::Y
        };
        (trend_3d::load(path, ModelLimits::default())?, up)
    };
    let mut camera = OrbitCamera::framing_mesh(&mesh, up).ok_or("empty model")?;
    if model == "--spheres" {
        camera.yaw = 0.0;
        camera.pitch = 0.0;
    }
    if let Some(yaw) = yaw {
        camera.yaw = yaw;
    }
    let mut buffer = PixelBuffer::new(width, height)?;
    let started = std::time::Instant::now();
    Renderer::with_threads(4).render(
        &mesh,
        &camera,
        &style,
        buffer.view_mut(),
        PixelAspect {
            width: 1,
            height: 1,
        },
    );
    eprintln!(
        "rendered in {:.1} ms",
        started.elapsed().as_secs_f64() * 1000.0
    );
    let mut file = std::fs::File::create(out)?;
    write!(file, "P6\n{width} {height}\n255\n")?;
    for pixel in buffer.view().pixels().chunks_exact(4) {
        file.write_all(&pixel[..3])?;
    }
    Ok(())
}

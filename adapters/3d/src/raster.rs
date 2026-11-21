//! The software rasterizer.
//!
//! Two passes per frame, both split into row bands over threads:
//!
//! 1. **Visibility:** every triangle is set up once (culled, clipped at the near plane, projected
//!    with the encoder's pixel aspect), then rasterized into a per-pixel buffer that keeps only the
//!    nearest triangle and where on it the pixel lies. Cut-outs and see-through materials are
//!    tested here, so hidden surfaces are never shaded.
//! 2. **Shading:** each pixel is lit once: physically based (metallic/roughness), with normal maps,
//!    emissive light, a coloured key light, and the environment for soft light and reflections.
//!    Lighting is computed in linear light, tone-mapped, and written as sRGB.

use std::sync::Arc;

use trend::{PixelAspect, PixelViewMut};

use crate::camera::{OrbitCamera, UpAxis, ViewBasis};
use crate::color::{linear_to_srgb, srgb_float_to_linear, srgb_to_linear, tone_map};
use crate::environment::Environment;
use crate::math::{Vec3, add, cross, dot, lerp, normalize_or, scale, sub};
use crate::mesh::{Material, Mesh};

/// How surface normals are chosen.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Shading {
    /// Normals interpolated across each triangle, plus normal maps.
    #[default]
    Smooth,
    /// One normal per triangle: facets show.
    Flat,
}

/// What is drawn.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum RenderMode {
    /// Lit surfaces.
    #[default]
    Shaded,
    /// Lit surfaces with triangle edges on top.
    ShadedEdges,
    /// Hidden-line wireframe: dim surfaces, visible edges only.
    Wireframe,
}

impl RenderMode {
    pub const ALL: [Self; 3] = [Self::Shaded, Self::ShadedEdges, Self::Wireframe];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Shaded => "Shaded",
            Self::ShadedEdges => "Shaded + edges",
            Self::Wireframe => "Wireframe",
        }
    }
}

/// Distance fog: surfaces fade into `color` between `start` and `end` (distances from the eye, in
/// model units), hiding where a large scene ends.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fog {
    /// sRGB colour, usually the sky's colour at the horizon. Ignored with `into_background`.
    pub color: [u8; 3],
    pub start: f32,
    pub end: f32,
    /// Fade into the background behind each pixel instead of `color`, so far surfaces melt into
    /// the sky box (or gradient) seamlessly.
    pub into_background: bool,
}

/// What fills pixels no surface covers.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Background {
    /// Alpha 0, for compositing (terminal default colour after encoding).
    Transparent,
    Solid([u8; 3]),
    /// Top colour to bottom colour.
    Gradient {
        top: [u8; 3],
        bottom: [u8; 3],
    },
    /// The environment itself, as a sky box around the model; `blur` 0 (sharp) to 1.
    Environment {
        blur: f32,
    },
}

/// The key light, placed relative to the camera so the model stays lit as it turns.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Light {
    /// sRGB colour.
    pub color: [u8; 3],
    /// 0 turns it off; 3 is bright daylight.
    pub intensity: f32,
    /// Degrees: 0 = from the camera, positive = from the right.
    pub azimuth: f32,
    /// Degrees above the camera's horizon.
    pub elevation: f32,
}

impl Default for Light {
    fn default() -> Self {
        Self {
            color: [255, 250, 240],
            intensity: 2.2,
            azimuth: -35.0,
            elevation: 40.0,
        }
    }
}

impl Light {
    /// Unit vector toward the light, in view space.
    fn direction(&self) -> Vec3 {
        let (azimuth, elevation) = (self.azimuth.to_radians(), self.elevation.to_radians());
        normalize_or(
            [
                azimuth.sin() * elevation.cos(),
                elevation.sin(),
                -azimuth.cos() * elevation.cos(),
            ],
            [0.0, 0.0, -1.0],
        )
    }
}

/// How a model is drawn.
#[derive(Clone, Debug, PartialEq)]
pub struct RenderStyle {
    pub mode: RenderMode,
    pub shading: Shading,
    /// sRGB colour of surfaces without colours or materials of their own.
    pub base_color: [u8; 3],
    pub edge_color: [u8; 3],
    /// Line colour in [`RenderMode::Wireframe`].
    pub wire_color: [u8; 3],
    /// Edge line width in pixels.
    pub edge_width: f32,
    pub background: Background,
    /// Multiplies all light; above 1 brightens dark models.
    pub exposure: f32,
    /// Skips triangles facing away. Faster for closed models; leave off for open or badly wound ones,
    /// which are then lit on both sides.
    pub cull_back_faces: bool,
    pub light: Light,
    /// The world around the model: soft light and reflections. Without one, a plain grey sky and
    /// floor light the model.
    pub environment: Option<Arc<Environment>>,
    /// Multiplies the environment's light.
    pub environment_strength: f32,
    /// Turns the environment around the up axis, in radians.
    pub environment_rotation: f32,
    /// Distance fog, off by default.
    pub fog: Option<Fog>,
}

impl Default for RenderStyle {
    fn default() -> Self {
        Self {
            mode: RenderMode::Shaded,
            shading: Shading::Smooth,
            base_color: [190, 196, 204],
            edge_color: [20, 24, 30],
            wire_color: [120, 200, 255],
            edge_width: 1.2,
            background: Background::Gradient {
                top: [52, 58, 66],
                bottom: [22, 24, 28],
            },
            exposure: 1.0,
            cull_back_faces: false,
            light: Light::default(),
            environment: None,
            environment_strength: 1.0,
            environment_rotation: 0.0,
            fog: None,
        }
    }
}

/// What one frame drew.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct FrameStats {
    /// Triangles that reached the rasterizer (after culling and clipping).
    pub triangles: usize,
    pub culled: usize,
    /// Triangles entirely behind the near plane, off screen, or between pixel centres.
    pub clipped: usize,
}

/// Everything interpolated across a triangle, at one corner.
#[derive(Clone, Copy, Debug, Default)]
struct Corner {
    /// View-space position.
    view: Vec3,
    /// View-space normal.
    normal: Vec3,
    /// View-space tangent and handedness.
    tangent: [f32; 4],
    uv: [f32; 2],
    /// Linear RGB and alpha.
    color: [f32; 4],
}

impl Corner {
    fn lerp(&self, other: &Self, t: f32) -> Self {
        let mix4 = |a: [f32; 4], b: [f32; 4]| std::array::from_fn(|i| a[i] + (b[i] - a[i]) * t);
        Self {
            view: lerp(self.view, other.view, t),
            normal: lerp(self.normal, other.normal, t),
            tangent: mix4(self.tangent, other.tangent),
            uv: [
                self.uv[0] + (other.uv[0] - self.uv[0]) * t,
                self.uv[1] + (other.uv[1] - self.uv[1]) * t,
            ],
            color: mix4(self.color, other.color),
        }
    }
}

/// A triangle ready to rasterize and shade.
#[derive(Clone, Copy, Debug)]
struct Prepared {
    /// x, y in pixels; 1/z.
    screen: [[f32; 3]; 3],
    corners: [Corner; 3],
    /// View-space normal of the flat triangle.
    face_normal: Vec3,
    /// Index into the mesh's materials, or `NO_MATERIAL`.
    material: u32,
    min_y: u32,
    max_y: u32,
}

const NO_MATERIAL: u32 = u32::MAX;
const NO_TRIANGLE: u32 = u32::MAX;

/// What the visibility pass found at each pixel of a band.
struct Visibility {
    depth: Vec<f32>,
    triangle: Vec<u32>,
    /// Perspective-correct barycentric weights of corners 1 and 2.
    weights: Vec<[f32; 2]>,
    /// Edge line coverage 0..255.
    edge: Vec<u8>,
}

impl Visibility {
    fn reset(&mut self, pixels: usize) {
        self.depth.clear();
        self.depth.resize(pixels, 0.0);
        self.triangle.clear();
        self.triangle.resize(pixels, NO_TRIANGLE);
        self.weights.clear();
        self.weights.resize(pixels, [0.0; 2]);
        self.edge.clear();
        self.edge.resize(pixels, 0);
    }
}

/// Renders meshes. Keep one and reuse it: it holds per-frame buffers.
pub struct Renderer {
    threads: usize,
    view_positions: Vec<Vec3>,
    /// Prepared triangles, one list per setup thread.
    prepared: Vec<Vec<Prepared>>,
    bands: Vec<Visibility>,
}

impl std::fmt::Debug for Renderer {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Renderer")
            .field("threads", &self.threads)
            .finish()
    }
}

impl Default for Renderer {
    fn default() -> Self {
        Self::new()
    }
}

/// Read-only frame data shared by every thread.
struct Frame<'a> {
    mesh: &'a Mesh,
    style: &'a RenderStyle,
    basis: ViewBasis,
    up: UpAxis,
    view_positions: &'a [Vec3],
    near: f32,
    focal: [f32; 2],
    center: [f32; 2],
    size: [f32; 2],
    has_normals: bool,
    has_tangents: bool,
    has_uvs: bool,
    triangle_colors: bool,
    vertex_colors: bool,
}

impl Renderer {
    /// A single-threaded renderer.
    pub const fn new() -> Self {
        Self {
            threads: 1,
            view_positions: Vec::new(),
            prepared: Vec::new(),
            bands: Vec::new(),
        }
    }

    /// Splits the work over up to `threads` threads.
    pub fn with_threads(threads: usize) -> Self {
        Self {
            threads: threads.clamp(1, 64),
            ..Self::new()
        }
    }

    /// Draws `mesh` as seen by `camera` into `target`, whose pixels have the physical shape
    /// `pixel_aspect` (from `RasterSpec::pixel_aspect`). Every pixel of `target` is written.
    pub fn render(
        &mut self,
        mesh: &Mesh,
        camera: &OrbitCamera,
        style: &RenderStyle,
        target: PixelViewMut<'_>,
        pixel_aspect: PixelAspect,
    ) -> FrameStats {
        let width = target.width();
        let height = target.height();
        let mut stats = FrameStats::default();
        if width == 0 || height == 0 {
            return stats;
        }

        // 1. Vertices into view space.
        let basis = camera.basis();
        self.view_positions.clear();
        self.view_positions
            .extend(mesh.positions.iter().map(|&point| basis.to_view(point)));

        // 2. Triangle setup, in parallel chunks.
        let focal_y = height as f32 * 0.5 / (camera.fov_y * 0.5).tan();
        // A pixel `pixel_width` times as wide as tall covers more of the scene horizontally.
        let pixel_width = pixel_aspect.width as f32 / pixel_aspect.height.max(1) as f32;
        let frame = Frame {
            mesh,
            style,
            basis,
            up: camera.up,
            view_positions: &self.view_positions,
            near: camera.near(),
            focal: [focal_y / pixel_width, focal_y],
            center: [width as f32 * 0.5, height as f32 * 0.5],
            size: [width as f32, height as f32],
            has_normals: mesh.corner_normals.len() == mesh.triangles.len() * 3,
            has_tangents: mesh.corner_tangents.len() == mesh.triangles.len() * 3,
            has_uvs: mesh.uvs.len() == mesh.positions.len(),
            triangle_colors: mesh.triangle_colors.len() == mesh.triangles.len(),
            vertex_colors: mesh.vertex_colors.len() == mesh.positions.len(),
        };
        let setup_threads = if mesh.triangles.len() >= 20_000 {
            self.threads
        } else {
            1
        };
        self.prepared.resize_with(setup_threads, Vec::new);
        self.prepared.truncate(setup_threads);
        let chunk = mesh.triangles.len().div_ceil(setup_threads).max(1);
        if setup_threads == 1 {
            prepare_chunk(
                &frame,
                0,
                &mesh.triangles,
                &mut self.prepared[0],
                &mut stats,
            );
        } else {
            let frame = &frame;
            let parts = std::thread::scope(|scope| {
                let handles: Vec<_> = self
                    .prepared
                    .iter_mut()
                    .zip(mesh.triangles.chunks(chunk))
                    .enumerate()
                    .map(|(index, (out, triangles))| {
                        scope.spawn(move || {
                            let mut stats = FrameStats::default();
                            prepare_chunk(frame, index * chunk, triangles, out, &mut stats);
                            stats
                        })
                    })
                    .collect();
                handles
                    .into_iter()
                    .map(|handle| handle.join().expect("setup thread panicked"))
                    .collect::<Vec<_>>()
            });
            for part in parts {
                stats.culled += part.culled;
                stats.clipped += part.clipped;
            }
        }
        stats.triangles = self.prepared.iter().map(Vec::len).sum();

        // 3. Visibility and shading, in parallel row bands.
        let pixels = width as usize * height as usize;
        let threads = if pixels >= 16_384 { self.threads } else { 1 };
        let rows_per_band = height.div_ceil(threads as u32).max(1);
        let band_count = height.div_ceil(rows_per_band) as usize;
        self.bands.resize_with(band_count, || Visibility {
            depth: Vec::new(),
            triangle: Vec::new(),
            weights: Vec::new(),
            edge: Vec::new(),
        });
        let prepared = &self.prepared;
        let frame = &frame;
        if band_count == 1 {
            draw_band(frame, prepared, target, 0, height, &mut self.bands[0]);
            return stats;
        }
        std::thread::scope(|scope| {
            for ((first_row, band), visibility) in target
                .into_row_bands(rows_per_band)
                .zip(self.bands.iter_mut())
            {
                scope
                    .spawn(move || draw_band(frame, prepared, band, first_row, height, visibility));
            }
        });
        stats
    }
}

/// sRGB colour bytes to linear RGBA.
fn linear_color(color: [u8; 4]) -> [f32; 4] {
    [
        srgb_to_linear(color[0]),
        srgb_to_linear(color[1]),
        srgb_to_linear(color[2]),
        f32::from(color[3]) / 255.0,
    ]
}

/// Culls, clips and projects `triangles` (which start at `first_index` in the mesh).
fn prepare_chunk(
    frame: &Frame<'_>,
    first_index: usize,
    triangles: &[[u32; 3]],
    out: &mut Vec<Prepared>,
    stats: &mut FrameStats,
) {
    out.clear();
    let Frame {
        mesh,
        style,
        basis,
        view_positions,
        near,
        ..
    } = *frame;
    let wire = style.mode == RenderMode::Wireframe;
    let wire_fill = linear_color(wire_surface(style.background));
    let default_color = {
        let [r, g, b] = style.base_color;
        linear_color([r, g, b, 255])
    };
    let project = |[x, y, z]: Vec3| {
        [
            frame.center[0] + x / z * frame.focal[0],
            frame.center[1] - y / z * frame.focal[1],
            1.0 / z,
        ]
    };
    for (offset, triangle) in triangles.iter().enumerate() {
        let index = first_index + offset;
        let points = triangle.map(|vertex| view_positions[vertex as usize]);
        // View space (x right, y up, z forward) is left-handed, so the operands are swapped to get
        // the outward normal of a counter-clockwise (right-handed) face.
        let face = cross(sub(points[2], points[0]), sub(points[1], points[0]));
        // Seen from the eye (the view-space origin), a front face's normal points back at it.
        if dot(face, points[0]) >= 0.0 && style.cull_back_faces {
            stats.culled += 1;
            continue;
        }
        // Fast path: wholly in front of the near plane. Reject triangles that are off screen or
        // fall between pixel centres before doing any more work; dense models have many.
        if points.iter().all(|point| point[2] >= near)
            && !covers_a_pixel(&points.map(project), frame.size)
        {
            stats.clipped += 1;
            continue;
        }

        let face_normal = normalize_or(face, [0.0, 0.0, -1.0]);
        let material_index = mesh
            .triangle_materials
            .get(index)
            .copied()
            .filter(|&material| (material as usize) < mesh.materials.len() && !wire)
            .unwrap_or(NO_MATERIAL);
        let material = mesh.materials.get(material_index as usize);
        let mut corners = [Corner::default(); 3];
        for (corner, slot) in corners.iter_mut().enumerate() {
            let vertex = triangle[corner] as usize;
            let normal = if frame.has_normals {
                normalize_or(
                    basis.direction_to_view(mesh.corner_normals[index * 3 + corner]),
                    face_normal,
                )
            } else {
                face_normal
            };
            let tangent = if frame.has_tangents {
                let [x, y, z, w] = mesh.corner_tangents[index * 3 + corner];
                let [x, y, z] = basis.direction_to_view([x, y, z]);
                [x, y, z, w]
            } else {
                [0.0; 4]
            };
            // Colour: per-triangle or per-vertex colour, times the material's colour.
            let own = if wire {
                Some(wire_fill)
            } else if frame.triangle_colors {
                Some(linear_color(mesh.triangle_colors[index]))
            } else if frame.vertex_colors {
                Some(linear_color(mesh.vertex_colors[vertex]))
            } else {
                None
            };
            let color = match (own, material) {
                (Some(own), Some(material)) => {
                    let factor = linear_color(material.color);
                    std::array::from_fn(|channel| own[channel] * factor[channel])
                }
                (Some(own), None) => own,
                (None, Some(material)) => linear_color(material.color),
                (None, None) => default_color,
            };
            *slot = Corner {
                view: points[corner],
                normal,
                tangent,
                uv: if frame.has_uvs {
                    mesh.uvs[vertex]
                } else {
                    [0.0; 2]
                },
                color,
            };
        }

        let visible = clip(&corners, near, |clipped| {
            let screen = clipped.map(|corner| project(corner.view));
            if !covers_a_pixel(&screen, frame.size) {
                return;
            }
            let min_y = screen[0][1].min(screen[1][1]).min(screen[2][1]);
            let max_y = screen[0][1].max(screen[1][1]).max(screen[2][1]);
            out.push(Prepared {
                screen,
                corners: clipped,
                face_normal,
                material: material_index,
                min_y: min_y.max(0.0) as u32,
                max_y: (max_y.ceil().max(0.0) as u32).min(frame.size[1] as u32 - 1),
            });
        });
        if !visible {
            stats.clipped += 1;
        }
    }
}

/// False when a projected triangle's box is off screen or contains no pixel centre (x + 0.5).
fn covers_a_pixel(screen: &[[f32; 3]; 3], size: [f32; 2]) -> bool {
    let min_x = screen[0][0].min(screen[1][0]).min(screen[2][0]);
    let max_x = screen[0][0].max(screen[1][0]).max(screen[2][0]);
    let min_y = screen[0][1].min(screen[1][1]).min(screen[2][1]);
    let max_y = screen[0][1].max(screen[1][1]).max(screen[2][1]);
    if !(max_x >= 0.0 && max_y >= 0.0 && min_x < size[0] && min_y < size[1]) {
        return false;
    }
    // The first pixel centre at or after min must not lie beyond max.
    (min_x - 0.5).ceil() + 0.5 <= max_x && (min_y - 0.5).ceil() + 0.5 <= max_y
}

/// Clips a triangle against the near plane (z = `near`) and hands the 1 or 2 resulting triangles
/// to `emit`. Returns false when nothing is in front of the plane.
fn clip(corners: &[Corner; 3], near: f32, mut emit: impl FnMut([Corner; 3])) -> bool {
    let inside = corners.map(|corner| corner.view[2] >= near);
    match inside.iter().filter(|&&inside| inside).count() {
        3 => {
            emit(*corners);
            true
        }
        0 => false,
        _ => {
            // Sutherland–Hodgman against one plane: at most 4 corners.
            let mut polygon: Vec<Corner> = Vec::with_capacity(4);
            for current in 0..3 {
                let next = (current + 1) % 3;
                let (a, b) = (&corners[current], &corners[next]);
                if inside[current] {
                    polygon.push(*a);
                }
                if inside[current] != inside[next] {
                    let t = (near - a.view[2]) / (b.view[2] - a.view[2]);
                    polygon.push(a.lerp(b, t));
                }
            }
            for index in 1..polygon.len().saturating_sub(1) {
                emit([polygon[0], polygon[index], polygon[index + 1]]);
            }
            true
        }
    }
}

/// The surface colour in wireframe mode: a dim tone of the background, so the model's shape reads
/// where its edges are too dense to draw.
fn wire_surface(background: Background) -> [u8; 4] {
    let average = match background {
        Background::Transparent | Background::Environment { .. } => [24, 26, 30],
        Background::Solid(color) => color,
        Background::Gradient { top, bottom } => std::array::from_fn(|channel| {
            ((u16::from(top[channel]) + u16::from(bottom[channel])) / 2) as u8
        }),
    };
    let [r, g, b] = average.map(|channel| channel.saturating_add(36));
    [r, g, b, 255]
}

/// Edges shorter than this on screen are left out of edge and wireframe drawing.
const MIN_EDGE_PIXELS: f32 = 4.0;

/// 4x4 ordered-dither thresholds, for see-through materials.
const BAYER: [f32; 16] = [
    0.03, 0.53, 0.16, 0.66, 0.78, 0.28, 0.91, 0.41, 0.22, 0.72, 0.09, 0.59, 0.97, 0.47, 0.84, 0.34,
];

/// Visibility then shading for rows `first_row .. first_row + band.height()`.
fn draw_band(
    frame: &Frame<'_>,
    prepared: &[Vec<Prepared>],
    mut band: PixelViewMut<'_>,
    first_row: u32,
    image_height: u32,
    visibility: &mut Visibility,
) {
    let width = band.width();
    let rows = band.height();
    visibility.reset(width as usize * rows as usize);
    let style = frame.style;
    let materials = &frame.mesh.materials;
    let last_row = first_row + rows - 1;
    let edges = style.mode != RenderMode::Shaded;
    let edge_width = style.edge_width.max(0.1);

    let mut base_index = 0_u32;
    for list in prepared {
        for (local, triangle) in list.iter().enumerate() {
            let id = base_index + local as u32;
            if triangle.max_y < first_row || triangle.min_y > last_row {
                continue;
            }
            let [a, b, c] = triangle.screen;
            let area = (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]);
            if area.abs() < 1e-12 {
                continue;
            }
            let inverse_area = 1.0 / area;
            let min_x = a[0].min(b[0]).min(c[0]).max(0.0) as u32;
            let max_x = (a[0].max(b[0]).max(c[0]).ceil().max(0.0) as u32).min(width - 1);
            let y_start = triangle.min_y.max(first_row);
            let y_end = triangle.max_y.min(last_row);
            // Distance to each edge = edge function / edge length; short edges are not drawn.
            let edge_length = |p: [f32; 3], q: [f32; 3]| (q[0] - p[0]).hypot(q[1] - p[1]);
            let inverse_lengths =
                [edge_length(b, c), edge_length(c, a), edge_length(a, b)].map(|length| {
                    if length >= MIN_EDGE_PIXELS {
                        1.0 / length
                    } else {
                        f32::INFINITY
                    }
                });
            let material = (triangle.material != NO_MATERIAL)
                .then(|| &materials[triangle.material as usize])
                .filter(|material| material.uses_alpha());

            for y in y_start..=y_end {
                let py = y as f32 + 0.5;
                let row = ((y - first_row) * width) as usize;
                for x in min_x..=max_x {
                    let px = x as f32 + 0.5;
                    let w0 = (c[0] - b[0]) * (py - b[1]) - (c[1] - b[1]) * (px - b[0]);
                    let w1 = (a[0] - c[0]) * (py - c[1]) - (a[1] - c[1]) * (px - c[0]);
                    let w2 = (b[0] - a[0]) * (py - a[1]) - (b[1] - a[1]) * (px - a[0]);
                    let (b0, b1, b2) = (w0 * inverse_area, w1 * inverse_area, w2 * inverse_area);
                    if b0 < 0.0 || b1 < 0.0 || b2 < 0.0 {
                        continue;
                    }
                    let inverse_z = b0 * a[2] + b1 * b[2] + b2 * c[2];
                    let slot = row + x as usize;
                    if inverse_z <= visibility.depth[slot] {
                        continue;
                    }
                    // Perspective-correct weights.
                    let (p1, p2) = (b1 * b[2] / inverse_z, b2 * c[2] / inverse_z);
                    if let Some(material) = material {
                        let weights = [1.0 - p1 - p2, p1, p2];
                        let corners = &triangle.corners;
                        let mut alpha = interpolate(weights, |corner| corners[corner].color[3]);
                        if let Some(texture) = &material.texture {
                            let u = interpolate(weights, |corner| corners[corner].uv[0]);
                            let v = interpolate(weights, |corner| corners[corner].uv[1]);
                            alpha *= material.sample(texture, u, v)[3] / 255.0;
                        }
                        let threshold = if material.alpha_blend {
                            BAYER[((y & 3) * 4 + (x & 3)) as usize]
                        } else {
                            f32::from(material.alpha_test.unwrap_or(128)) / 255.0
                        };
                        if alpha < threshold {
                            continue;
                        }
                    }
                    visibility.depth[slot] = inverse_z;
                    visibility.triangle[slot] = id;
                    visibility.weights[slot] = [p1, p2];
                    visibility.edge[slot] = if edges {
                        let distance = (w0.abs() * inverse_lengths[0])
                            .min(w1.abs() * inverse_lengths[1])
                            .min(w2.abs() * inverse_lengths[2]);
                        ((edge_width * 0.5 - distance + 0.5).clamp(0.0, 1.0) * 255.0) as u8
                    } else {
                        0
                    };
                }
            }
        }
        base_index += list.len() as u32;
    }

    // Shading.
    let flat: Vec<&Prepared> = prepared.iter().flatten().collect();
    let shader = Shader::new(frame);
    let edge_color = if style.mode == RenderMode::Wireframe {
        style.wire_color
    } else {
        style.edge_color
    }
    .map(f32::from);
    for y in 0..rows {
        let row = band.row_mut(y);
        for x in 0..width {
            let slot = (y * width + x) as usize;
            let pixel = &mut row[x as usize * 4..x as usize * 4 + 4];
            let id = visibility.triangle[slot];
            if id == NO_TRIANGLE {
                pixel.copy_from_slice(&shader.background(x, first_row + y, image_height));
                continue;
            }
            let triangle = flat[id as usize];
            let [p1, p2] = visibility.weights[slot];
            let weights = [1.0 - p1 - p2, p1, p2];
            let mut color = shader.shade(triangle, weights).map(f32::from);
            if let Some(fog) = style.fog {
                let corners = &triangle.corners;
                let at = |axis: usize| interpolate(weights, |corner| corners[corner].view[axis]);
                let distance = (at(0).powi(2) + at(1).powi(2) + at(2).powi(2)).sqrt();
                let amount =
                    ((distance - fog.start) / (fog.end - fog.start).max(1e-6)).clamp(0.0, 1.0);
                let amount = amount * amount * (3.0 - 2.0 * amount);
                if amount > 0.0 {
                    let target = if fog.into_background {
                        let [r, g, b, _] = shader.background(x, first_row + y, image_height);
                        [r, g, b]
                    } else {
                        fog.color
                    };
                    for (value, target) in color.iter_mut().zip(target) {
                        *value += (f32::from(target) - *value) * amount;
                    }
                }
            }
            let coverage = f32::from(visibility.edge[slot]) / 255.0;
            if coverage > 0.0 {
                for (value, edge) in color.iter_mut().zip(edge_color) {
                    *value += (edge - *value) * coverage;
                }
            }
            pixel[0] = color[0] as u8;
            pixel[1] = color[1] as u8;
            pixel[2] = color[2] as u8;
            pixel[3] = 255;
        }
    }
}

fn interpolate(weights: [f32; 3], value: impl Fn(usize) -> f32) -> f32 {
    weights[0] * value(0) + weights[1] * value(1) + weights[2] * value(2)
}

/// Per-pixel lighting for one frame.
struct Shader<'a> {
    frame: &'a Frame<'a>,
    to_light: Vec3,
    light: [f32; 3],
    exposure: f32,
    /// World direction → environment frame (up axis and rotation).
    rotation: (f32, f32),
}

impl<'a> Shader<'a> {
    fn new(frame: &'a Frame<'a>) -> Self {
        let style = frame.style;
        let light = style
            .light
            .color
            .map(|channel| srgb_to_linear(channel) * style.light.intensity.max(0.0));
        Self {
            frame,
            to_light: style.light.direction(),
            light,
            exposure: style.exposure.max(0.0),
            rotation: style.environment_rotation.sin_cos(),
        }
    }

    /// A view-space direction in the environment's Y-up frame.
    fn to_environment(&self, view: Vec3) -> Vec3 {
        let basis = &self.frame.basis;
        let world = add(
            add(scale(basis.right, view[0]), scale(basis.up, view[1])),
            scale(basis.forward, view[2]),
        );
        // Undo the up-axis mapping (Z-up worlds keep their sky above).
        let [x, y, z] = match self.frame.up {
            UpAxis::Y => world,
            UpAxis::Z => [world[0], world[2], -world[1]],
        };
        let (sin, cos) = self.rotation;
        [x * cos - z * sin, y, x * sin + z * cos]
    }

    /// Diffuse light from the surroundings on a surface facing `normal` (view space).
    fn ambient(&self, normal: Vec3) -> [f32; 3] {
        let direction = self.to_environment(normal);
        let strength = self.frame.style.environment_strength.max(0.0);
        match &self.frame.style.environment {
            Some(environment) => environment
                .irradiance(direction)
                .map(|value| value * strength),
            None => {
                // A plain studio: lighter above, darker below.
                let t = direction[1] * 0.5 + 0.5;
                [0.06 + 0.16 * t, 0.06 + 0.16 * t, 0.065 + 0.18 * t].map(|value| value * strength)
            }
        }
    }

    /// Light reflected toward the eye from `direction` (view space).
    fn reflection(&self, direction: Vec3, roughness: f32) -> [f32; 3] {
        let strength = self.frame.style.environment_strength.max(0.0);
        match &self.frame.style.environment {
            Some(environment) => environment
                .radiance(self.to_environment(direction), roughness)
                .map(|value| value * strength),
            None => self.ambient(direction),
        }
    }

    fn output(&self, linear: [f32; 3]) -> [u8; 3] {
        linear.map(|value| linear_to_srgb(tone_map(value * self.exposure)))
    }

    fn background(&self, x: u32, y: u32, image_height: u32) -> [u8; 4] {
        match self.frame.style.background {
            Background::Transparent => [0, 0, 0, 0],
            Background::Solid([r, g, b]) => [r, g, b, 255],
            Background::Gradient { top, bottom } => gradient(top, bottom, y, image_height),
            Background::Environment { blur } => {
                if self.frame.style.environment.is_none() {
                    return gradient([52, 58, 66], [22, 24, 28], y, image_height);
                }
                // The ray through this pixel.
                let view = [
                    (x as f32 + 0.5 - self.frame.center[0]) / self.frame.focal[0],
                    -(y as f32 + 0.5 - self.frame.center[1]) / self.frame.focal[1],
                    1.0,
                ];
                let [r, g, b] = self.output(self.reflection(view, blur));
                [r, g, b, 255]
            }
        }
    }

    /// The lit colour of `triangle` at barycentric `weights`.
    fn shade(&self, triangle: &Prepared, weights: [f32; 3]) -> [u8; 3] {
        let corners = &triangle.corners;
        let at =
            |value: &dyn Fn(&Corner) -> f32| interpolate(weights, |corner| value(&corners[corner]));
        let position = [at(&|c| c.view[0]), at(&|c| c.view[1]), at(&|c| c.view[2])];
        let uv = [at(&|c| c.uv[0]), at(&|c| c.uv[1])];
        let mut base = [
            at(&|c| c.color[0]),
            at(&|c| c.color[1]),
            at(&|c| c.color[2]),
        ];
        let material: Option<&Material> = (triangle.material != NO_MATERIAL)
            .then(|| &self.frame.mesh.materials[triangle.material as usize]);
        let style = self.frame.style;

        let mut metallic = 0.0;
        let mut roughness = 0.6;
        let mut occlusion = 1.0;
        let mut emissive = [0.0; 3];
        if let Some(material) = material {
            if let Some(texture) = &material.texture {
                let texel = material.sample(texture, uv[0], uv[1]);
                for (channel, value) in base.iter_mut().enumerate() {
                    *value *= srgb_float_to_linear(texel[channel]);
                }
            }
            if material.unlit {
                return self.output(base);
            }
            metallic = material.metallic;
            roughness = material.roughness;
            if let Some(texture) = &material.metallic_roughness_texture {
                let texel = material.sample(texture, uv[0], uv[1]);
                roughness *= texel[1] / 255.0;
                metallic *= texel[2] / 255.0;
            }
            if let Some(texture) = &material.occlusion_texture {
                let ambient = material.sample(texture, uv[0], uv[1])[0] / 255.0;
                occlusion = 1.0 + material.occlusion_strength * (ambient - 1.0);
            }
            emissive = material.emissive;
            if let Some(texture) = &material.emissive_texture {
                let texel = material.sample(texture, uv[0], uv[1]);
                for (channel, value) in emissive.iter_mut().enumerate() {
                    *value *= srgb_float_to_linear(texel[channel]);
                }
            }
        }
        let roughness = roughness.clamp(0.03, 1.0);
        let metallic = metallic.clamp(0.0, 1.0);

        // The shading normal.
        let to_eye = normalize_or(scale(position, -1.0), [0.0, 0.0, -1.0]);
        let mut normal = if style.shading == Shading::Flat {
            triangle.face_normal
        } else {
            normalize_or(
                [
                    at(&|c| c.normal[0]),
                    at(&|c| c.normal[1]),
                    at(&|c| c.normal[2]),
                ],
                triangle.face_normal,
            )
        };
        if style.shading == Shading::Smooth
            && let Some(texture) = material.and_then(|material| material.normal_texture.as_ref())
            && self.frame.has_tangents
        {
            let material = material.expect("normal texture belongs to a material");
            let tangent = [
                at(&|c| c.tangent[0]),
                at(&|c| c.tangent[1]),
                at(&|c| c.tangent[2]),
            ];
            let handedness = if at(&|c| c.tangent[3]) < 0.0 {
                -1.0
            } else {
                1.0
            };
            let tangent = normalize_or(sub(tangent, scale(normal, dot(normal, tangent))), [0.0; 3]);
            if tangent != [0.0; 3] {
                // View space is left-handed, so the bitangent's cross product is reversed.
                let bitangent = scale(cross(tangent, normal), handedness);
                let texel = material.sample(texture, uv[0], uv[1]);
                let mut x = texel[0] / 127.5 - 1.0;
                let mut y = texel[1] / 127.5 - 1.0;
                let z = texel[2] / 127.5 - 1.0;
                if material.normal_green_down {
                    y = -y;
                }
                x *= material.normal_scale;
                y *= material.normal_scale;
                normal = normalize_or(
                    add(
                        add(scale(tangent, x), scale(bitangent, y)),
                        scale(normal, z.max(0.0)),
                    ),
                    normal,
                );
            }
        }
        // Both sides are lit: a normal facing away from the eye is turned toward it.
        if dot(normal, to_eye) < 0.0 {
            normal = scale(normal, -1.0);
        }

        let n_dot_v = dot(normal, to_eye).max(1e-4);
        let f0: [f32; 3] = std::array::from_fn(|channel| 0.04 + (base[channel] - 0.04) * metallic);
        let diffuse_color = base.map(|value| value * (1.0 - metallic));

        // Key light: Lambert diffuse plus normalized Blinn-Phong specular with Schlick Fresnel.
        let n_dot_l = dot(normal, self.to_light);
        let mut color = [0.0_f32; 3];
        if n_dot_l > 0.0 {
            let half = normalize_or(add(self.to_light, to_eye), normal);
            let n_dot_h = dot(normal, half).max(0.0);
            let h_dot_v = dot(half, to_eye).max(0.0);
            let alpha = roughness * roughness;
            let shininess = (2.0 / (alpha * alpha) - 2.0).clamp(1.0, 4096.0);
            let distribution = (shininess + 2.0) / 8.0 * n_dot_h.powf(shininess);
            let fresnel = (1.0 - h_dot_v).powi(5);
            for channel in 0..3 {
                let specular = (f0[channel] + (1.0 - f0[channel]) * fresnel) * distribution;
                color[channel] +=
                    (diffuse_color[channel] + specular) * n_dot_l * self.light[channel];
            }
        }

        // The environment: soft light, and reflections that sharpen as roughness falls.
        let ambient = self.ambient(normal);
        let reflected = sub(scale(normal, 2.0 * n_dot_v), to_eye);
        let reflection = self.reflection(reflected, roughness);
        let grazing = (1.0 - n_dot_v).powi(5);
        for channel in 0..3 {
            let fresnel =
                f0[channel] + ((1.0 - roughness).max(f0[channel]) - f0[channel]) * grazing;
            color[channel] += (diffuse_color[channel] * ambient[channel]
                + fresnel * reflection[channel])
                * occlusion
                + emissive[channel];
        }
        self.output(color)
    }
}

fn gradient(top: [u8; 3], bottom: [u8; 3], y: u32, height: u32) -> [u8; 4] {
    let t = y as f32 / height.saturating_sub(1).max(1) as f32;
    let mix = |channel: usize| {
        (f32::from(top[channel]) + (f32::from(bottom[channel]) - f32::from(top[channel])) * t)
            .round() as u8
    };
    [mix(0), mix(1), mix(2), 255]
}

#[cfg(test)]
mod tests {
    use trend::PixelBuffer;

    use super::*;
    use crate::camera::UpAxis;
    use crate::mesh::tests::cube;

    const SQUARE: PixelAspect = PixelAspect {
        width: 1,
        height: 1,
    };

    fn draw(
        mesh: &Mesh,
        camera: &OrbitCamera,
        style: &RenderStyle,
        size: (u32, u32),
        aspect: PixelAspect,
        threads: usize,
    ) -> (PixelBuffer, FrameStats) {
        let mut buffer = PixelBuffer::new(size.0, size.1).unwrap();
        let stats =
            Renderer::with_threads(threads).render(mesh, camera, style, buffer.view_mut(), aspect);
        (buffer, stats)
    }

    fn pixel(buffer: &PixelBuffer, x: u32, y: u32) -> [u8; 4] {
        let view = buffer.view();
        let offset = y as usize * view.stride() + x as usize * 4;
        view.pixels()[offset..offset + 4].try_into().unwrap()
    }

    fn head_on(mesh: &Mesh) -> OrbitCamera {
        let mut camera = OrbitCamera::framing(mesh.bounds().unwrap(), UpAxis::Y);
        camera.yaw = 0.0;
        camera.pitch = 0.0;
        camera
    }

    fn covered(buffer: &PixelBuffer, background: [u8; 4]) -> (u32, u32) {
        let view = buffer.view();
        let (mut columns, mut rows) = (0, 0);
        for y in 0..view.height() {
            if (0..view.width()).any(|x| pixel(buffer, x, y) != background) {
                rows += 1;
            }
        }
        for x in 0..view.width() {
            if (0..view.height()).any(|y| pixel(buffer, x, y) != background) {
                columns += 1;
            }
        }
        (columns, rows)
    }

    #[test]
    fn cube_is_drawn_lit_over_the_background() {
        let mut mesh = cube();
        mesh.ensure_normals();
        let style = RenderStyle {
            background: Background::Solid([1, 2, 3]),
            ..RenderStyle::default()
        };
        let (buffer, stats) = draw(&mesh, &head_on(&mesh), &style, (64, 64), SQUARE, 1);
        assert_eq!(stats.triangles, 12);
        assert_eq!(pixel(&buffer, 0, 0), [1, 2, 3, 255]);
        let center = pixel(&buffer, 32, 32);
        assert_ne!(center, [1, 2, 3, 255]);
        assert_eq!(center[3], 255);
        // Head-on, the visible face is a square.
        let (columns, rows) = covered(&buffer, [1, 2, 3, 255]);
        assert!(columns.abs_diff(rows) <= 1, "{columns} x {rows}");
        // Culling back faces draws half the triangles and the same picture.
        let culled = RenderStyle {
            cull_back_faces: true,
            ..style.clone()
        };
        let (same, stats) = draw(&mesh, &head_on(&mesh), &culled, (64, 64), SQUARE, 1);
        assert_eq!(stats.triangles + stats.culled, 12);
        assert!(stats.culled >= 5);
        assert_eq!(same.view().pixels(), buffer.view().pixels());
    }

    #[test]
    fn pixel_aspect_keeps_proportions() {
        let mut mesh = cube();
        mesh.ensure_normals();
        let style = RenderStyle {
            background: Background::Solid([0, 0, 0]),
            ..RenderStyle::default()
        };
        // Pixels twice as tall as wide (Quadrant on a 1:2 cell): the square needs twice the columns.
        let tall = PixelAspect {
            width: 1,
            height: 2,
        };
        let (buffer, _) = draw(&mesh, &head_on(&mesh), &style, (128, 64), tall, 1);
        let (columns, rows) = covered(&buffer, [0, 0, 0, 255]);
        assert!(
            (columns as f32 / rows as f32 - 2.0).abs() < 0.1,
            "{columns} x {rows}"
        );
    }

    #[test]
    fn depth_test_and_colors_and_threads_agree() {
        // A red square in front of a larger blue one.
        let mesh = Mesh {
            positions: vec![
                [-1.0, -1.0, 0.5],
                [1.0, -1.0, 0.5],
                [1.0, 1.0, 0.5],
                [-1.0, 1.0, 0.5],
                [-2.0, -2.0, -0.5],
                [2.0, -2.0, -0.5],
                [2.0, 2.0, -0.5],
                [-2.0, 2.0, -0.5],
            ],
            // The near square is listed last, so drawing order alone would hide it.
            triangles: vec![[4, 5, 6], [4, 6, 7], [0, 1, 2], [0, 2, 3]],
            triangle_colors: vec![
                [0, 0, 255, 255],
                [0, 0, 255, 255],
                [255, 0, 0, 255],
                [255, 0, 0, 255],
            ],
            ..Mesh::default()
        };
        let camera = head_on(&mesh);
        let style = RenderStyle::default();
        let (one, _) = draw(&mesh, &camera, &style, (96, 80), SQUARE, 1);
        let center = pixel(&one, 48, 40);
        assert!(center[0] > center[2], "near red square wins: {center:?}");
        let edge = pixel(&one, 48, 12);
        assert!(edge[2] > edge[0], "far blue square around it: {edge:?}");
        for threads in [2, 3, 7] {
            let (many, _) = draw(&mesh, &camera, &style, (96, 80), SQUARE, threads);
            assert_eq!(
                many.view().pixels(),
                one.view().pixels(),
                "{threads} threads"
            );
        }
    }

    #[test]
    fn textures_are_sampled_and_alpha_tested() {
        use std::sync::Arc;

        use crate::mesh::{Material, Texture};

        // A quad split into a red left half and a transparent right half.
        let mut pixels = Vec::new();
        for _ in 0..2 {
            pixels.extend_from_slice(&[255, 0, 0, 255, 0, 0, 255, 0]);
        }
        let texture = Arc::new(Texture::new(2, 2, pixels).unwrap());
        let mesh = Mesh {
            positions: vec![
                [-1.0, -1.0, 0.0],
                [1.0, -1.0, 0.0],
                [1.0, 1.0, 0.0],
                [-1.0, 1.0, 0.0],
            ],
            triangles: vec![[0, 1, 2], [0, 2, 3]],
            uvs: vec![[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]],
            triangle_materials: vec![0, 0],
            materials: vec![Material {
                texture: Some(texture),
                alpha_test: Some(128),
                ..Material::new("half")
            }],
            ..Mesh::default()
        };
        let style = RenderStyle {
            background: Background::Solid([0, 0, 0]),
            ..RenderStyle::default()
        };
        let (buffer, _) = draw(&mesh, &head_on(&mesh), &style, (64, 64), SQUARE, 1);
        let left = pixel(&buffer, 24, 32);
        assert!(
            left[0] > left[1] + 120 && left[0] > left[2] + 120,
            "red texel: {left:?}"
        );
        // The transparent half is cut out: the background shows through.
        assert_eq!(pixel(&buffer, 44, 32), [0, 0, 0, 255]);
        assert_eq!(
            mesh.materials[0]
                .texture
                .as_ref()
                .unwrap()
                .sample(0.25, 0.5)[0],
            255.0
        );
    }

    #[test]
    fn near_plane_clipping_and_modes_never_panic() {
        let mut mesh = cube();
        mesh.ensure_normals();
        let mut camera = head_on(&mesh);
        for distance in [3.0, 1.2, 0.5, 0.01] {
            camera.distance = distance;
            for mode in RenderMode::ALL {
                for shading in [Shading::Smooth, Shading::Flat] {
                    let style = RenderStyle {
                        mode,
                        shading,
                        ..RenderStyle::default()
                    };
                    let (buffer, stats) = draw(&mesh, &camera, &style, (40, 30), SQUARE, 2);
                    assert!(
                        buffer
                            .view()
                            .pixels()
                            .chunks_exact(4)
                            .all(|pixel| pixel[3] == 255)
                    );
                    assert!(stats.triangles <= 24);
                }
            }
        }
        // Edges appear in wireframe mode: the wire colour shows up somewhere.
        camera.distance = 5.0;
        let style = RenderStyle {
            mode: RenderMode::Wireframe,
            ..RenderStyle::default()
        };
        let (buffer, _) = draw(&mesh, &camera, &style, (80, 60), SQUARE, 1);
        assert!(
            buffer
                .view()
                .pixels()
                .chunks_exact(4)
                .any(|pixel| pixel[..3] == style.wire_color)
        );
        // Empty targets and meshes are fine.
        let (_, stats) = draw(&Mesh::default(), &camera, &style, (8, 8), SQUARE, 1);
        assert_eq!(stats, FrameStats::default());
    }

    fn quad(material: Material) -> Mesh {
        let mut mesh = Mesh {
            positions: vec![
                [-1.0, -1.0, 0.0],
                [1.0, -1.0, 0.0],
                [1.0, 1.0, 0.0],
                [-1.0, 1.0, 0.0],
            ],
            triangles: vec![[0, 1, 2], [0, 2, 3]],
            uvs: vec![[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]],
            triangle_materials: vec![0, 0],
            materials: vec![material],
            ..Mesh::default()
        };
        mesh.ensure_normals();
        mesh.ensure_tangents();
        mesh
    }

    fn center_of(mesh: &Mesh, style: &RenderStyle) -> [u8; 4] {
        let (buffer, _) = draw(mesh, &head_on(mesh), style, (48, 48), SQUARE, 1);
        pixel(&buffer, 24, 24)
    }

    #[test]
    fn emissive_surfaces_glow_without_light() {
        let dark = RenderStyle {
            light: Light {
                intensity: 0.0,
                ..Light::default()
            },
            environment_strength: 0.0,
            ..RenderStyle::default()
        };
        let plain = quad(Material::new("plain"));
        assert!(center_of(&plain, &dark)[..3].iter().all(|&value| value < 8));
        let glowing = quad(Material {
            emissive: [0.0, 2.0, 0.0],
            ..Material::new("glow")
        });
        let lit = center_of(&glowing, &dark);
        assert!(lit[1] > 200 && lit[0] < 30, "{lit:?}");
    }

    #[test]
    fn shiny_metal_reflects_the_environment() {
        use crate::environment::{Environment, EnvironmentPreset};
        let sky = RenderStyle {
            environment: Some(Arc::new(Environment::preset(EnvironmentPreset::Sky))),
            light: Light {
                intensity: 0.0,
                ..Light::default()
            },
            ..RenderStyle::default()
        };
        let metal = |roughness: f32| {
            quad(Material {
                color: [255, 255, 255, 255],
                metallic: 1.0,
                roughness,
                ..Material::new("metal")
            })
        };
        // A mirror facing the camera reflects what is behind the camera: the sky's blue horizon.
        let mirror = center_of(&metal(0.05), &sky);
        assert!(mirror[2] > mirror[0], "blue sky in the mirror: {mirror:?}");
        // Without an environment the same mirror is grey.
        let grey = center_of(
            &metal(0.05),
            &RenderStyle {
                environment: None,
                ..sky.clone()
            },
        );
        assert!(grey[2].abs_diff(grey[0]) < 12, "{grey:?}");
        // The environment can be the background.
        let shown = RenderStyle {
            background: Background::Environment { blur: 0.0 },
            ..sky
        };
        let (buffer, _) = draw(
            &metal(0.5),
            &head_on(&metal(0.5)),
            &shown,
            (48, 48),
            SQUARE,
            1,
        );
        let corner = pixel(&buffer, 0, 0);
        assert_eq!(corner[3], 255);
        assert!(corner[2] > corner[0], "sky behind the model: {corner:?}");
    }

    #[test]
    fn normal_maps_tilt_the_lighting() {
        use std::sync::Arc;

        use crate::mesh::Texture;
        // A normal map tilting every normal to the right (+X in tangent space).
        let tilted = Arc::new(Texture::new(1, 1, vec![230, 128, 160, 255]).unwrap());
        let style = |azimuth: f32| RenderStyle {
            light: Light {
                azimuth,
                elevation: 0.0,
                intensity: 3.0,
                ..Light::default()
            },
            environment_strength: 0.0,
            ..RenderStyle::default()
        };
        let flat = quad(Material::new("flat"));
        let bumped = quad(Material {
            normal_texture: Some(tilted),
            ..Material::new("bumped")
        });
        // Light from the right: the tilted surface catches more of it than the flat one.
        assert!(center_of(&bumped, &style(70.0))[0] > center_of(&flat, &style(70.0))[0] + 10);
        // Light from the left: less.
        assert!(center_of(&bumped, &style(-70.0))[0] + 10 < center_of(&flat, &style(-70.0))[0]);

        // Green up (glTF / OpenGL maps) tilts toward the top of the picture (v = 0).
        let up = Arc::new(Texture::new(1, 1, vec![128, 230, 160, 255]).unwrap());
        let from_above = |elevation: f32| RenderStyle {
            light: Light {
                azimuth: 0.0,
                elevation,
                intensity: 3.0,
                ..Light::default()
            },
            environment_strength: 0.0,
            ..RenderStyle::default()
        };
        let raised = quad(Material {
            normal_texture: Some(Arc::clone(&up)),
            ..Material::new("up")
        });
        assert!(
            center_of(&raised, &from_above(70.0))[0] > center_of(&flat, &from_above(70.0))[0] + 10
        );
        // The same map read as DirectX (green down) tilts the other way.
        let lowered = quad(Material {
            normal_texture: Some(up),
            normal_green_down: true,
            ..Material::new("down")
        });
        assert!(
            center_of(&lowered, &from_above(70.0))[0] + 10 < center_of(&flat, &from_above(70.0))[0]
        );
    }

    #[test]
    fn see_through_materials_dither() {
        let glass = quad(Material {
            color: [255, 255, 255, 128],
            alpha_blend: true,
            ..Material::new("glass")
        });
        let style = RenderStyle {
            background: Background::Solid([0, 0, 0]),
            ..RenderStyle::default()
        };
        let (buffer, _) = draw(&glass, &head_on(&glass), &style, (32, 32), SQUARE, 1);
        let covered = (8..24)
            .flat_map(|y| (8..24).map(move |x| (x, y)))
            .filter(|&(x, y)| pixel(&buffer, x, y) != [0, 0, 0, 255])
            .count();
        assert!((96..=160).contains(&covered), "about half drawn: {covered}");
    }
}

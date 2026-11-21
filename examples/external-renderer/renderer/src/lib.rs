//! `tiny-raster`: a small software 3D renderer standing in for any external
//! renderer (a game engine, a GPU readback, an emulator, a browser).
//!
//! It deliberately knows nothing about TRend or terminals. It renders into a
//! framebuffer it owns, in the conventions GPU APIs commonly use:
//!
//! - BGRA byte order;
//! - bottom-left origin, so row 0 in memory is the bottom of the image;
//! - premultiplied alpha, with a transparent background;
//! - rows padded to 256 bytes.
//!
//! It describes that memory with its own [`FrameInfo`]. Mapping it onto a
//! consumer's types is the consumer's job.

use core::f32::consts::TAU;

/// Byte order of each 4-byte pixel.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChannelOrder {
    Bgra,
}

/// Which image row comes first in memory.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Origin {
    BottomLeft,
}

/// How color relates to alpha.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AlphaConvention {
    Premultiplied,
}

/// Everything a consumer needs to read [`Renderer::pixels`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FrameInfo {
    pub width: u32,
    pub height: u32,
    /// Bytes from one row in memory to the next.
    pub stride: usize,
    pub order: ChannelOrder,
    pub origin: Origin,
    pub alpha: AlphaConvention,
}

type Vec3 = [f32; 3];

/// Renders a lit torus and a translucent orbiting panel with a depth buffer.
pub struct Renderer {
    info: FrameInfo,
    pixels: Vec<u8>,
    depth: Vec<f32>,
    vertices: Vec<Vec3>,
    triangles: Vec<[usize; 3]>,
}

const RING_SEGMENTS: usize = 28;
const TUBE_SEGMENTS: usize = 14;
const ROW_ALIGNMENT: usize = 256;

impl Renderer {
    /// A renderer with an empty framebuffer; call [`Self::resize`] first.
    pub fn new() -> Self {
        let mut vertices = Vec::new();
        for ring in 0..RING_SEGMENTS {
            let u = ring as f32 / RING_SEGMENTS as f32 * TAU;
            for tube in 0..TUBE_SEGMENTS {
                let v = tube as f32 / TUBE_SEGMENTS as f32 * TAU;
                let radius = 1.0 + 0.42 * v.cos();
                vertices.push([radius * u.cos(), 0.42 * v.sin(), radius * u.sin()]);
            }
        }
        let mut triangles = Vec::new();
        for ring in 0..RING_SEGMENTS {
            for tube in 0..TUBE_SEGMENTS {
                let index =
                    |r: usize, t: usize| (r % RING_SEGMENTS) * TUBE_SEGMENTS + t % TUBE_SEGMENTS;
                let (a, b) = (index(ring, tube), index(ring + 1, tube));
                let (c, d) = (index(ring + 1, tube + 1), index(ring, tube + 1));
                triangles.push([a, b, c]);
                triangles.push([a, c, d]);
            }
        }
        Self {
            info: FrameInfo {
                width: 0,
                height: 0,
                stride: 0,
                order: ChannelOrder::Bgra,
                origin: Origin::BottomLeft,
                alpha: AlphaConvention::Premultiplied,
            },
            pixels: Vec::new(),
            depth: Vec::new(),
            vertices,
            triangles,
        }
    }

    /// Sets the framebuffer size. Reallocates only when it grows.
    pub fn resize(&mut self, width: u32, height: u32) {
        let stride = (width as usize * 4).next_multiple_of(ROW_ALIGNMENT);
        self.info.width = width;
        self.info.height = height;
        self.info.stride = stride;
        self.pixels.resize(stride * height as usize, 0);
        self.depth
            .resize(width as usize * height as usize, f32::INFINITY);
    }

    pub fn info(&self) -> FrameInfo {
        self.info
    }

    /// The framebuffer bytes described by [`Self::info`].
    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    /// Renders the scene at `time` seconds. `pixel_aspect` is the width of
    /// one pixel relative to its height, so the output is not stretched on
    /// displays with non-square pixels.
    pub fn render(&mut self, time: f32, pixel_aspect: f32) {
        let FrameInfo { width, height, .. } = self.info;
        self.pixels.fill(0); // Transparent background.
        self.depth.fill(f32::INFINITY);
        if width == 0 || height == 0 {
            return;
        }

        let (sin_a, cos_a) = (time * 0.7).sin_cos();
        let (sin_b, cos_b) = (time * 0.45 + 0.6).sin_cos();
        let transform = |[x, y, z]: Vec3| -> Vec3 {
            let (x, z) = (x * cos_a + z * sin_a, -x * sin_a + z * cos_a);
            let (y, z) = (y * cos_b - z * sin_b, y * sin_b + z * cos_b);
            [x, y, z + 3.6]
        };
        let focal = height as f32 * 1.05;
        let project = |[x, y, z]: Vec3| -> Vec3 {
            [
                width as f32 / 2.0 + x / z * focal / pixel_aspect,
                height as f32 / 2.0 - y / z * focal,
                z,
            ]
        };

        let light = normalize([-0.5, 0.8, -0.6]);
        for triangle_index in 0..self.triangles.len() {
            let [a, b, c] = self.triangles[triangle_index].map(|i| transform(self.vertices[i]));
            let normal = normalize(cross(sub(b, a), sub(c, a)));
            if dot(normal, a) >= 0.0 {
                continue; // Back-facing.
            }
            let diffuse = (-dot(normal, light)).max(0.0);
            let ring = triangle_index / (2 * TUBE_SEGMENTS);
            let base: Vec3 = if ring % 4 < 2 {
                [255.0, 140.0, 60.0]
            } else {
                [70.0, 150.0, 255.0]
            };
            let shade = 0.35 + 0.65 * diffuse;
            let color = [base[0] * shade, base[1] * shade, base[2] * shade, 1.0];
            self.fill_triangle([project(a), project(b), project(c)], color, true);
        }

        // A translucent panel orbiting the torus, drawn after the opaque
        // pass: depth-tested, not depth-written, blended "over".
        let angle = time * 1.3;
        let center = [
            2.0 * angle.cos(),
            0.3 * (time * 2.0).sin(),
            2.0 * angle.sin(),
        ];
        let corner = |dx: f32, dy: f32| -> Vec3 {
            transform([
                center[0] + dx * angle.sin(),
                center[1] + dy,
                center[2] - dx * angle.cos(),
            ])
        };
        let quad = [
            corner(-0.5, -0.5),
            corner(0.5, -0.5),
            corner(0.5, 0.5),
            corner(-0.5, 0.5),
        ];
        let tint = [80.0, 255.0, 200.0, 0.45];
        let projected = quad.map(project);
        self.fill_triangle([projected[0], projected[1], projected[2]], tint, false);
        self.fill_triangle([projected[0], projected[2], projected[3]], tint, false);
    }

    /// Rasterizes a screen-space triangle. `color` is straight RGB (0–255)
    /// and alpha (0–1); pixels are stored premultiplied, BGRA, bottom-up.
    fn fill_triangle(&mut self, [p0, p1, p2]: [Vec3; 3], color: [f32; 4], opaque: bool) {
        let FrameInfo {
            width,
            height,
            stride,
            ..
        } = self.info;
        let edge = |a: Vec3, b: Vec3, x: f32, y: f32| {
            (b[0] - a[0]) * (y - a[1]) - (b[1] - a[1]) * (x - a[0])
        };
        let area = edge(p0, p1, p2[0], p2[1]);
        if area.abs() < 1e-6 {
            return;
        }
        let min_x = p0[0].min(p1[0]).min(p2[0]).floor().max(0.0) as u32;
        let max_x = (p0[0].max(p1[0]).max(p2[0]).ceil().max(0.0) as u32).min(width);
        let min_y = p0[1].min(p1[1]).min(p2[1]).floor().max(0.0) as u32;
        let max_y = (p0[1].max(p1[1]).max(p2[1]).ceil().max(0.0) as u32).min(height);
        let alpha = color[3];
        for y in min_y..max_y {
            // Bottom-left origin: image row `y` lives at memory row height-1-y.
            let row = (height - 1 - y) as usize * stride;
            for x in min_x..max_x {
                let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                let w0 = edge(p1, p2, px, py) / area;
                let w1 = edge(p2, p0, px, py) / area;
                let w2 = edge(p0, p1, px, py) / area;
                if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                    continue;
                }
                let z = w0 * p0[2] + w1 * p1[2] + w2 * p2[2];
                let depth = &mut self.depth[(y * width + x) as usize];
                if z >= *depth {
                    continue;
                }
                let pixel = &mut self.pixels[row + x as usize * 4..row + x as usize * 4 + 4];
                if opaque {
                    *depth = z;
                    pixel.copy_from_slice(&[color[2] as u8, color[1] as u8, color[0] as u8, 255]);
                } else {
                    // Premultiplied "over": dst = src·α + dst·(1 − α).
                    let keep = 1.0 - alpha;
                    let over = [
                        color[2] * alpha,
                        color[1] * alpha,
                        color[0] * alpha,
                        255.0 * alpha,
                    ];
                    for (channel, value) in pixel.iter_mut().zip(over) {
                        *channel = (value + f32::from(*channel) * keep).round().min(255.0) as u8;
                    }
                }
            }
        }
    }
}

impl Default for Renderer {
    fn default() -> Self {
        Self::new()
    }
}

fn sub(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn cross(a: Vec3, b: Vec3) -> Vec3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn dot(a: Vec3, b: Vec3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn normalize(v: Vec3) -> Vec3 {
    let length = dot(v, v).sqrt().max(f32::EPSILON);
    [v[0] / length, v[1] / length, v[2] / length]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_are_bottom_up_premultiplied_bgra_with_padded_rows() {
        let mut renderer = Renderer::new();
        renderer.resize(70, 40);
        renderer.render(0.3, 1.0);
        let info = renderer.info();
        assert_eq!(info.stride, 512);
        assert_eq!(renderer.pixels().len(), info.stride * 40);
        let pixels = renderer.pixels();
        let mut opaque = 0;
        for row in pixels.chunks_exact(info.stride) {
            for pixel in row[..70 * 4].chunks_exact(4) {
                // Premultiplied: no channel exceeds alpha.
                assert!(pixel[..3].iter().all(|&channel| channel <= pixel[3]));
                opaque += usize::from(pixel[3] == 255);
            }
            // Padding is never written.
            assert!(row[70 * 4..].iter().all(|&byte| byte == 0));
        }
        assert!(opaque > 100, "the torus is drawn");
    }
}
